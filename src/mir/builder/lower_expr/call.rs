use super::super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{CallArg, Expr};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    fn write_func_name(ty: &Type) -> &'static str {
        // Tag-encoded unions decode via the Any writer.
        if ty.is_tag_encoded_union() {
            return "__olive_write_any";
        }
        let ty = crate::semantic::type_descriptor::concrete_ty(ty);
        match ty {
            Type::Str | Type::Null => "__olive_write_str",
            Type::Bool => "__olive_write_bool",
            Type::Int
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::Usize
            | Type::IntegerLiteral(_) => "__olive_write_int",
            // Only U64 needs a distinct unsigned formatter: the other
            // narrower unsigned types never set bit 63 once zero-extended
            // into the 64-bit register, so signed decimal printing is
            // already correct for them.
            Type::U64 => "__olive_write_u64",
            Type::Float | Type::F32 | Type::FloatLiteral(_) => "__olive_write_float",
            _ => "__olive_write_any",
        }
    }

    pub(super) fn lower_call_args(
        &mut self,
        args: &[CallArg],
        _callee: &Expr,
        _span: Span,
    ) -> (Vec<Operand>, Vec<Option<String>>, Vec<Type>) {
        let mut arg_ops = Vec::new();
        let mut arg_kw_names: Vec<Option<String>> = Vec::new();
        let mut arg_tys: Vec<Type> = Vec::new();
        for arg in args {
            match arg {
                CallArg::Splat(e) if self.get_type(e.id) == crate::semantic::types::Type::Int => {
                    let ptr_op = self.lower_expr(e);
                    let tmp = self.new_local(crate::semantic::types::Type::Int, None, false);
                    self.push_statement(
                        StatementKind::Assign(tmp, Rvalue::PtrLoad(ptr_op)),
                        e.span,
                    );
                    arg_ops.push(Operand::Copy(tmp));
                    arg_kw_names.push(None);
                    arg_tys.push(crate::semantic::types::Type::Int);
                }
                CallArg::Positional(e) | CallArg::Splat(e) | CallArg::KwSplat(e) => {
                    let arg_ty = self.get_type(e.id);
                    // Arguments are borrows: the caller keeps ownership and
                    // frees the value when its own scope ends.
                    arg_ops.push(self.lower_expr_as_copy(e));
                    arg_kw_names.push(None);
                    arg_tys.push(arg_ty);
                }
                CallArg::Keyword(name, e) => {
                    let arg_ty = self.get_type(e.id);
                    arg_ops.push(self.lower_expr_as_copy(e));
                    arg_kw_names.push(Some(name.clone()));
                    arg_tys.push(arg_ty);
                }
            }
        }
        (arg_ops, arg_kw_names, arg_tys)
    }

    pub(super) fn lower_pyobject_call(
        &mut self,
        callee_op: Operand,
        args: &[CallArg],
        arg_ops: Vec<Operand>,
        arg_kw_names: Vec<Option<String>>,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let call_args = self.build_py_call_args(args, arg_ops, arg_kw_names, span);
        // See the identical comment in `call_method.rs`'s `is_py_value()`
        // branch: the real declared type lets `emit_py_call` fuse a scalar
        // result, with the caller's later `coerce_pyobj_if_needed` becoming a
        // harmless identity cast in that case.
        self.emit_py_call(
            callee_op,
            call_args,
            super::py_call::PyCallFlavor::Unsafe,
            self.get_type(expr_id),
            span,
        )
    }

    pub(super) fn lower_type_builtin(&mut self, args: &[CallArg], span: Span) -> Option<Operand> {
        if args.is_empty() {
            return None;
        }
        let arg_expr = match &args[0] {
            CallArg::Positional(e)
            | CallArg::Keyword(_, e)
            | CallArg::Splat(e)
            | CallArg::KwSplat(e) => e,
        };
        let mut arg_ty = self.get_type(arg_expr.id);
        while let Type::Ref(inner) | Type::MutRef(inner) = arg_ty {
            arg_ty = *inner;
        }

        // A concrete static type gives a constant name; `Any`/Python is read at
        // runtime.
        let name = match arg_ty {
            Type::Int
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize
            | Type::IntegerLiteral(_) => "int",
            Type::Float | Type::F32 | Type::FloatLiteral(_) => "float",
            Type::Bool => "bool",
            Type::Str => "str",
            Type::Bytes => "bytes",
            Type::List(_) | Type::Tuple(_) => "list",
            Type::Dict(_, _) => "dict",
            Type::Set(_) => "set",
            Type::Null => "None",
            Type::Enum(_, _) => "enum",
            Type::Any | Type::PyObject | Type::PyNamed(_, _) | Type::Var(_) => {
                let arg = self.lower_expr(arg_expr);
                let result = self.new_local(Type::Str, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        result,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_typeof_str".to_string(),
                            )),
                            args: vec![arg],
                        },
                    ),
                    span,
                );
                return Some(self.operand_for_local(result));
            }
            other => return Some(Operand::Constant(Constant::Str(format!("{other}")))),
        };
        Some(Operand::Constant(Constant::Str(name.to_string())))
    }

    pub(super) fn lower_len_builtin(&mut self, args: &[CallArg], span: Span) -> Option<Operand> {
        if args.is_empty() {
            return None;
        }
        let arg_expr = match &args[0] {
            CallArg::Positional(e)
            | CallArg::Keyword(_, e)
            | CallArg::Splat(e)
            | CallArg::KwSplat(e) => e,
        };
        let arg_ty = self.get_type(arg_expr.id);
        let mut current_arg_ty = arg_ty;
        while let Type::Ref(inner) | Type::MutRef(inner) = current_arg_ty {
            current_arg_ty = *inner;
        }

        let func_name = if current_arg_ty == Type::Str {
            "__olive_str_len"
        } else if matches!(current_arg_ty, Type::Dict(_, _)) {
            // A dict is an object (key map), not a contiguous vector, so it has
            // its own length function.
            "__olive_obj_len"
        } else if matches!(
            current_arg_ty,
            Type::List(_) | Type::Tuple(_) | Type::Set(_)
        ) {
            "__olive_list_len"
        } else if current_arg_ty == Type::Any {
            // The concrete kind isn't known until runtime (str/list/dict/bytes/
            // pyobject all reach here), so dispatch dynamically instead of
            // assuming a list layout -- see olive_len_any.
            "__olive_len_any"
        } else if current_arg_ty == Type::Bytes {
            "__olive_buf_len"
        } else if current_arg_ty.is_py_value() {
            "__olive_py_len"
        } else {
            return None;
        };

        let arg_op = self.lower_expr_as_copy(arg_expr);
        let tmp = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(func_name.to_string())),
                    args: vec![arg_op],
                },
            ),
            span,
        );
        Some(self.operand_for_local(tmp))
    }

    /// Single-argument `sum`/`min`/`max` over a collection whose substituted
    /// element type is concrete but non-numeric. The checker gate passes
    /// unresolved params, so a monomorphized instance (e.g. `[S]`) can reach
    /// lowering, where import-time dispatch would read struct words with the
    /// integer reducers (a segfault once the garbage is used as a struct).
    /// Fully dynamic shapes (`Param`/`Var`/`Any` remnants) stay on the legacy
    /// path; only shapes the checker would reject get the fault, with its
    /// exact message.
    pub(super) fn lower_checked_numeric_builtin(
        &mut self,
        name: &str,
        args: &[CallArg],
        arg_tys: &[Type],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        if !matches!(name, "sum" | "min" | "max") || args.len() != 1 {
            return None;
        }
        // A user-defined function shadowing the builtin (nested fn, lambda,
        // global, or generic) takes the normal call path: the checker's own
        // gate applies the same exclusion via `lookup_type`.
        if self.is_user_callable(name) {
            return None;
        }
        let CallArg::Positional(arg) = &args[0] else {
            return None;
        };
        let sub = self.subst_mono_type(arg_tys.first()?);
        let mut current = sub;
        while let Type::Ref(inner) | Type::MutRef(inner) = current {
            current = *inner;
        }
        // Mirror the checker's `numeric`: `None` marks a still-dynamic leaf
        // the checker owns (unresolved param/var, `Any`).
        fn leaf(ty: &Type) -> Option<bool> {
            match ty {
                Type::Int
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Usize
                | Type::IntegerLiteral(_)
                | Type::Float
                | Type::F32
                | Type::FloatLiteral(_)
                | Type::Bool => Some(true),
                Type::Var(_) | Type::Param(_) | Type::Any => None,
                _ => Some(false),
            }
        }
        // `None` = dynamic (legacy path), `Some(true)` = numeric (normal
        // path), `Some(false)` = concrete but rejected (fault).
        fn judge(ty: &Type) -> Option<bool> {
            match ty {
                Type::List(e) | Type::Set(e) => leaf(e),
                Type::Tuple(members) => {
                    let mut ok = true;
                    for m in members {
                        match leaf(m) {
                            None => return None,
                            Some(false) => ok = false,
                            Some(true) => {}
                        }
                    }
                    Some(ok)
                }
                _ => leaf(ty).and(Some(false)),
            }
        }
        // `T | None` judges by `T`, like codegen; wider unions narrow first.
        // A dynamic member keeps the legacy path.
        let verdict = match &current {
            Type::Union(members) => {
                let non_null: Vec<&Type> = members
                    .iter()
                    .filter(|m| !matches!(m, Type::Null))
                    .collect();
                match non_null.as_slice() {
                    [single] => match single {
                        Type::List(e) | Type::Set(e) => leaf(e),
                        Type::Tuple(ms) => {
                            let mut ok = true;
                            for m in ms {
                                match leaf(m) {
                                    None => return None,
                                    Some(false) => ok = false,
                                    Some(true) => {}
                                }
                            }
                            Some(ok)
                        }
                        _ => Some(false),
                    },
                    _ => {
                        return self.lower_union_narrow_fault(span, expr_id);
                    }
                }
            }
            other => judge(other),
        };
        if verdict != Some(false) {
            return None;
        }
        // Evaluate the argument first so its side effects precede the fault,
        // matching normal call evaluation order.
        let _ = self.lower_expr_as_copy(arg);
        let ret_ty = self.subst_mono_type(&self.get_type(expr_id));
        let tmp = self.new_local(ret_ty, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_panic".to_string())),
                    args: vec![Operand::Constant(Constant::Str(format!(
                        "`{name}` requires a list, tuple, or set of numbers, got `{current}`"
                    )))],
                },
            ),
            span,
        );
        Some(self.operand_for_local(tmp))
    }

    /// Fault for a use of a union value that must narrow first. Generic
    /// bodies skip that gate on unresolved params; the message mirrors the
    /// checker's.
    fn lower_union_narrow_fault(&mut self, span: Span, expr_id: usize) -> Option<Operand> {
        let ret_ty = self.subst_mono_type(&self.get_type(expr_id));
        let tmp = self.new_local(ret_ty, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_panic".to_string())),
                    args: vec![Operand::Constant(Constant::Str(
                        "cannot use union type in this operation, narrow the union first"
                            .to_string(),
                    ))],
                },
            ),
            span,
        );
        Some(self.operand_for_local(tmp))
    }

    pub(super) fn lower_maxmin_builtin(
        &mut self,
        name: &str,
        args: &[CallArg],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        if args.len() != 2 {
            return None;
        }
        let a_expr = match &args[0] {
            CallArg::Positional(e)
            | CallArg::Keyword(_, e)
            | CallArg::Splat(e)
            | CallArg::KwSplat(e) => e,
        };
        let b_expr = match &args[1] {
            CallArg::Positional(e)
            | CallArg::Keyword(_, e)
            | CallArg::Splat(e)
            | CallArg::KwSplat(e) => e,
        };

        let a_op = self.lower_expr_as_copy(a_expr);
        let b_op = self.lower_expr_as_copy(b_expr);
        let result_ty = self.get_type(expr_id);

        // Two-argument `min`/`max` lower to a raw comparison, so aggregate
        // operands would compare pointers. The checker rejects them for
        // concrete shapes; generic bodies skip that gate, so a bad
        // instantiation faults here with the checker's message instead of
        // silently comparing addresses. User functions shadowing the name
        // keep today's path exactly.
        if !self.is_user_callable(name) {
            let ta = self.subst_mono_type(&self.get_type(a_expr.id));
            let tb = self.subst_mono_type(&self.get_type(b_expr.id));
            fn blocked(ty: &Type) -> Option<bool> {
                match ty {
                    Type::List(_)
                    | Type::Tuple(_)
                    | Type::Set(_)
                    | Type::Dict(..)
                    | Type::Bytes
                    | Type::Struct(..)
                    | Type::Enum(..)
                    | Type::TraitObject(..)
                    | Type::Fn(..)
                    | Type::Future(..)
                    | Type::Null => Some(true),
                    Type::Var(_) | Type::Param(_) | Type::Any => None,
                    _ => Some(false),
                }
            }
            let dynamic = [blocked(&ta), blocked(&tb)].iter().any(|b| b.is_none());
            if !dynamic {
                let bad = matches!(&ta, Type::Union(_)) || matches!(&tb, Type::Union(_));
                if bad {
                    return self.lower_union_narrow_fault(span, expr_id);
                }
                if blocked(&ta) == Some(true) || blocked(&tb) == Some(true) {
                    let ret_ty = self.subst_mono_type(&result_ty);
                    let tmp = self.new_local(ret_ty, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_panic".to_string(),
                                )),
                                args: vec![Operand::Constant(Constant::Str(format!(
                                    "`{name}` requires two comparable numbers or strings"
                                )))],
                            },
                        ),
                        span,
                    );
                    return Some(self.operand_for_local(tmp));
                }
            }
        }

        let cmp_op = if name == "max" {
            crate::parser::BinOp::Gt
        } else {
            crate::parser::BinOp::Lt
        };
        let cond_local = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(
                cond_local,
                Rvalue::BinaryOp(cmp_op, a_op.clone(), b_op.clone()),
            ),
            span,
        );

        let result_local = self.new_local(result_ty.clone(), None, false);
        let true_bb = self.new_block();
        let false_bb = self.new_block();
        let exit_bb = self.new_block();

        if let Some(cur) = self.current_block {
            self.terminate_block(
                cur,
                TerminatorKind::SwitchInt {
                    discr: Operand::Copy(cond_local),
                    targets: vec![(1, true_bb)],
                    otherwise: false_bb,
                },
                span,
            );
        }

        self.current_block = Some(true_bb);
        self.push_statement(StatementKind::Assign(result_local, Rvalue::Use(a_op)), span);
        self.terminate_block(true_bb, TerminatorKind::Goto { target: exit_bb }, span);

        self.current_block = Some(false_bb);
        let b_ty = self.get_type(b_expr.id);
        let b_op = if result_ty.is_py_value() && !b_ty.is_py_value() {
            self.emit_to_py_arg(b_op, &b_ty, span)
        } else {
            b_op
        };
        self.push_statement(StatementKind::Assign(result_local, Rvalue::Use(b_op)), span);
        self.terminate_block(false_bb, TerminatorKind::Goto { target: exit_bb }, span);

        self.current_block = Some(exit_bb);
        Some(self.operand_for_local(result_local))
    }

    pub(super) fn lower_enum_variant_call(
        &mut self,
        name: &str,
        arg_ops: Vec<Operand>,
        span: Span,
        _expr_id: usize,
    ) -> Option<Operand> {
        if let Some((enum_name, tag)) = self.enum_variants.get(name).cloned() {
            let type_id = crate::mir::enum_type_id(&enum_name);
            let enum_ty = self.get_type(_expr_id);
            let desc = self.enum_variant_desc(&enum_ty);
            let tmp = self.new_local(enum_ty, None, false);
            // Payload slots hold canonical float bits (see
            // `coerce_float_slot`); readers reinterpret the word.
            let param_tys = self
                .global_types
                .get(name)
                .and_then(|ty| match ty {
                    Type::Fn(pts, _, _) => Some(pts.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let arg_ops = arg_ops
                .into_iter()
                .enumerate()
                .map(|(i, op)| match param_tys.get(i) {
                    Some(pt) => {
                        let from_ty = self.operand_static_ty(&op);
                        self.coerce_float_slot(op, &from_ty, pt, span)
                    }
                    None => op,
                })
                .collect::<Vec<_>>();
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Aggregate(AggregateKind::EnumVariant(type_id, tag, desc), arg_ops),
                ),
                span,
            );
            Some(self.operand_for_local(tmp))
        } else {
            None
        }
    }

    pub(super) fn lower_list_new_builtin(
        &mut self,
        args: &[CallArg],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        if args.is_empty() {
            return None;
        }
        let arg_expr = match &args[0] {
            CallArg::Positional(e)
            | CallArg::Keyword(_, e)
            | CallArg::Splat(e)
            | CallArg::KwSplat(e) => e,
        };
        let arg_op = self.lower_expr(arg_expr);
        let result_ty = self.get_type(expr_id);
        let tmp = self.new_local(result_ty, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_new".to_string())),
                    args: vec![arg_op],
                },
            ),
            span,
        );
        Some(self.operand_for_local(tmp))
    }

    pub(super) fn lower_bytes_builtin(
        &mut self,
        name: &str,
        args: &[CallArg],
        span: Span,
    ) -> Option<Operand> {
        let bytes_builtin = match name {
            "bytes_new" => Some(("__olive_buf_new_zeroed", Type::Bytes)),
            "bytes_push" => Some(("__olive_buf_push", Type::Null)),
            "bytes_push_u16_le" => Some(("__olive_buf_push_u16_le", Type::Null)),
            "bytes_push_u32_le" => Some(("__olive_buf_push_u32_le", Type::Null)),
            _ => None,
        };
        if let Some((runtime_name, ret_ty)) = bytes_builtin {
            let arg_ops: Vec<Operand> = args
                .iter()
                .map(|a| match a {
                    CallArg::Positional(e)
                    | CallArg::Keyword(_, e)
                    | CallArg::Splat(e)
                    | CallArg::KwSplat(e) => self.lower_expr_as_copy(e),
                })
                .collect();
            let tmp = self.new_local(ret_ty, None, false);
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(runtime_name.to_string())),
                        args: arg_ops,
                    },
                ),
                span,
            );
            Some(self.operand_for_local(tmp))
        } else {
            None
        }
    }

    pub(super) fn lower_print_builtin(
        &mut self,
        _callee: &Expr,
        args: &[CallArg],
        arg_ops: &[Operand],
        arg_tys: &[Type],
        span: Span,
        _expr_id: usize,
    ) -> Operand {
        if args.is_empty() {
            let nl = self.new_local(Type::Int, None, false);
            self.push_statement(
                StatementKind::Assign(
                    nl,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__olive_write_nl".to_string())),
                        args: vec![],
                    },
                ),
                span,
            );
            let ret = self.new_local(Type::Int, None, false);
            self.push_statement(
                StatementKind::Assign(ret, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
                span,
            );
            return self.operand_for_local(ret);
        }
        for (i, arg_op) in arg_ops.iter().enumerate() {
            if i > 0 {
                let space = self.new_local(Type::Int, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        space,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_write_char".to_string(),
                            )),
                            args: vec![Operand::Constant(Constant::Int(32))],
                        },
                    ),
                    span,
                );
            }
            let arg_ty = arg_tys.get(i).cloned().unwrap_or(Type::Any);
            let (write_arg, func_name) =
                match self.lower_struct_str_call(arg_op.clone(), &arg_ty, span) {
                    Some(str_op) => (str_op, "__olive_write_str"),
                    None => (arg_op.clone(), Self::write_func_name(&arg_ty)),
                };
            let write = self.new_local(Type::Int, None, false);
            self.push_statement(
                StatementKind::Assign(
                    write,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(func_name.to_string())),
                        args: vec![write_arg],
                    },
                ),
                span,
            );
        }
        let nl = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                nl,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_write_nl".to_string())),
                    args: vec![],
                },
            ),
            span,
        );
        let ret = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(ret, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            span,
        );
        self.operand_for_local(ret)
    }

    /// `sorted`/`reversed`/`any`/`all`: bare-function sequence builtins
    /// dispatched by the argument's element type. `sorted`/`reversed`
    /// deep-copy heap-owning elements first (rule 3) so the source list is
    /// untouched, then run the existing in-place list op on the copy.
    pub(super) fn lower_sequence_builtin(
        &mut self,
        name: &str,
        args: &[CallArg],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        let key_arg_expr = if name == "sorted" && args.len() == 2 {
            match &args[1] {
                CallArg::Keyword(kw, e) if kw == "key" => Some(e),
                _ => None,
            }
        } else {
            None
        };
        if !matches!(name, "sorted" | "reversed" | "any" | "all")
            || (args.len() != 1 && key_arg_expr.is_none())
        {
            return None;
        }
        let arg_expr = match &args[0] {
            CallArg::Positional(e)
            | CallArg::Keyword(_, e)
            | CallArg::Splat(e)
            | CallArg::KwSplat(e) => e,
        };
        let recv_ty = self.get_type(arg_expr.id);
        let mut current = &recv_ty;
        while let Type::Ref(inner) | Type::MutRef(inner) = current {
            current = inner;
        }
        let elem_ty: Type = match current {
            Type::List(e) => (**e).clone(),
            _ => Type::Any,
        };
        let list_op = self.lower_expr_as_copy(arg_expr);

        if matches!(name, "any" | "all") {
            let runtime = match (name, &elem_ty) {
                ("any", Type::Bool) => "__olive_list_any_bool",
                ("all", Type::Bool) => "__olive_list_all_bool",
                ("any", _) => "__olive_list_any_any",
                ("all", _) => "__olive_list_all_any",
                _ => unreachable!(),
            };
            let tmp = self.new_local(Type::Bool, None, false);
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(runtime.to_string())),
                        args: vec![list_op],
                    },
                ),
                span,
            );
            return Some(self.operand_for_local(tmp));
        }

        let needs_copy = Self::list_elem_needs_copy(&elem_ty);
        let copy_fn = if needs_copy {
            "__olive_list_getslice_typed"
        } else {
            "__olive_list_getslice"
        };
        let zero = Operand::Constant(Constant::Int(0));
        let copy_local = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(
                copy_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(copy_fn.to_string())),
                    args: vec![list_op, zero.clone(), zero.clone(), zero.clone(), zero],
                },
            ),
            span,
        );
        let copy_op = self.operand_for_local(copy_local);

        if let Some(key_expr) = key_arg_expr {
            let key_ret_ty = match self.get_type(key_expr.id) {
                Type::Fn(_, ret, _) => *ret,
                _ => Type::Int,
            };
            let key_op = self.lower_expr(key_expr);
            return Some(self.lower_sort_by_key(copy_op, &elem_ty, key_op, &key_ret_ty, span));
        }

        // E6.3: `sorted(xs)` with no key on a struct element list uses the
        // struct's own `__lt__` (checker already required it).
        if name == "sorted"
            && let Type::Struct(struct_name, ..) = &elem_ty
            && self.fn_meta.contains_key(&format!("{struct_name}::__lt__"))
        {
            return Some(self.lower_sort_by_lt(copy_op, &elem_ty, span));
        }
        // Without `__lt__` the checker rejects concrete shapes; a generic
        // body skips that gate, so a bad instantiation faults here instead
        // of int-sorting struct pointers into a silently wrong order. User
        // functions shadowing the builtin keep today's path exactly.
        if name == "sorted"
            && key_arg_expr.is_none()
            && !self.is_user_callable(name)
            && let Type::Struct(struct_name, ..) = &elem_ty
        {
            let ret_ty = self.subst_mono_type(&self.get_type(expr_id));
            return Some(self.missing_dunder_fault(struct_name, "__lt__", ret_ty, span));
        }

        let apply_fn = if name == "sorted" {
            match &elem_ty {
                Type::Float => "__olive_list_sort_float",
                Type::F32 => "__olive_list_sort_f32",
                Type::Str => "__olive_list_sort_str",
                Type::Any | Type::Var(_) | Type::Param(_) => "__olive_list_sort_any",
                _ => "__olive_list_sort_int",
            }
        } else {
            "__olive_list_reverse"
        };
        let void_local = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                void_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(apply_fn.to_string())),
                    args: vec![copy_op.clone()],
                },
            ),
            span,
        );
        Some(copy_op)
    }
}
