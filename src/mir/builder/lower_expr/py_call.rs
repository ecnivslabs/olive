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
    arg_tags: i64,
    coll_tags: i64,
    kw_arg_tags: i64,
    kw_coll_tags: i64,
    fast_path: bool,
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
        // kwargs call keep the list-based path below.
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
        } else {
            let kwargs_list = self.new_local(Type::List(Box::new(Type::Any)), None, true);
            self.push_statement(
                StatementKind::Assign(kwargs_list, Rvalue::Aggregate(AggregateKind::List, kw_ops)),
                span,
            );
            call_operands.push(Operand::Copy(kwargs_list));
            call_operands.push(Operand::Constant(Constant::Int(kw_coll_tags)));
            if fast_path {
                call_operands.push(Operand::Constant(Constant::Int(kw_arg_tags)));
            }
            match (fast_path, &flavor) {
                (true, PyCallFlavor::Safe) => "__olive_py_call_kw_t_safe",
                (true, PyCallFlavor::Unsafe) => "__olive_py_call_kw_t",
                (false, PyCallFlavor::Safe) => "__olive_py_call_kw_safe",
                (false, PyCallFlavor::Unsafe) => "__olive_py_call_kw",
            }
        };

        let result = self.new_local(result_ty, None, true);
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
    /// list allocation for this call site at all. `olive_py_call0` takes no
    /// tag words (there is nothing to tag); every other arity takes the same
    /// `coll_tags`/`arg_tags` pair `olive_py_call_t` does.
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

        let mut call_operands = vec![callee_op];
        if !pos_ops.is_empty() {
            call_operands.push(Operand::Constant(Constant::Int(coll_tags)));
            call_operands.push(Operand::Constant(Constant::Int(arg_tags)));
            call_operands.extend(pos_ops);
        }

        let result = self.new_local(result_ty, None, true);
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

    pub(super) fn is_py_call(&self, expr: &Expr) -> bool {
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

        let func_op = if let ExprKind::Attr { obj, attr } = &callee.kind {
            let obj_op = self.lower_expr_as_copy(obj);
            let attr_local = self.new_local(Type::PyObject, None, true);
            self.push_statement(
                StatementKind::Assign(
                    attr_local,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_py_getattr".to_string(),
                        )),
                        args: vec![obj_op, Operand::Constant(Constant::Str(attr.clone()))],
                    },
                ),
                expr.span,
            );
            self.operand_for_local(attr_local)
        } else {
            self.lower_expr_as_copy(callee)
        };

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

    /// A positional call with 0-4 args must emit no `[Any]` list aggregate
    /// at all -- the arguments go straight into the arity-specialized entry
    /// point's call registers.
    #[test]
    fn arity_0_to_4_positional_calls_emit_no_list_aggregate() {
        let cases: &[(&str, &str)] = &[
            ("math.f()", "__olive_py_call0"),
            ("math.f(1)", "__olive_py_call1"),
            ("math.f(1, 2)", "__olive_py_call2"),
            ("math.f(1, 2, 3)", "__olive_py_call3"),
            ("math.f(1, 2, 3, 4)", "__olive_py_call4"),
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

    /// A call with any keyword argument keeps the kwargs list path
    /// regardless of its positional arity -- only the positional-only,
    /// kwargs-free shape gets specialized.
    #[test]
    fn kwargs_call_keeps_list_path_regardless_of_positional_arity() {
        let src = "import py \"math\" as math\n\nfn f():\n    math.f(1, x=2)\n\nf()\n";
        let fns = build(src);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            has_list_aggregate(f),
            "a kwargs call must still build a list aggregate"
        );
        assert_eq!(call_target(f).as_deref(), Some("__olive_py_call_kw_t"));
    }
}
