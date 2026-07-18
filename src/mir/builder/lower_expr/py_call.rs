//! Lowering for calls into Python: building the argument list(s), picking
//! the right `__olive_py_call*` entry point, and choosing between the
//! tagged fast path and the legacy pre-converting fallback.

use super::super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{CallArg, Expr, ExprKind};
use crate::semantic::types::Type;
use crate::span::Span;

/// Argument-encoding tag: how a raw call-argument word decodes on the
/// Python side, chosen from the argument's static type. Mirrors
/// `python_writeback::ARG_*` in the runtime crate exactly -- the two crates
/// share no dependency, so keep this enumeration in lockstep by hand if
/// either changes.
const ARG_PYOBJECT: i64 = 0;
const ARG_INT: i64 = 1;
const ARG_FLOAT: i64 = 2;
const ARG_STR: i64 = 3;
const ARG_BOOL: i64 = 4;
const ARG_ANY: i64 = 5;
const ARG_NONE: i64 = 6;
const ARG_BYTES: i64 = 7;

/// Result-fusion tag: how a call's Python return converts directly into the
/// scalar the checker already knows it produces, instead of wrapping a
/// handle and paying a second boundary crossing to unwrap it. Mirrors
/// `python_ret::RET_*` in the runtime crate exactly; packed into the top 4
/// bits of the same `arg_tags` word the arity-specialized entry points pass
/// (arity 0-4 never uses more than the low 16 bits for real argument tags).
pub(super) const RET_HANDLE: i64 = 0;
const RET_INT: i64 = 1;
const RET_FLOAT: i64 = 2;
const RET_STR: i64 = 3;
const RET_BOOL: i64 = 4;
const RET_ANY: i64 = 5;
const RET_NONE: i64 = 6;

/// Which family of py-call entry point a call site wants: `Result`-returning
/// (inside a `try`-propagating expression) or the plain form that aborts on
/// an uncaught Python exception.
pub(super) enum PyCallFlavor {
    Safe,
    Unsafe,
}

/// Positional/keyword operands and tag words for one py-call, ready for
/// `MirBuilder::emit_py_call`. `fast_path` is false for a splat/kwsplat call
/// or one with more than 16 positional or 16 keyword arguments -- the real
/// arity isn't known at compile time (splat) or doesn't fit the packed tag
/// word (16 args), so `pos_ops`/`kw_ops` hold pre-converted `PyObject`
/// operands (the legacy `emit_to_py_arg` path) instead of raw words, and
/// `arg_tags`/`kw_arg_tags` are unused.
pub(super) struct PyCallArgs {
    pos_ops: Vec<Operand>,
    kw_ops: Vec<Operand>,
    /// R15: keyword *values* only, parallel to `kw_arg_tags`/`kw_coll_tags`
    /// -- names are compile-time constants, so they never need a runtime
    /// slot of their own; see `kw_names_packed`. Populated only when
    /// `fast_path` (a splat/kwsplat call keeps names and values interleaved
    /// in `kw_ops` for the legacy dict-building path instead).
    kw_vals: Vec<Operand>,
    /// R15: every keyword name, comma-joined into one compile-time
    /// constant (kwarg names are always valid Python identifiers, so a
    /// comma can never appear inside one) -- the runtime interns and
    /// tuple-packs this once per unique call site and caches the result
    /// forever, keyed by this constant's own address. `None` when there
    /// are no keyword arguments.
    kw_names_packed: Option<String>,
    arg_tags: i64,
    coll_tags: i64,
    kw_arg_tags: i64,
    kw_coll_tags: i64,
    fast_path: bool,
}

impl PyCallArgs {
    /// Unpacks the fields `py_call_kw_arity.rs`'s method-call emitter needs
    /// -- that file lives outside this module, so it can't destructure a
    /// private-field struct literal directly the way this file's own
    /// `emit_py_call_method_kw_v` does.
    pub(super) fn into_kw_parts(
        self,
    ) -> (
        Vec<Operand>,
        Vec<Operand>,
        Option<String>,
        i64,
        i64,
        i64,
        i64,
    ) {
        (
            self.pos_ops,
            self.kw_vals,
            self.kw_names_packed,
            self.arg_tags,
            self.coll_tags,
            self.kw_arg_tags,
            self.kw_coll_tags,
        )
    }
}

impl<'a> MirBuilder<'a> {
    /// The scalar-kind tag (2/3/4/5, matching `python_writeback`'s
    /// `TAG_*_LIST` constants) for a concrete scalar element/value type, or
    /// `None` when it's `Any`/anything else needing the boxed path.
    fn py_scalar_kind(ty: &Type) -> Option<i64> {
        match ty {
            t if Self::is_int_ty(t) => Some(2),
            Type::Float | Type::F32 => Some(3),
            Type::Bool => Some(4),
            Type::Str => Some(5),
            _ => None,
        }
    }

    /// The 4-bit copy-out tag for a py-call argument's static type: which
    /// sync routine `sync_back` runs on it after the call, chosen from the
    /// argument's *declared* Olive type since the runtime cannot recover an
    /// element type from a raw list word alone. A concretely-typed dict/set
    /// (`{str: int}`, `set[int]`) stores its values raw, the same as a typed
    /// list, so it gets its own tag distinct from an Any-valued one (`{str:
    /// Any}` boxes every value) -- see the tag table in `python_writeback.rs`.
    ///
    /// A container whose element/value is itself a concrete container (a
    /// `[[int]]`, `{str: [int]}`, ...) gets `0` (no copy-out attempted, same
    /// as an untagged argument): the Any-boxed recursive path (tag 1/6/7)
    /// only matches a genuinely dynamic `Any` slot, whose runtime decode
    /// (`py_to_any_internal`) always boxes what it finds -- applying that to
    /// a concretely-typed inner container would rebuild it with boxed
    /// elements the outer type never declared, corrupting it exactly like
    /// the untyped-dict/set bug this tag scheme exists to avoid. Expressing
    /// "sync a concretely nested container" correctly needs a real, per-level
    /// type descriptor (the `type_descriptor`/`*_typed` machinery elsewhere
    /// already builds these for hashing/eq/copy); that is follow-up work, not
    /// this phase's flat tag word.
    ///
    /// Not a collection -> `0`.
    pub(super) fn py_collection_tag(ty: &Type) -> i64 {
        match ty {
            Type::List(elem) => match Self::py_scalar_kind(elem) {
                Some(k) => k,
                None if elem.as_ref() == &Type::Any => 1,
                None => 0,
            },
            Type::Dict(_, val) => match Self::py_scalar_kind(val) {
                Some(2) => 8,
                Some(3) => 9,
                Some(4) => 10,
                Some(5) => 11,
                _ if val.as_ref() == &Type::Any => 6,
                _ => 0,
            },
            Type::Set(elem) => match Self::py_scalar_kind(elem) {
                Some(2) => 12,
                Some(3) => 13,
                Some(4) => 14,
                Some(5) => 15,
                _ if elem.as_ref() == &Type::Any => 7,
                _ => 0,
            },
            _ => 0,
        }
    }

    /// Packs up to 16 per-argument tags into one `i64`, 4 bits per arg,
    /// little-endian. More than 16 packs `0` for every slot: for the
    /// collection-tag word this means no copy-out is attempted (same as an
    /// argument that was never a collection); the encode-tag word never
    /// reaches this branch with more than 16 entries since `build_py_call_args`
    /// disables the fast path first.
    fn pack_tags(tags: &[i64]) -> i64 {
        if tags.len() > 16 {
            return 0;
        }
        tags.iter()
            .enumerate()
            .fold(0i64, |acc, (i, &t)| acc | (t << (i * 4)))
    }

    /// The encode tag for a non-collection argument's static type: how its
    /// raw, unconverted word decodes inside the call's single GIL region.
    /// A `Type::Null` argument gets its own tag rather than falling into
    /// `ARG_ANY`: a bare `None` local's raw representation is the plain
    /// sentinel `0`, bit-identical to integer zero, so only a dedicated tag
    /// (not a runtime guess) tells the two apart. Everything not named
    /// individually (a struct, tuple, enum, or a container whose
    /// `py_collection_tag` came back `0`) gets `ARG_ANY`: `olive_any_to_py`
    /// is a strict superset of the legacy fallback (`olive_to_py`) here,
    /// since it also strips an inline `Any` tag first, so this can't regress
    /// anything the old per-arg `emit_to_py_arg` path already handled.
    fn py_arg_tag(ty: &Type) -> i64 {
        if ty.is_py_value() {
            return ARG_PYOBJECT;
        }
        match ty {
            t if Self::is_int_ty(t) => ARG_INT,
            Type::Float | Type::F32 => ARG_FLOAT,
            Type::Str => ARG_STR,
            Type::Bool => ARG_BOOL,
            Type::Bytes => ARG_BYTES,
            Type::Null => ARG_NONE,
            _ => ARG_ANY,
        }
    }

    /// The result-fusion tag and locally-typed result for a call's declared
    /// result type, or `None` when it isn't one of the scalars R10 fuses --
    /// the caller keeps `RET_HANDLE`/`Type::PyObject` in that case, exactly
    /// the pre-R10 behavior. `Type::Null` covers both a genuinely `None`-typed
    /// stub and a statement-position call whose result is discarded
    /// (`lower_py_call_discard` forces the declared type to `Null` for that).
    pub(super) fn py_ret_tag(ty: &Type) -> (i64, Type) {
        match ty {
            t if Self::is_int_ty(t) => (RET_INT, t.clone()),
            Type::Float | Type::F32 => (RET_FLOAT, ty.clone()),
            Type::Str => (RET_STR, Type::Str),
            Type::Bool => (RET_BOOL, Type::Bool),
            Type::Null => (RET_NONE, Type::Null),
            Type::Any => (RET_ANY, Type::Any),
            _ => (RET_HANDLE, Type::PyObject),
        }
    }

    /// R19: the `ARG_*` tag for a value crossing into a `PyCFunction` --
    /// exactly `py_arg_tag`'s scalar/str/PyObject cases, since those are
    /// the only shapes the trampoline's fixed `ARG_*` dispatch decodes.
    /// Anything else is unreachable here: the E0603 checker
    /// (`unify.rs::check_py_callable_shape`) already rejected it before
    /// this ever lowers.
    fn py_callable_tag(ty: &Type) -> i64 {
        if ty.is_py_value() {
            return ARG_PYOBJECT;
        }
        match ty {
            t if Self::is_int_ty(t) => ARG_INT,
            Type::Float | Type::F32 => ARG_FLOAT,
            Type::Str => ARG_STR,
            Type::Bool => ARG_BOOL,
            _ => unreachable!("py_callable_tag: `{ty}` should have been rejected by E0603"),
        }
    }

    /// R19: converts a closure-record operand (built by `build_closure_value`
    /// for any `Type::Fn` value, capturing or not -- every such value is
    /// uniformly a record pointer) into a genuine Python callable. The
    /// record's own `__thunk`/`__desc` fields (offsets 8/16,
    /// `closures.rs::build_closure_value`'s layout) are read back at
    /// runtime by `olive_py_make_callable`, so this just packs the arg/ret
    /// tags and emits the call. `RUNTIME_ESCAPES` marks position 0 of this
    /// call as escaping: ownership of the record transfers into the
    /// returned capsule, so the caller must not (and, per the ownership
    /// pass, will not) drop it afterward.
    pub(super) fn emit_fn_to_py_callable(
        &mut self,
        op: Operand,
        params: &[Type],
        ret: &Type,
        span: Span,
    ) -> Operand {
        let mut tags: i64 = (params.len() as i64) << 56;
        for (i, p) in params.iter().enumerate() {
            tags |= Self::py_callable_tag(p) << (i * 4);
        }
        let ret_tag = if *ret == Type::Null {
            ARG_NONE
        } else {
            Self::py_callable_tag(ret)
        };
        tags |= ret_tag << 60;

        let tmp = self.new_local(Type::PyObject, None, true);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(
                        "__olive_py_make_callable".to_string(),
                    )),
                    args: vec![op, Operand::Constant(Constant::Int(tags))],
                },
            ),
            span,
        );
        self.operand_for_local(tmp)
    }

    /// Builds the raw positional/keyword operands and both tag words for one
    /// py-call: shared by `lower_py_call_safe`, `lower_pyobject_call`, and
    /// the attr-call branch in `call_method.rs`, the three places a call into
    /// Python gets lowered.
    pub(super) fn build_py_call_args(
        &mut self,
        args: &[CallArg],
        arg_ops: Vec<Operand>,
        arg_kw_names: Vec<Option<String>>,
        span: Span,
    ) -> PyCallArgs {
        let has_splat = args
            .iter()
            .any(|a| matches!(a, CallArg::Splat(_) | CallArg::KwSplat(_)));
        let pos_count = arg_kw_names.iter().filter(|k| k.is_none()).count();
        let kw_count = arg_kw_names.len() - pos_count;
        let fast_path = !has_splat && pos_count <= 16 && kw_count <= 16;

        let mut pos_ops = Vec::new();
        let mut kw_ops = Vec::new();
        let mut kw_vals = Vec::new();
        let mut kw_names = Vec::new();
        let mut pos_coll_tags = Vec::new();
        let mut pos_arg_tags = Vec::new();
        let mut kw_coll_tags = Vec::new();
        let mut kw_arg_tags = Vec::new();

        for (i, (op, kw_name)) in arg_ops.into_iter().zip(arg_kw_names).enumerate() {
            let arg_ty = args
                .get(i)
                .map(|a| match a {
                    CallArg::Positional(e)
                    | CallArg::Splat(e)
                    | CallArg::KwSplat(e)
                    | CallArg::Keyword(_, e) => self.get_type(e.id),
                })
                .unwrap_or(Type::Any);
            // R19: a function-typed argument (e.g. `mod.apply(my_fn)`) never
            // reaches the ordinary `emit_to_py_arg`/`coerce` path below --
            // the fast path (no kwargs, arity <= 4) skips that call
            // entirely and would otherwise pass the raw closure-record
            // pointer straight through as an untagged `Any` word. Convert
            // it to a real `PyCFunction` up front, uniformly for both paths.
            let (op, arg_ty) = match &arg_ty {
                Type::Fn(params, ret, _) => {
                    let converted = self.emit_fn_to_py_callable(op, params, ret, span);
                    (converted, Type::PyObject)
                }
                _ => (op, arg_ty),
            };
            let coll_tag = Self::py_collection_tag(&arg_ty);
            // A collection-tagged slot's decode is entirely driven by
            // `coll_tag` at the runtime end (`convert_arg_tagged` checks it
            // first); the encode tag is never consulted for that slot, so
            // its exact value doesn't matter -- `ARG_PYOBJECT` documents
            // "not applicable" without inventing a new sentinel.
            let arg_tag = if coll_tag != 0 {
                ARG_PYOBJECT
            } else {
                Self::py_arg_tag(&arg_ty)
            };
            let py_op = if fast_path {
                op
            } else {
                self.emit_to_py_arg(op, &arg_ty, span)
            };
            if let Some(name) = kw_name {
                if fast_path {
                    kw_vals.push(py_op.clone());
                    kw_names.push(name.clone());
                }
                kw_ops.push(Operand::Constant(Constant::Str(name)));
                kw_ops.push(py_op);
                kw_coll_tags.push(coll_tag);
                kw_arg_tags.push(arg_tag);
            } else {
                pos_ops.push(py_op);
                pos_coll_tags.push(coll_tag);
                pos_arg_tags.push(arg_tag);
            }
        }

        PyCallArgs {
            pos_ops,
            kw_ops,
            kw_vals,
            kw_names_packed: if kw_names.is_empty() {
                None
            } else {
                Some(kw_names.join(","))
            },
            arg_tags: if fast_path {
                Self::pack_tags(&pos_arg_tags)
            } else {
                0
            },
            coll_tags: if has_splat {
                0
            } else {
                Self::pack_tags(&pos_coll_tags)
            },
            kw_arg_tags: if fast_path {
                Self::pack_tags(&kw_arg_tags)
            } else {
                0
            },
            kw_coll_tags: if has_splat {
                0
            } else {
                Self::pack_tags(&kw_coll_tags)
            },
            fast_path,
        }
    }

    /// Emits the `args_list`/`kwargs_list` aggregates and the call to
    /// whichever `__olive_py_call*` entry point matches `call_args.fast_path`
    /// and `flavor`, returning the result operand (typed `result_ty`).
    pub(super) fn emit_py_call(
        &mut self,
        callee_op: Operand,
        call_args: PyCallArgs,
        flavor: PyCallFlavor,
        result_ty: Type,
        span: Span,
    ) -> Operand {
        let PyCallArgs {
            pos_ops,
            kw_ops,
            kw_vals,
            kw_names_packed,
            arg_tags,
            coll_tags,
            kw_arg_tags,
            kw_coll_tags,
            fast_path,
        } = call_args;

        // A positional-only, tagged-fast-path call with 0-4 arguments skips
        // the `args_list` aggregate entirely -- each argument goes straight
        // into a call register through a dedicated arity-specialized entry
        // point, so this call site allocates nothing. Arity 5-16 and any
        // kwargs call over the combined cap below keep the list-based path.
        if kw_ops.is_empty() && fast_path && pos_ops.len() <= 4 {
            return self.emit_py_call_arity(
                callee_op,
                pos_ops,
                (coll_tags, arg_tags),
                flavor,
                result_ty,
                span,
            );
        }

        // Same no-allocation shape extended to kwargs: `positional +
        // keyword <= 4` skips both `args_list` and `kwvals_list`.
        if !kw_ops.is_empty() && fast_path && pos_ops.len() + kw_vals.len() <= 4 {
            let kwnames = kw_names_packed.clone().unwrap_or_default();
            return self.emit_py_call_kw_arity(
                callee_op,
                pos_ops,
                kw_vals,
                kwnames,
                coll_tags,
                arg_tags,
                kw_coll_tags,
                kw_arg_tags,
                flavor,
                span,
            );
        }

        let args_list = self.new_local(Type::List(Box::new(Type::Any)), None, true);
        self.push_statement(
            StatementKind::Assign(args_list, Rvalue::Aggregate(AggregateKind::List, pos_ops)),
            span,
        );

        let mut call_operands = vec![
            callee_op,
            Operand::Copy(args_list),
            Operand::Constant(Constant::Int(coll_tags)),
        ];
        if fast_path {
            call_operands.push(Operand::Constant(Constant::Int(arg_tags)));
        }

        let name = if kw_ops.is_empty() {
            match (fast_path, &flavor) {
                (true, PyCallFlavor::Safe) => "__olive_py_call_t_safe",
                (true, PyCallFlavor::Unsafe) => "__olive_py_call_t",
                (false, PyCallFlavor::Safe) => "__olive_py_call_safe",
                (false, PyCallFlavor::Unsafe) => "__olive_py_call",
            }
        } else if fast_path {
            // R15: names are a packed compile-time constant, not part of
            // the runtime aggregate -- `kwvals_list` holds values only.
            let kwvals_list = self.new_local(Type::List(Box::new(Type::Any)), None, true);
            self.push_statement(
                StatementKind::Assign(kwvals_list, Rvalue::Aggregate(AggregateKind::List, kw_vals)),
                span,
            );
            let kwnames = kw_names_packed.unwrap_or_default();
            call_operands.push(Operand::Constant(Constant::Str(kwnames)));
            call_operands.push(Operand::Copy(kwvals_list));
            call_operands.push(Operand::Constant(Constant::Int(kw_coll_tags)));
            call_operands.push(Operand::Constant(Constant::Int(kw_arg_tags)));
            match flavor {
                PyCallFlavor::Safe => "__olive_py_call_kw_v_safe",
                PyCallFlavor::Unsafe => {
                    // R17: the location the aborting path needs is folded
                    // in as a trailing arg instead of a separate
                    // `__olive_py_set_loc` statement -- the `_safe` twin
                    // never aborts, so it carries no location at all.
                    call_operands.push(Operand::Constant(Constant::Str(self.call_loc_str(span))));
                    "__olive_py_call_kw_v"
                }
            }
        } else {
            let kwargs_list = self.new_local(Type::List(Box::new(Type::Any)), None, true);
            self.push_statement(
                StatementKind::Assign(kwargs_list, Rvalue::Aggregate(AggregateKind::List, kw_ops)),
                span,
            );
            call_operands.push(Operand::Copy(kwargs_list));
            call_operands.push(Operand::Constant(Constant::Int(kw_coll_tags)));
            match flavor {
                PyCallFlavor::Safe => "__olive_py_call_kw_safe",
                PyCallFlavor::Unsafe => "__olive_py_call_kw",
            }
        };

        // This path (the list-based `_t`/kwargs entries) never fuses the
        // result -- only the arity-specialized shells above do -- so the
        // assigned local is always a plain handle regardless of what
        // `result_ty` the caller passed for a potential arity fusion.
        let result = self.new_local(Type::PyObject, None, true);
        self.push_statement(
            StatementKind::Assign(
                result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(name.to_string())),
                    args: call_operands,
                },
            ),
            span,
        );
        self.operand_for_local(result)
    }

    /// Emits a call through `__olive_py_call{0..4}(_safe)`, `pos_ops`
    /// passed straight as call registers -- no `args_list` aggregate, so no
    /// list allocation for this call site at all. `olive_py_call0` takes an
    /// `arg_tags` word purely to carry `ret_tag`; every other arity shares it
    /// with the real per-argument tags `olive_py_call_t` also uses.
    ///
    /// `result_ty` only drives fusion for the `Unsafe` flavor: `Safe`'s
    /// result is a `Result` wire value (its `result_ty` is always `Type::Any`,
    /// a placeholder for that wire, not a real target type), so ret_tag stays
    /// `RET_HANDLE` there and the assigned local keeps `result_ty` as given,
    /// unchanged from before R10.
    fn emit_py_call_arity(
        &mut self,
        callee_op: Operand,
        pos_ops: Vec<Operand>,
        tags: (i64, i64),
        flavor: PyCallFlavor,
        result_ty: Type,
        span: Span,
    ) -> Operand {
        let (coll_tags, arg_tags) = tags;
        let name = match (pos_ops.len(), &flavor) {
            (0, PyCallFlavor::Unsafe) => "__olive_py_call0",
            (0, PyCallFlavor::Safe) => "__olive_py_call0_safe",
            (1, PyCallFlavor::Unsafe) => "__olive_py_call1",
            (1, PyCallFlavor::Safe) => "__olive_py_call1_safe",
            (2, PyCallFlavor::Unsafe) => "__olive_py_call2",
            (2, PyCallFlavor::Safe) => "__olive_py_call2_safe",
            (3, PyCallFlavor::Unsafe) => "__olive_py_call3",
            (3, PyCallFlavor::Safe) => "__olive_py_call3_safe",
            (4, PyCallFlavor::Unsafe) => "__olive_py_call4",
            (4, PyCallFlavor::Safe) => "__olive_py_call4_safe",
            (n, _) => unreachable!("emit_py_call_arity: arity {n} out of range"),
        };

        let (ret_tag, local_ty) = match flavor {
            PyCallFlavor::Unsafe => Self::py_ret_tag(&result_ty),
            PyCallFlavor::Safe => (RET_HANDLE, result_ty),
        };
        let tagged_arg_tags = arg_tags | (ret_tag << 60);

        let mut call_operands = vec![callee_op];
        if pos_ops.is_empty() {
            call_operands.push(Operand::Constant(Constant::Int(tagged_arg_tags)));
        } else {
            call_operands.push(Operand::Constant(Constant::Int(coll_tags)));
            call_operands.push(Operand::Constant(Constant::Int(tagged_arg_tags)));
            call_operands.extend(pos_ops);
        }
        // R17: the abort path needs the call site to report an uncaught
        // exception against; folded in as a trailing arg instead of a
        // separate `__olive_py_set_loc` statement. `Safe` never aborts.
        if matches!(flavor, PyCallFlavor::Unsafe) {
            call_operands.push(Operand::Constant(Constant::Str(self.call_loc_str(span))));
        }

        let result = self.new_local(local_ty, None, true);
        self.push_statement(
            StatementKind::Assign(
                result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(name.to_string())),
                    args: call_operands,
                },
            ),
            span,
        );
        self.operand_for_local(result)
    }

    /// Emits `obj.attr(args...)`. A positional-only, tagged-fast-path call
    /// with 0-4 arguments fuses the getattr into the call itself via
    /// `__olive_py_call_method{0..4}` -- no separate getattr call, no
    /// bound-method handle ever created at this level (the runtime may still
    /// build one internally as a fallback if vectorcall-method or interning
    /// isn't available). Any other shape (kwargs, arity 5+, splat) keeps the
    /// original two-step getattr-then-call path, unchanged.
    pub(super) fn emit_py_method_call(
        &mut self,
        obj_op: Operand,
        attr: String,
        call_args: PyCallArgs,
        flavor: PyCallFlavor,
        result_ty: Type,
        span: Span,
    ) -> Operand {
        if call_args.kw_ops.is_empty() && call_args.fast_path && call_args.pos_ops.len() <= 4 {
            return self
                .emit_py_call_method_arity(obj_op, attr, call_args, flavor, result_ty, span);
        }
        if !call_args.kw_ops.is_empty()
            && call_args.fast_path
            && call_args.pos_ops.len() + call_args.kw_vals.len() <= 4
        {
            return self.emit_py_call_method_kw_arity(obj_op, attr, call_args, flavor, span);
        }
        if !call_args.kw_ops.is_empty() && call_args.fast_path {
            return self.emit_py_call_method_kw_v(obj_op, attr, call_args, flavor, span);
        }

        let attr_local = self.new_local(Type::PyObject, None, true);
        self.push_statement(
            StatementKind::Assign(
                attr_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_py_getattr".to_string())),
                    args: vec![obj_op, Operand::Constant(Constant::Str(attr))],
                },
            ),
            span,
        );
        self.emit_py_call(
            Operand::Copy(attr_local),
            call_args,
            flavor,
            result_ty,
            span,
        )
    }

    /// R15: `obj.attr(*positional, **keyword)` on the tagged fast path.
    /// `PyObject_VectorcallMethod` resolves the bound method and calls it in
    /// one step (falls back to a plain `GetAttr` + the module-level kwargs
    /// path when vectorcall or interning isn't available), so this skips
    /// the separate `__olive_py_getattr` call `emit_py_method_call`'s
    /// general kwargs fallback still pays. Like that fallback, this never
    /// fuses the result -- only the no-kwargs arity shells do.
    fn emit_py_call_method_kw_v(
        &mut self,
        obj_op: Operand,
        attr: String,
        call_args: PyCallArgs,
        flavor: PyCallFlavor,
        span: Span,
    ) -> Operand {
        let PyCallArgs {
            pos_ops,
            kw_vals,
            kw_names_packed,
            arg_tags,
            coll_tags,
            kw_arg_tags,
            kw_coll_tags,
            ..
        } = call_args;

        let args_list = self.new_local(Type::List(Box::new(Type::Any)), None, true);
        self.push_statement(
            StatementKind::Assign(args_list, Rvalue::Aggregate(AggregateKind::List, pos_ops)),
            span,
        );
        let kwvals_list = self.new_local(Type::List(Box::new(Type::Any)), None, true);
        self.push_statement(
            StatementKind::Assign(kwvals_list, Rvalue::Aggregate(AggregateKind::List, kw_vals)),
            span,
        );
        let kwnames = kw_names_packed.unwrap_or_default();

        let name = match flavor {
            PyCallFlavor::Safe => "__olive_py_call_method_kw_v_safe",
            PyCallFlavor::Unsafe => "__olive_py_call_method_kw_v",
        };
        let mut call_operands = vec![
            obj_op,
            Operand::Constant(Constant::Str(attr)),
            Operand::Copy(args_list),
            Operand::Constant(Constant::Int(coll_tags)),
            Operand::Constant(Constant::Int(arg_tags)),
            Operand::Constant(Constant::Str(kwnames)),
            Operand::Copy(kwvals_list),
            Operand::Constant(Constant::Int(kw_coll_tags)),
            Operand::Constant(Constant::Int(kw_arg_tags)),
        ];
        // R17: see `emit_py_call_arity`'s equivalent trailing-arg fold-in.
        if matches!(flavor, PyCallFlavor::Unsafe) {
            call_operands.push(Operand::Constant(Constant::Str(self.call_loc_str(span))));
        }

        let result = self.new_local(Type::PyObject, None, true);
        self.push_statement(
            StatementKind::Assign(
                result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(name.to_string())),
                    args: call_operands,
                },
            ),
            span,
        );
        self.operand_for_local(result)
    }

    /// Emits a call through `__olive_py_call_method{0..4}(_safe)`: `obj` and
    /// `attr` go straight in as call registers alongside `pos_ops`, no
    /// `args_list` aggregate and no separate getattr call. Mirrors
    /// `emit_py_call_arity`'s shape and fusion rules exactly, with the
    /// receiver and attribute name as two extra leading operands.
    fn emit_py_call_method_arity(
        &mut self,
        obj_op: Operand,
        attr: String,
        call_args: PyCallArgs,
        flavor: PyCallFlavor,
        result_ty: Type,
        span: Span,
    ) -> Operand {
        let PyCallArgs {
            pos_ops,
            coll_tags,
            arg_tags,
            ..
        } = call_args;
        let name = match (pos_ops.len(), &flavor) {
            (0, PyCallFlavor::Unsafe) => "__olive_py_call_method0",
            (0, PyCallFlavor::Safe) => "__olive_py_call_method0_safe",
            (1, PyCallFlavor::Unsafe) => "__olive_py_call_method1",
            (1, PyCallFlavor::Safe) => "__olive_py_call_method1_safe",
            (2, PyCallFlavor::Unsafe) => "__olive_py_call_method2",
            (2, PyCallFlavor::Safe) => "__olive_py_call_method2_safe",
            (3, PyCallFlavor::Unsafe) => "__olive_py_call_method3",
            (3, PyCallFlavor::Safe) => "__olive_py_call_method3_safe",
            (4, PyCallFlavor::Unsafe) => "__olive_py_call_method4",
            (4, PyCallFlavor::Safe) => "__olive_py_call_method4_safe",
            (n, _) => unreachable!("emit_py_call_method_arity: arity {n} out of range"),
        };

        let (ret_tag, local_ty) = match flavor {
            PyCallFlavor::Unsafe => Self::py_ret_tag(&result_ty),
            PyCallFlavor::Safe => (RET_HANDLE, result_ty),
        };
        let tagged_arg_tags = arg_tags | (ret_tag << 60);

        let mut call_operands = vec![obj_op, Operand::Constant(Constant::Str(attr))];
        if pos_ops.is_empty() {
            call_operands.push(Operand::Constant(Constant::Int(tagged_arg_tags)));
        } else {
            call_operands.push(Operand::Constant(Constant::Int(coll_tags)));
            call_operands.push(Operand::Constant(Constant::Int(tagged_arg_tags)));
            call_operands.extend(pos_ops);
        }
        // R17: see `emit_py_call_arity`'s equivalent trailing-arg fold-in.
        if matches!(flavor, PyCallFlavor::Unsafe) {
            call_operands.push(Operand::Constant(Constant::Str(self.call_loc_str(span))));
        }

        let result = self.new_local(local_ty, None, true);
        self.push_statement(
            StatementKind::Assign(
                result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(name.to_string())),
                    args: call_operands,
                },
            ),
            span,
        );
        self.operand_for_local(result)
    }

    /// `pub(in crate::mir::builder)`, not `pub(super)`: `lower_stmt`'s
    /// statement-position call lowering (a sibling of `lower_expr`, not a
    /// descendant of it) needs this too, to route a discarded Python call
    /// through `lower_py_call_discard` instead of the ordinary expression path.
    pub(in crate::mir::builder) fn is_py_call(&self, expr: &Expr) -> bool {
        if let ExprKind::Call { callee, .. } = &expr.kind {
            let callee_ty = self.get_type(callee.id);
            if callee_ty.is_py_value() {
                return true;
            }
            if let ExprKind::Attr { obj, .. } = &callee.kind {
                return self.get_type(obj.id).is_py_value();
            }
        }
        false
    }

    /// Lowers a Python call in statement position whose result is never
    /// read: forces `RET_NONE` (`Type::Null`) regardless of the call's own
    /// declared return type, so the runtime decrefs the result immediately
    /// instead of building a handle this statement was always going to throw
    /// away. Only `is_py_call(expr)` shapes reach here (checked by the
    /// caller in `lower_stmt`); anything else keeps the args/kwargs list
    /// fallback exactly as before (unfused, same as any non-arity call).
    pub(in crate::mir::builder) fn lower_py_call_discard(&mut self, expr: &Expr) -> Operand {
        let ExprKind::Call { callee, args } = &expr.kind else {
            return self.lower_expr(expr);
        };

        let (arg_ops, arg_kw_names, _arg_tys) = self.lower_call_args(args, callee, expr.span);
        let call_args = self.build_py_call_args(args, arg_ops, arg_kw_names, expr.span);

        if let ExprKind::Attr { obj, attr } = &callee.kind
            && self.get_type(obj.id).is_py_value()
        {
            let obj_op = self.lower_expr_as_copy(obj);
            return self.emit_py_method_call(
                obj_op,
                attr.clone(),
                call_args,
                PyCallFlavor::Unsafe,
                Type::Null,
                expr.span,
            );
        }

        let callee_op = self.lower_expr_as_copy(callee);
        self.emit_py_call(
            callee_op,
            call_args,
            PyCallFlavor::Unsafe,
            Type::Null,
            expr.span,
        )
    }

    /// Tries every py-value expression shape R10-style scalar-hint fusion
    /// covers, in turn: a direct call (`lower_py_call_scalar_hint`), then a
    /// direct attribute read (`lower_py_getattr_scalar_hint` in `data.rs`).
    /// Called from the same four statement sites (`let`, plain assignment,
    /// explicit `return`, tail-expression return) that need a fusable-scalar
    /// hint from their own already-known target type. `None` from both
    /// means `expr`'s shape doesn't support fusion or `hint` isn't a
    /// concrete scalar; the caller falls through to plain `lower_expr` +
    /// `coerce`, unchanged from before either fusion existed.
    pub(in crate::mir::builder) fn lower_py_scalar_hint(
        &mut self,
        expr: &Expr,
        hint: &Type,
    ) -> Option<Operand> {
        self.lower_py_call_scalar_hint(expr, hint)
            .or_else(|| self.lower_py_getattr_scalar_hint(expr, hint))
    }

    /// Widens R10's scalar-return fusion beyond stub-typed calls, with no
    /// new pyffi syntax: when `expr` is directly a Python call the checker
    /// types as `PyObject` (no stub return, or a dynamically dispatched
    /// method -- there is no per-class/method stub syntax to give one a
    /// scalar type instead) but the immediate assignment context (a
    /// `let`/`return`/plain assignment) already declares a fusable scalar
    /// `hint`, lowers the call as if it had that scalar type instead of
    /// paying the wrap-then-coerce round trip. `None` falls back to the
    /// caller's ordinary `lower_expr` path unchanged: not a directly
    /// py-call-shaped `Call` node, already non-`PyObject` (already fused, or
    /// genuinely non-scalar), `hint` isn't one of R10's tags, or the call's
    /// own shape can't reach the arity-specialized entry points (5+
    /// positional args, a splat, or any keyword argument all keep the
    /// list-based path, which never fuses regardless of `result_ty`) --
    /// that last check runs before lowering anything, so no side effect
    /// from a call argument ever runs twice.
    pub(in crate::mir::builder) fn lower_py_call_scalar_hint(
        &mut self,
        expr: &Expr,
        hint: &Type,
    ) -> Option<Operand> {
        if self.get_type(expr.id) != Type::PyObject || Self::py_ret_tag(hint).0 == RET_HANDLE {
            return None;
        }
        let ExprKind::Call { callee, args } = &expr.kind else {
            return None;
        };
        let has_splat = args
            .iter()
            .any(|a| matches!(a, CallArg::Splat(_) | CallArg::KwSplat(_)));
        let has_kw = args.iter().any(|a| matches!(a, CallArg::Keyword(_, _)));
        if has_splat || has_kw || args.len() > 4 {
            return None;
        }

        let is_method = matches!(&callee.kind, ExprKind::Attr { obj, .. }
            if self.get_type(obj.id).is_py_value());
        if !is_method && !self.get_type(callee.id).is_py_value() {
            return None;
        }

        let (arg_ops, arg_kw_names, _arg_tys) = self.lower_call_args(args, callee, expr.span);
        let call_args = self.build_py_call_args(args, arg_ops, arg_kw_names, expr.span);

        if let ExprKind::Attr { obj, attr } = &callee.kind {
            let obj_op = self.lower_expr_as_copy(obj);
            return Some(self.emit_py_method_call(
                obj_op,
                attr.clone(),
                call_args,
                PyCallFlavor::Unsafe,
                hint.clone(),
                expr.span,
            ));
        }

        let callee_op = self.lower_expr_as_copy(callee);
        Some(self.emit_py_call(
            callee_op,
            call_args,
            PyCallFlavor::Unsafe,
            hint.clone(),
            expr.span,
        ))
    }

    pub(super) fn lower_py_call_safe(&mut self, expr: &Expr) -> Operand {
        let ExprKind::Call { callee, args } = &expr.kind else {
            return self.lower_expr(expr);
        };

        let mut arg_ops: Vec<Operand> = Vec::new();
        let mut arg_kw_names: Vec<Option<String>> = Vec::new();
        for arg in args {
            match arg {
                CallArg::Positional(e) | CallArg::Splat(e) | CallArg::KwSplat(e) => {
                    arg_ops.push(self.lower_expr(e));
                    arg_kw_names.push(None);
                }
                CallArg::Keyword(name, e) => {
                    arg_ops.push(self.lower_expr(e));
                    arg_kw_names.push(Some(name.clone()));
                }
            }
        }

        let call_args = self.build_py_call_args(args, arg_ops, arg_kw_names, expr.span);

        if let ExprKind::Attr { obj, attr } = &callee.kind {
            let obj_op = self.lower_expr_as_copy(obj);
            return self.emit_py_method_call(
                obj_op,
                attr.clone(),
                call_args,
                PyCallFlavor::Safe,
                Type::Any,
                expr.span,
            );
        }

        let func_op = self.lower_expr_as_copy(callee);
        self.emit_py_call(func_op, call_args, PyCallFlavor::Safe, Type::Any, expr.span)
    }
}

#[cfg(test)]
mod tests {
    use super::super::MirBuilder;
    use crate::lexer::Lexer;
    use crate::mir::ir::{AggregateKind, Constant, Operand, Rvalue, StatementKind};
    use crate::parser::Parser;
    use crate::semantic::{Resolver, TypeChecker};
    use rustc_hash::FxHashSet;

    fn build(src: &str) -> Vec<super::super::super::super::ir::MirFunction> {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        let mut tc = TypeChecker::new();
        tc.check_program(&prog);
        let mut builder = MirBuilder::new(
            &tc.expr_types,
            &tc.expr_kwarg_maps,
            &tc.type_env[0],
            tc.struct_fields.clone(),
            &tc.traits,
            FxHashSet::default(),
            tc.enum_defs.clone(),
        );
        builder.build_program(&prog);
        builder.functions
    }

    fn has_list_aggregate(f: &crate::mir::ir::MirFunction) -> bool {
        f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, Rvalue::Aggregate(AggregateKind::List, _))
                )
            })
        })
    }

    fn call_target(f: &crate::mir::ir::MirFunction) -> Option<String> {
        f.basic_blocks.iter().find_map(|bb| {
            bb.statements.iter().find_map(|s| match &s.kind {
                StatementKind::Assign(
                    _,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(name)),
                        ..
                    },
                ) if name.starts_with("__olive_py_call") => Some(name.clone()),
                _ => None,
            })
        })
    }

    fn has_set_loc_call(f: &crate::mir::ir::MirFunction) -> bool {
        f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(
                        _,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(name)),
                            ..
                        },
                    ) if name == "__olive_py_set_loc"
                )
            })
        })
    }

    /// A positional call with 0-4 args must emit no `[Any]` list aggregate
    /// at all -- the arguments go straight into the arity-specialized entry
    /// point's call registers. `math.f(...)`'s callee is `Attr{obj: math,
    /// attr: f}` with `math` itself a raw Python value, so this call shape
    /// dispatches through the fused method-call entry points (R9): fusion
    /// applies uniformly to any `obj.attr(...)` on a Python value, whether
    /// `obj` is a module or a class instance -- `PyObject_VectorcallMethod`'s
    /// own semantics (getattr the first arg, call the result with the rest)
    /// make this correct either way.
    #[test]
    fn arity_0_to_4_positional_calls_emit_no_list_aggregate() {
        let cases: &[(&str, &str)] = &[
            ("math.f()", "__olive_py_call_method0"),
            ("math.f(1)", "__olive_py_call_method1"),
            ("math.f(1, 2)", "__olive_py_call_method2"),
            ("math.f(1, 2, 3)", "__olive_py_call_method3"),
            ("math.f(1, 2, 3, 4)", "__olive_py_call_method4"),
        ];
        for (call, want_symbol) in cases {
            let src = format!("import py \"math\" as math\n\nfn f():\n    {call}\n\nf()\n");
            let fns = build(&src);
            let f = fns.iter().find(|f| f.name == "f").unwrap();
            assert!(
                !has_list_aggregate(f),
                "{call} unexpectedly built a list aggregate"
            );
            assert_eq!(
                call_target(f).as_deref(),
                Some(*want_symbol),
                "{call} dispatched to the wrong entry point"
            );
        }
    }

    /// Arity 5 and up has no fixed-register entry point, so it must keep
    /// building the `args_list` aggregate and calling the tagged list path.
    #[test]
    fn arity_5_positional_call_falls_back_to_list_path() {
        let src = "import py \"math\" as math\n\nfn f():\n    math.f(1, 2, 3, 4, 5)\n\nf()\n";
        let fns = build(src);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            has_list_aggregate(f),
            "arity-5 call must still build the args_list aggregate"
        );
        assert_eq!(call_target(f).as_deref(), Some("__olive_py_call_t"));
    }

    /// A call with any keyword argument keeps a list-based path regardless
    /// of its positional arity -- only the positional-only, kwargs-free
    /// shape skips list aggregates entirely. R15: on the tagged fast path
    /// this is the vectorcall entry point (still one `args_list` and one
    /// `kwvals_list` aggregate), not the dict-building `_kw_t` path.
    #[test]
    fn kwargs_call_over_the_arity_cap_keeps_the_list_path() {
        // positional + keyword = 5, over the arity-specialized shells' cap
        // of 4 -- must fall back to the list-based `_method_kw_v` entry.
        let src = "import py \"math\" as math\n\nfn f():\n    math.f(1, 2, 3, 4, x=5)\n\nf()\n";
        let fns = build(src);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            has_list_aggregate(f),
            "a kwargs call over the arity cap must still build a list aggregate"
        );
        assert_eq!(
            call_target(f).as_deref(),
            Some("__olive_py_call_method_kw_v")
        );
    }

    #[test]
    fn kwargs_call_within_the_arity_cap_skips_the_list_aggregate() {
        // positional + keyword = 2, within the arity-specialized shells'
        // cap -- routes to `__olive_py_call_method_kw_v_p1_k1`, no list.
        let src = "import py \"math\" as math\n\nfn f():\n    math.f(1, x=2)\n\nf()\n";
        let fns = build(src);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            !has_list_aggregate(f),
            "a kwargs call within the arity cap must not build a list aggregate"
        );
        assert_eq!(
            call_target(f).as_deref(),
            Some("__olive_py_call_method_kw_v_p1_k1")
        );
    }

    /// R17: every R7/R9/R15 fast-path shape (plain arity 0-4, fused method
    /// arity 0-4, R15's kwargs vectorcall entry point) folds the call-site
    /// location into its own trailing argument instead of emitting the
    /// separate `__olive_py_set_loc` statement pair the legacy path still
    /// pays -- this is the acceptance criterion's MIR half.
    #[test]
    fn fast_path_calls_emit_no_separate_set_loc_statement() {
        let cases: &[&str] = &[
            "math.f()",
            "math.f(1)",
            "math.f(1, 2)",
            "math.f(1, 2, 3)",
            "math.f(1, 2, 3, 4)",
            "math.f(1, x=2)",
        ];
        for call in cases {
            let src = format!("import py \"math\" as math\n\nfn f():\n    {call}\n\nf()\n");
            let fns = build(&src);
            let f = fns.iter().find(|f| f.name == "f").unwrap();
            assert!(
                !has_set_loc_call(f),
                "{call} unexpectedly emitted a separate __olive_py_set_loc call"
            );
        }
    }

    /// The legacy list-based path (arity 5+, no fixed-register entry point)
    /// is out of R17's scope and must keep the separate `__olive_py_set_loc`
    /// statement exactly as before.
    #[test]
    fn legacy_path_call_still_emits_set_loc_statement() {
        let src = "import py \"math\" as math\n\nfn f():\n    math.f(1, 2, 3, 4, 5)\n\nf()\n";
        let fns = build(src);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            has_set_loc_call(f),
            "arity-5 legacy call must still emit __olive_py_set_loc"
        );
    }

    fn has_call_to(f: &crate::mir::ir::MirFunction, target: &str) -> bool {
        f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(
                        _,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(name)),
                            ..
                        },
                    ) if name == target
                )
            })
        })
    }

    /// R19: a function-typed argument passed straight to a py-call (the
    /// tagged fast path, no kwargs, arity <= 4 -- which never runs
    /// `emit_to_py_arg`/`coerce`) must still route through
    /// `__olive_py_make_callable`, not reach the call as a raw,
    /// mistagged closure-record pointer.
    #[test]
    fn fn_typed_fast_path_arg_becomes_callable() {
        let src = "import py \"math\" as math\n\nfn cb(x: int) -> int:\n    return x\n\nfn f():\n    math.apply(cb)\n\nf()\n";
        let fns = build(src);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            has_call_to(f, "__olive_py_make_callable"),
            "a Type::Fn fast-path argument must be converted via __olive_py_make_callable"
        );
        assert_eq!(
            call_target(f).as_deref(),
            Some("__olive_py_call_method1"),
            "the callable conversion must still leave the call on the arity-1 fast path"
        );
    }

    /// R19: a function-typed keyword argument (the shape `sorted(xs,
    /// key=f)`-into-Python actually needs) goes through the kwargs
    /// vectorcall path, which also bypasses `emit_to_py_arg` -- same fix,
    /// different call shape.
    #[test]
    fn fn_typed_kwarg_becomes_callable() {
        let src = "import py \"math\" as math\n\nfn cb(x: int) -> int:\n    return x\n\nfn f():\n    math.apply(key=cb)\n\nf()\n";
        let fns = build(src);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            has_call_to(f, "__olive_py_make_callable"),
            "a Type::Fn keyword argument must be converted via __olive_py_make_callable"
        );
    }

    /// R19: `let cb: PyObject = my_fn` (no py-call in sight, a general
    /// assignment) routes through `coerce`'s new `Type::Fn` arm, the other
    /// integration point besides the py-call-argument one above.
    #[test]
    fn fn_assigned_to_pyobject_let_becomes_callable() {
        let src = "fn cb(x: int) -> int:\n    return x\n\nfn f():\n    let handle: PyObject = cb\n\nf()\n";
        let fns = build(src);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            has_call_to(f, "__olive_py_make_callable"),
            "assigning a Type::Fn value to a PyObject-typed let must convert via __olive_py_make_callable"
        );
    }

    /// GetAttr analogue of the call-scalar-fusion tests above: a `let`
    /// annotation supplies the concrete scalar hint `math.value`'s own
    /// checker type (bare `PyObject`, no stub) can't provide on its own.
    #[test]
    fn attr_read_with_let_annotation_fuses_to_getattr_ret() {
        let src = "import py \"math\" as math\n\nfn f():\n    let v: int = math.value\n\nf()\n";
        let fns = build(src);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            has_call_to(f, "__olive_py_getattr_ret"),
            "a scalar-annotated let over a dynamic attribute read must fuse to __olive_py_getattr_ret"
        );
        assert!(
            !has_call_to(f, "__olive_py_getattr"),
            "a fused attribute read must not also emit the plain unfused getattr"
        );
    }

    /// Same fusion, reached through a plain reassignment instead of a
    /// `let`: the target's already-declared type is the hint.
    #[test]
    fn attr_read_reassigned_into_typed_local_fuses_to_getattr_ret() {
        let src = "import py \"math\" as math\n\nfn f():\n    let mut last = 0\n    last = math.value\n\nf()\n";
        let fns = build(src);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            has_call_to(f, "__olive_py_getattr_ret"),
            "reassigning a dynamic attribute read into an already-int local must fuse"
        );
    }

    /// No fusable hint (unannotated `let`, checker types the local itself
    /// as `PyObject`) must keep the plain, unfused `olive_py_getattr` path.
    #[test]
    fn attr_read_without_scalar_hint_stays_unfused() {
        let src = "import py \"math\" as math\n\nfn f():\n    let v = math.value\n\nf()\n";
        let fns = build(src);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            has_call_to(f, "__olive_py_getattr"),
            "an unannotated attribute read must stay on the plain getattr path"
        );
        assert!(
            !has_call_to(f, "__olive_py_getattr_ret"),
            "an unannotated attribute read has no hint to fuse with"
        );
    }
}
