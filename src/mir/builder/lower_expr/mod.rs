mod call;
mod call_method;
mod control;
mod data;
mod literals;
mod ops;

use super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{CallArg, Expr, ExprKind};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    /// Boxes a scalar into an `Any` container slot. Only container elements box
    /// (a raw slot can't tell float-bits/null from an int); scalar `Any`
    /// variables/args/returns stay raw, so this is the sole boxing site.
    pub(super) fn coerce_to_elem(&mut self, op: Operand, elem: &Expr, elem_ty: &Type) -> Operand {
        // A trait-object element holds a fat pointer (data + vtable), so a
        // concrete struct stored into one must be widened the same way an
        // argument or assignment is.
        if let Type::TraitObject(_, _) = elem_ty {
            let from_ty = self.get_type(elem.id);
            return self.coerce(op, &from_ty, elem_ty, elem.span);
        }
        if *elem_ty != Type::Any {
            return op;
        }
        let from_ty = self.get_type(elem.id);
        self.box_into_any(op, &from_ty, elem.span)
    }

    /// Boxes a float, bool, or null into its `Any` heap form; passes anything
    /// else through.
    pub(super) fn box_into_any(&mut self, op: Operand, from_ty: &Type, span: Span) -> Operand {
        let boxer = match from_ty {
            Type::Float | Type::F32 => Some(("__olive_box_float", true)),
            Type::Bool => Some(("__olive_box_bool", true)),
            Type::Null => Some(("__olive_box_null", false)),
            _ => return op,
        };
        let (boxer, takes_arg) = boxer.unwrap();
        let tmp = self.new_local(Type::Any, None, false);
        let args = if takes_arg { vec![op] } else { vec![] };
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(boxer.to_string())),
                    args,
                },
            ),
            span,
        );
        Operand::Copy(tmp)
    }

    pub(super) fn coerce(
        &mut self,
        op: Operand,
        from_ty: &Type,
        to_ty: &Type,
        span: Span,
    ) -> Operand {
        if let Type::TraitObject(trait_name, _) = to_ty
            && let Type::Struct(struct_name, _) = from_ty
        {
            let vtable_name = format!("__vtable_{}_{}", trait_name, struct_name);
            if !self.vtables.contains_key(&vtable_name)
                && let Some(trait_def) = self.traits.get(trait_name)
            {
                let mut method_names = Vec::new();
                for (method_name, _) in &trait_def.methods {
                    let mangled = format!("{}::{}", struct_name, method_name);
                    if let Type::Struct(_, type_args) = from_ty
                        && !type_args.is_empty()
                    {
                        method_names.push(self.monomorphize(&mangled, type_args));
                        continue;
                    }
                    method_names.push(mangled);
                }
                self.vtables.insert(vtable_name.clone(), method_names);
            }
            let vtable_op = Operand::Constant(Constant::GlobalData(vtable_name.clone()));
            let fat_ptr_tmp = self.new_local(to_ty.clone(), None, false);
            self.push_statement(
                StatementKind::Assign(
                    fat_ptr_tmp,
                    Rvalue::Aggregate(AggregateKind::FatPtr, vec![op, vtable_op]),
                ),
                span,
            );
            return Operand::Copy(fat_ptr_tmp);
        }

        if *from_ty == Type::PyObject {
            let native_ty = match to_ty {
                Type::Float
                | Type::F32
                | Type::Int
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Usize
                | Type::Bool => Some(to_ty.clone()),
                _ => None,
            };
            if let Some(cast_ty) = native_ty {
                let tmp = self.new_local(cast_ty.clone(), None, false);
                self.push_statement(StatementKind::Assign(tmp, Rvalue::Cast(op, cast_ty)), span);
                return Operand::Copy(tmp);
            }
        }

        // `None` carries no type tag once it sits in an `Any` slot, so box it to
        // the runtime null sentinel rather than leaving a bare `0`.
        if *to_ty == Type::Any && *from_ty == Type::Null {
            return self.box_into_any(op, from_ty, span);
        }

        op
    }

    pub(super) fn lower_expr(&mut self, expr: &Expr) -> Operand {
        match &expr.kind {
            ExprKind::Integer(i) => Operand::Constant(Constant::Int(*i)),
            ExprKind::Null => Operand::Constant(Constant::Int(0)),
            ExprKind::Range {
                start,
                end,
                inclusive,
            } => {
                let start_op = self.lower_expr_as_copy(start);
                let end_op = self.lower_expr_as_copy(end);
                let tmp = self.new_local(Type::List(Box::new(Type::Int)), None, true);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_range_list".to_string(),
                            )),
                            args: vec![
                                start_op,
                                end_op,
                                Operand::Constant(Constant::Int(*inclusive as i64)),
                            ],
                        },
                    ),
                    expr.span,
                );
                Operand::Copy(tmp)
            }
            ExprKind::Float(f) => Operand::Constant(Constant::Float((*f).to_bits())),
            ExprKind::Str(s) => Operand::Constant(Constant::Str(s.clone())),
            ExprKind::FStr(exprs) => self.lower_fstr_expr(exprs, expr.span),
            ExprKind::Bool(b) => Operand::Constant(Constant::Bool(*b)),
            ExprKind::Try(inner) => self.lower_try_expr(inner, expr.span, expr.id),
            ExprKind::Await(inner) => self.lower_await_expr(inner, expr.id),
            ExprKind::AsyncBlock(body) => self.lower_async_block_expr(body, expr.span),
            ExprKind::Deref(inner) => self.lower_deref_expr(inner, expr.id),
            ExprKind::Borrow(inner) => self.lower_borrow_expr(inner, expr.id, expr.span),
            ExprKind::MutBorrow(inner) => self.lower_mut_borrow_expr(inner, expr.id, expr.span),
            ExprKind::Identifier(name) => self.lower_identifier_expr(name, expr.id),
            ExprKind::BinOp { left, op, right } => {
                self.lower_binop_expr(left, op, right, expr.span, expr.id)
            }
            ExprKind::UnaryOp { op, operand } => {
                self.lower_unary_op_expr(op, operand, expr.span, expr.id)
            }
            ExprKind::Cast(operand, _ty) => self.lower_cast_expr(operand, expr.span, expr.id),
            ExprKind::Call { callee, args } => self.lower_call_expr(callee, args, expr),
            ExprKind::List(elems) | ExprKind::Tuple(elems) => {
                let is_tuple = matches!(expr.kind, ExprKind::Tuple(_));
                self.lower_list_or_tuple_expr(elems, is_tuple, expr.span, expr.id)
            }
            ExprKind::Set(elems) => self.lower_set_expr(elems, expr.span, expr.id),
            ExprKind::Dict(pairs) => self.lower_dict_expr(pairs, expr.span, expr.id),
            ExprKind::Attr { obj, attr } => self.lower_attr_expr(obj, attr, expr.span, expr.id),
            ExprKind::Index { obj, index } => self.lower_index_expr(obj, index, expr.span, expr.id),
            ExprKind::Slice { .. } => {
                panic!(
                    "standalone slice expression not supported at expr.span={:?}",
                    expr.span
                );
            }
            ExprKind::ListComp { elt, clauses } => {
                self.lower_list_comp_expr(elt, clauses, expr.span, expr.id)
            }
            ExprKind::SetComp { elt, clauses } => {
                self.lower_set_comp_expr(elt, clauses, expr.span, expr.id)
            }
            ExprKind::DictComp {
                key,
                value,
                clauses,
            } => self.lower_dict_comp_expr(key, value, clauses, expr.span, expr.id),
            ExprKind::Match {
                expr: match_expr,
                cases,
            } => self.lower_match_expr(match_expr, cases, expr.span, expr.id),
        }
    }

    pub(super) fn lower_call_expr(
        &mut self,
        callee: &Expr,
        args: &[CallArg],
        expr: &Expr,
    ) -> Operand {
        let callee_name = if let ExprKind::Identifier(name) = &callee.kind {
            Some(name.as_str())
        } else {
            None
        };

        let (mut arg_ops, mut arg_kw_names, arg_tys) =
            self.lower_call_args(args, callee, callee_name, expr.span);

        if let Some(kwarg_map) = self.expr_kwarg_maps.get(&expr.id) {
            let mut new_arg_ops = vec![Operand::Constant(Constant::Int(0)); kwarg_map.len()];
            for (i, op) in arg_ops.iter().enumerate() {
                if let Some(target_idx) = kwarg_map.iter().position(|&x| x == i) {
                    new_arg_ops[target_idx] = op.clone();
                }
            }
            arg_ops = new_arg_ops;
            arg_kw_names = vec![None; arg_ops.len()];
        }

        let callee_ty = self.get_type(callee.id);

        if callee_ty == Type::PyObject {
            let callee_op = self.lower_expr_as_copy(callee);
            let py_result =
                self.lower_pyobject_call(callee_op, args, arg_ops, arg_kw_names, expr.span);
            return self.coerce_pyobj_if_needed(py_result, expr.id, expr.span);
        }

        if let Some(name) = callee_name {
            let base = name.rsplit("::").next().unwrap_or(name);
            if matches!(base, "panic" | "unwrap" | "unwrap_err") {
                self.emit_set_fault_loc(expr.span);
            }

            if name == "type"
                && !args.is_empty()
                && let Some(op) = self.lower_type_builtin(args, expr.span)
            {
                return op;
            }

            if name == "len"
                && !args.is_empty()
                && let Some(op) = self.lower_len_builtin(args, expr.span)
            {
                return op;
            }

            if (name == "max" || name == "min")
                && args.len() == 2
                && let Some(op) = self.lower_maxmin_builtin(name, args, expr.span, expr.id)
            {
                return op;
            }

            if let Some(op) =
                self.lower_enum_variant_call(name, arg_ops.clone(), expr.span, expr.id)
            {
                return op;
            }

            if name == "list_new"
                && !args.is_empty()
                && let Some(op) = self.lower_list_new_builtin(args, expr.span)
            {
                return op;
            }

            if let Some(op) = self.lower_bytes_builtin(name, args, expr.span) {
                return op;
            }
        }

        if let ExprKind::Attr { obj, attr } = &callee.kind {
            return self.lower_attr_method_call_section(
                callee,
                obj,
                attr,
                args,
                arg_ops,
                arg_kw_names,
                arg_tys,
                expr.span,
                expr.id,
            );
        }

        if let Type::Struct(struct_name, type_args) = callee_ty {
            return self.lower_struct_construct_call(
                &struct_name,
                &type_args,
                arg_ops,
                expr.span,
                expr.id,
            );
        }

        let func = self.lower_expr(callee);
        self.lower_general_call_path(
            callee,
            func,
            arg_ops,
            arg_kw_names,
            arg_tys,
            expr.span,
            expr.id,
        )
    }

    pub(super) fn lower_expr_as_copy(&mut self, expr: &Expr) -> Operand {
        let op = self.lower_expr(expr);
        match op {
            Operand::Move(l) => Operand::Copy(l),
            _ => op,
        }
    }

    pub(super) fn coerce_pyobj_if_needed(
        &mut self,
        op: Operand,
        expr_id: usize,
        span: crate::span::Span,
    ) -> Operand {
        let declared_ty = self.get_type(expr_id);
        match &declared_ty {
            Type::Float
            | Type::F32
            | Type::Int
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize
            | Type::Bool => {
                let coerced = self.new_local(declared_ty.clone(), None, false);
                self.push_statement(
                    StatementKind::Assign(coerced, Rvalue::Cast(op, declared_ty)),
                    span,
                );
                self.operand_for_local(coerced)
            }
            _ => op,
        }
    }

    pub(super) fn emit_to_py_arg(
        &mut self,
        op: Operand,
        ty: &Type,
        span: crate::span::Span,
    ) -> Operand {
        if *ty != Type::Float {
            return op;
        }

        let tmp = self.new_local(Type::PyObject, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(
                        "__olive_py_from_float".to_string(),
                    )),
                    args: vec![op],
                },
            ),
            span,
        );
        self.operand_for_local(tmp)
    }

    pub(super) fn is_int_ty(ty: &Type) -> bool {
        matches!(
            ty,
            Type::Int
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Usize
        )
    }

    pub(super) fn enum_type_id(enum_name: &str) -> i64 {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        enum_name.hash(&mut h);
        (h.finish() & 0x7FFF_FFFF_FFFF_FFFF) as i64
    }

    pub(super) fn struct_type_id(struct_name: &str) -> i64 {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        struct_name.hash(&mut h);
        (h.finish() & 0x7FFF_FFFF_FFFF_FFFF) as i64
    }

    /// Records the Olive call site just before a Python call so an uncaught
    /// Python exception can be reported against the exact source line.
    pub(super) fn emit_py_set_loc(&mut self, span: Span) {
        let loc = match self.file_names.get(&span.file_id) {
            Some(file) => format!("{}:{}:{}", file, span.line, span.col),
            None => format!("{}:{}", span.line, span.col),
        };
        let loc_local = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                loc_local,
                Rvalue::Use(Operand::Constant(Constant::Str(loc))),
            ),
            span,
        );
        let sink = self.new_local(Type::Null, None, false);
        self.push_statement(
            StatementKind::Assign(
                sink,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_py_set_loc".to_string())),
                    args: vec![Operand::Copy(loc_local)],
                },
            ),
            span,
        );
    }

    /// Records the user's call site just before a fault-prone prelude call
    /// (`panic`, `unwrap`, `unwrap_err`) so the abort points at the caller, not
    /// the one-line library wrapper. Mirrors Rust's `#[track_caller]`.
    pub(super) fn emit_set_fault_loc(&mut self, span: Span) {
        let loc = match self.file_names.get(&span.file_id) {
            Some(file) => format!("{}:{}:{}", file, span.line, span.col),
            None => format!("{}:{}", span.line, span.col),
        };
        let loc_local = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                loc_local,
                Rvalue::Use(Operand::Constant(Constant::Str(loc))),
            ),
            span,
        );
        let sink = self.new_local(Type::Null, None, false);
        self.push_statement(
            StatementKind::Assign(
                sink,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(
                        "__olive_set_fault_loc".to_string(),
                    )),
                    args: vec![Operand::Copy(loc_local)],
                },
            ),
            span,
        );
    }

    /// Prepends `<file>:<line>:<col>: ` to a runtime error-message string local,
    /// so a `try`-caught Python exception carries the same call-site prefix as an
    /// uncaught one.
    pub(super) fn prepend_call_loc(&mut self, msg_local: Local, span: Span) -> Local {
        let loc = match self.file_names.get(&span.file_id) {
            Some(file) => format!("{}:{}:{}: ", file, span.line, span.col),
            None => format!("{}:{}: ", span.line, span.col),
        };
        let loc_local = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                loc_local,
                Rvalue::Use(Operand::Constant(Constant::Str(loc))),
            ),
            span,
        );
        let out = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                out,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_str_concat".to_string())),
                    args: vec![Operand::Copy(loc_local), Operand::Copy(msg_local)],
                },
            ),
            span,
        );
        out
    }

    fn is_py_call(&self, expr: &Expr) -> bool {
        if let ExprKind::Call { callee, .. } = &expr.kind {
            let callee_ty = self.get_type(callee.id);
            if callee_ty == Type::PyObject {
                return true;
            }
            if let ExprKind::Attr { obj, .. } = &callee.kind {
                return self.get_type(obj.id) == Type::PyObject;
            }
        }
        false
    }

    fn lower_py_call_safe(&mut self, expr: &Expr) -> Operand {
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

        let zipped: Vec<(Operand, Option<String>, usize)> = arg_ops
            .into_iter()
            .zip(arg_kw_names)
            .enumerate()
            .map(|(i, (op, kw))| (op, kw, i))
            .collect();

        let mut pos_ops: Vec<Operand> = Vec::new();
        let mut kw_ops: Vec<Operand> = Vec::new();
        for (op, kw_name, i) in zipped {
            let arg_ty = args
                .get(i)
                .map(|a| match a {
                    CallArg::Positional(e)
                    | CallArg::Splat(e)
                    | CallArg::KwSplat(e)
                    | CallArg::Keyword(_, e) => self.get_type(e.id),
                })
                .unwrap_or(Type::Any);
            if let Some(name) = kw_name {
                let py_op = self.emit_to_py_arg(op, &arg_ty, expr.span);
                kw_ops.push(Operand::Constant(Constant::Str(name)));
                kw_ops.push(py_op);
            } else {
                pos_ops.push(self.emit_to_py_arg(op, &arg_ty, expr.span));
            }
        }

        let args_list = self.new_local(Type::List(Box::new(Type::Any)), None, true);
        self.push_statement(
            StatementKind::Assign(args_list, Rvalue::Aggregate(AggregateKind::List, pos_ops)),
            expr.span,
        );

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

        let result = self.new_local(Type::Any, None, true);
        if kw_ops.is_empty() {
            self.push_statement(
                StatementKind::Assign(
                    result,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_py_call_safe".to_string(),
                        )),
                        args: vec![func_op, Operand::Copy(args_list)],
                    },
                ),
                expr.span,
            );
        } else {
            let kwargs_list = self.new_local(Type::List(Box::new(Type::Any)), None, true);
            self.push_statement(
                StatementKind::Assign(kwargs_list, Rvalue::Aggregate(AggregateKind::List, kw_ops)),
                expr.span,
            );
            self.push_statement(
                StatementKind::Assign(
                    result,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_py_call_kw_safe".to_string(),
                        )),
                        args: vec![
                            func_op,
                            Operand::Copy(args_list),
                            Operand::Copy(kwargs_list),
                        ],
                    },
                ),
                expr.span,
            );
        }

        self.operand_for_local(result)
    }
}

#[cfg(test)]
mod tests {
    use super::super::MirBuilder;
    use crate::lexer::Lexer;
    use crate::mir::ir::{Constant, Operand, Rvalue, StatementKind};
    use crate::parser::Parser;
    use crate::semantic::{Resolver, TypeChecker};
    use rustc_hash::FxHashSet;

    fn build(src: &str) -> Vec<super::super::super::ir::MirFunction> {
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
        );
        builder.build_program(&prog);
        builder.functions
    }

    #[test]
    fn integer_literal_produces_constant() {
        let fns = build("fn f() -> i64:\n    return 42\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_const = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Int(42))))
                )
            })
        });
        assert!(has_const);
    }

    #[test]
    fn bool_literal_produces_constant() {
        let fns = build("fn f() -> bool:\n    return true\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_const = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Bool(_))))
                )
            })
        });
        assert!(has_const, "expected a Bool constant in MIR");
    }

    #[test]
    fn binary_op_produces_binop_rvalue() {
        let fns = build("fn f() -> i64:\n    return 1 + 2\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_binop = f.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(&s.kind, StatementKind::Assign(_, Rvalue::BinaryOp(_, _, _))))
        });
        assert!(has_binop);
    }

    #[test]
    fn unary_op_produces_unaryop_rvalue() {
        let fns = build("fn f(x: i64) -> i64:\n    return -x\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_unary = f.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(&s.kind, StatementKind::Assign(_, Rvalue::UnaryOp(_, _))))
        });
        assert!(has_unary);
    }

    #[test]
    fn function_call_produces_call_rvalue() {
        let fns = build("fn g() -> i64:\n    return 1\n\nfn f() -> i64:\n    return g()\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_call = f.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(&s.kind, StatementKind::Assign(_, Rvalue::Call { .. })))
        });
        assert!(has_call);
    }

    #[test]
    fn string_literal_produces_constant() {
        let fns = build("fn f() -> str:\n    return \"hello\"\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_str = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(&s.kind, StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Str(h)))) if h == "hello")
            })
        });
        assert!(has_str);
    }

    #[test]
    fn cast_produces_cast_rvalue() {
        let fns = build("fn f(x: i64) -> float:\n    return x as float\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_cast = f.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(&s.kind, StatementKind::Assign(_, Rvalue::Cast(_, _))))
        });
        assert!(has_cast);
    }
}
