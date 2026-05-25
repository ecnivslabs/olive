use super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{CallArg, Expr, ExprKind, StmtKind};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    pub(super) fn coerce(
        &mut self,
        op: Operand,
        from_ty: &Type,
        to_ty: &Type,
        span: Span,
    ) -> Operand {
        if let Type::TraitObject(trait_name, _) = to_ty {
            if let Type::Struct(struct_name, _) = from_ty {
                let vtable_name = format!("__vtable_{}_{}", trait_name, struct_name);
                if !self.vtables.contains_key(&vtable_name) {
                    if let Some(trait_def) = self.traits.get(trait_name) {
                        let mut method_names = Vec::new();
                        for (method_name, _) in &trait_def.methods {
                            let mangled = format!("{}::{}", struct_name, method_name);
                            if let Type::Struct(_, type_args) = from_ty {
                                if !type_args.is_empty() {
                                    method_names.push(self.monomorphize(&mangled, type_args));
                                    continue;
                                }
                            }
                            method_names.push(mangled);
                        }
                        self.vtables.insert(vtable_name.clone(), method_names);
                    }
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
        }
        op
    }

    pub(super) fn lower_expr(&mut self, expr: &Expr) -> Operand {
        match &expr.kind {
            ExprKind::Integer(i) => Operand::Constant(Constant::Int(*i)),
            ExprKind::Float(f) => Operand::Constant(Constant::Float((*f).to_bits())),
            ExprKind::Str(s) => Operand::Constant(Constant::Str(s.clone())),
            ExprKind::FStr(exprs) => {
                if exprs.is_empty() {
                    return Operand::Constant(Constant::Str("".to_string()));
                }

                let mut current_res: Option<Operand> = None;

                for e in exprs {
                    let op = self.lower_expr_as_copy(e);
                    let ty = self.get_type(e.id);

                    let str_op = if ty == Type::Str {
                        op
                    } else {
                        let tmp = self.new_local(Type::Str, None, true);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function("str".to_string())),
                                    args: vec![op],
                                },
                            ),
                            e.span,
                        );
                        self.operand_for_local(tmp)
                    };

                    if let Some(res) = current_res {
                        let tmp = self.new_local(Type::Str, None, true);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::BinaryOp(crate::parser::BinOp::Add, res, str_op),
                            ),
                            expr.span,
                        );
                        current_res = Some(Operand::Copy(tmp));
                    } else {
                        current_res = Some(str_op);
                    }
                }

                current_res.unwrap()
            }
            ExprKind::Bool(b) => Operand::Constant(Constant::Bool(*b)),

            ExprKind::Try(inner) => {
                if self.is_py_call(inner) {
                    let result_op = self.lower_py_call_safe(inner);

                    let is_ok_tmp = self.new_local(Type::Int, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            is_ok_tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_result_is_ok".to_string(),
                                )),
                                args: vec![result_op.clone()],
                            },
                        ),
                        expr.span,
                    );

                    let success_bb = self.new_block();
                    let error_bb = self.new_block();

                    if let Some(bb) = self.current_block {
                        self.terminate_block(
                            bb,
                            TerminatorKind::SwitchInt {
                                discr: Operand::Copy(is_ok_tmp),
                                targets: vec![(1, success_bb)],
                                otherwise: error_bb,
                            },
                            expr.span,
                        );
                    }

                    self.current_block = Some(error_bb);

                    let err_msg_tmp = self.new_local(Type::Str, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            err_msg_tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_result_err_msg".to_string(),
                                )),
                                args: vec![result_op.clone()],
                            },
                        ),
                        expr.span,
                    );

                    let err_struct_tmp =
                        self.new_local(Type::Struct("Error".to_string(), vec![]), None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            err_struct_tmp,
                            Rvalue::Aggregate(crate::mir::AggregateKind::Dict, vec![]),
                        ),
                        expr.span,
                    );

                    self.push_statement(
                        StatementKind::SetAttr(
                            Operand::Copy(err_struct_tmp),
                            "msg".to_string(),
                            Operand::Copy(err_msg_tmp),
                        ),
                        expr.span,
                    );

                    self.push_statement(
                        StatementKind::Assign(Local(0), Rvalue::Use(Operand::Copy(err_struct_tmp))),
                        expr.span,
                    );
                    self.emit_defers();
                    self.terminate_block(error_bb, TerminatorKind::Return, expr.span);

                    self.current_block = Some(success_bb);
                    let payload_tmp = self.new_local(Type::PyObject, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            payload_tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_result_unwrap".to_string(),
                                )),
                                args: vec![result_op],
                            },
                        ),
                        expr.span,
                    );

                    return Operand::Copy(payload_tmp);
                }

                let inner_op = self.lower_expr_as_copy(inner);
                let inner_ty = self.get_type(inner.id);

                let is_error = |ty: &Type| -> bool {
                    match ty {
                        Type::Struct(name, _) | Type::Enum(name, _) => {
                            name == "Error" || name.ends_with("Error")
                        }
                        _ => false,
                    }
                };

                let mut error_type_id = -1;
                if let Type::Union(variants) = &inner_ty {
                    for v in variants {
                        if is_error(v) {
                            match v {
                                Type::Struct(name, _) => {
                                    error_type_id = Self::struct_type_id(name) as i64;
                                    break;
                                }
                                Type::Enum(name, _) => {
                                    error_type_id = Self::enum_type_id(name) as i64;
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                } else if is_error(&inner_ty) {
                    match &inner_ty {
                        Type::Struct(name, _) => {
                            error_type_id = Self::struct_type_id(name) as i64;
                        }
                        Type::Enum(name, _) => {
                            error_type_id = Self::enum_type_id(name) as i64;
                        }
                        _ => {}
                    }
                } else if inner_ty == Type::PyObject {
                    error_type_id = Self::struct_type_id(&"Error".to_string()) as i64;
                }

                if error_type_id == -1 {
                    return inner_op;
                }

                let type_id_tmp = self.new_local(Type::Int, None, false);
                self.push_statement(
                    StatementKind::Assign(type_id_tmp, Rvalue::GetTypeId(inner_op.clone())),
                    expr.span,
                );

                let success_bb = self.new_block();
                let error_bb = self.new_block();

                let is_err_tmp = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        is_err_tmp,
                        Rvalue::BinaryOp(
                            crate::parser::BinOp::Eq,
                            Operand::Copy(type_id_tmp),
                            Operand::Constant(Constant::Int(error_type_id)),
                        ),
                    ),
                    expr.span,
                );

                if let Some(bb) = self.current_block {
                    self.terminate_block(
                        bb,
                        TerminatorKind::SwitchInt {
                            discr: Operand::Copy(is_err_tmp),
                            targets: vec![(1, error_bb)],
                            otherwise: success_bb,
                        },
                        expr.span,
                    );
                }

                self.current_block = Some(error_bb);
                self.push_statement(
                    StatementKind::Assign(Local(0), Rvalue::Use(inner_op.clone())),
                    expr.span,
                );
                self.emit_defers();
                self.terminate_block(error_bb, TerminatorKind::Return, expr.span);

                self.current_block = Some(success_bb);

                inner_op
            }

            ExprKind::Await(inner) => {
                let inner_op = self.lower_expr(inner);
                let result_ty = self.get_type(expr.id);
                let tmp = self.new_local(result_ty, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_await".to_string(),
                            )),
                            args: vec![inner_op],
                        },
                    ),
                    expr.span,
                );
                Operand::Copy(tmp)
            }

            ExprKind::AsyncBlock(body) => {
                let tmp = self.new_local(Type::Any, None, false);
                self.enter_scope();
                let mut last_op = Operand::Constant(Constant::None);
                for (i, s) in body.iter().enumerate() {
                    if i == body.len() - 1 {
                        if let StmtKind::ExprStmt(e) = &s.kind {
                            last_op = self.lower_expr(e);
                        } else {
                            self.lower_stmt(s);
                        }
                    } else {
                        self.lower_stmt(s);
                    }
                }
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_make_future".to_string(),
                            )),
                            args: vec![last_op],
                        },
                    ),
                    expr.span,
                );
                self.leave_scope();
                Operand::Copy(tmp)
            }

            ExprKind::Deref(inner) => {
                let ptr_op = self.lower_expr(inner);
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::PtrLoad(ptr_op)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Borrow(inner) => {
                let tmp = self.new_tmp_for_expr(expr);
                let rval = if let ExprKind::Identifier(name) = &inner.kind {
                    if let Some(local) = self.lookup_var(name) {
                        Rvalue::Ref(local)
                    } else {
                        let op = self.lower_expr(inner);
                        Rvalue::Use(op)
                    }
                } else {
                    let op = self.lower_expr(inner);
                    Rvalue::Use(op)
                };
                self.push_statement(StatementKind::Assign(tmp, rval), expr.span);
                self.operand_for_local(tmp)
            }

            ExprKind::MutBorrow(inner) => {
                let tmp = self.new_tmp_for_expr(expr);
                let rval = if let ExprKind::Identifier(name) = &inner.kind {
                    if let Some(local) = self.lookup_var(name) {
                        Rvalue::MutRef(local)
                    } else {
                        let op = self.lower_expr(inner);
                        Rvalue::Use(op)
                    }
                } else {
                    let op = self.lower_expr(inner);
                    Rvalue::Use(op)
                };
                self.push_statement(StatementKind::Assign(tmp, rval), expr.span);
                self.operand_for_local(tmp)
            }

            ExprKind::Identifier(name) => {
                if let Some(local) = self.lookup_var(name) {
                    self.operand_for_local(local)
                } else if let Some(global_op) = self.globals.get(name).cloned() {
                    if let Operand::Constant(Constant::GlobalData(_)) = &global_op {
                        let ty = self.get_type(expr.id);
                        let tmp = self.new_local(ty, None, false);
                        self.push_statement(
                            StatementKind::Assign(tmp, Rvalue::PtrLoad(global_op.clone())),
                            expr.span,
                        );
                        Operand::Copy(tmp)
                    } else {
                        global_op
                    }
                } else {
                    Operand::Constant(Constant::Function(name.clone()))
                }
            }

            ExprKind::BinOp { left, op, right } => {
                let r_ty = self.get_type(right.id).clone();

                if r_ty == Type::Str
                    && matches!(op, crate::parser::BinOp::In | crate::parser::BinOp::NotIn)
                {
                    let haystack = self.lower_expr_as_copy(right);
                    let needle = self.lower_expr_as_copy(left);

                    let call_tmp = self.new_local(Type::Bool, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            call_tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_str_contains".to_string(),
                                )),
                                args: vec![haystack, needle],
                            },
                        ),
                        expr.span,
                    );

                    if matches!(op, crate::parser::BinOp::In) {
                        return self.operand_for_local(call_tmp);
                    } else {
                        let not_tmp = self.new_tmp_for_expr(expr);
                        self.push_statement(
                            StatementKind::Assign(
                                not_tmp,
                                Rvalue::UnaryOp(
                                    crate::parser::UnaryOp::Not,
                                    Operand::Copy(call_tmp),
                                ),
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(not_tmp);
                    }
                }

                if matches!(op, crate::parser::BinOp::And | crate::parser::BinOp::Or) {
                    let tmp = self.new_tmp_for_expr(expr);
                    let l = self.lower_expr(left);
                    self.push_statement(
                        StatementKind::Assign(tmp, Rvalue::Use(l.clone())),
                        expr.span,
                    );

                    let rhs_bb = self.new_block();
                    let merge_bb = self.new_block();

                    if let Some(bb) = self.current_block {
                        if matches!(op, crate::parser::BinOp::And) {
                            self.terminate_block(
                                bb,
                                TerminatorKind::SwitchInt {
                                    discr: l,
                                    targets: vec![(1, rhs_bb)],
                                    otherwise: merge_bb,
                                },
                                expr.span,
                            );
                        } else {
                            self.terminate_block(
                                bb,
                                TerminatorKind::SwitchInt {
                                    discr: l,
                                    targets: vec![(0, rhs_bb)],
                                    otherwise: merge_bb,
                                },
                                expr.span,
                            );
                        }
                    }

                    self.current_block = Some(rhs_bb);
                    let r = self.lower_expr(right);
                    self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(r)), expr.span);
                    if let Some(bb) = self.current_block {
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: merge_bb },
                            expr.span,
                        );
                    }

                    self.current_block = Some(merge_bb);
                    return self.operand_for_local(tmp);
                }

                let l = if matches!(
                    op,
                    crate::parser::BinOp::Eq
                        | crate::parser::BinOp::NotEq
                        | crate::parser::BinOp::Lt
                        | crate::parser::BinOp::LtEq
                        | crate::parser::BinOp::Gt
                        | crate::parser::BinOp::GtEq
                        | crate::parser::BinOp::In
                        | crate::parser::BinOp::NotIn
                ) {
                    self.lower_expr_as_copy(left)
                } else {
                    self.lower_expr(left)
                };
                let r = if matches!(
                    op,
                    crate::parser::BinOp::Eq
                        | crate::parser::BinOp::NotEq
                        | crate::parser::BinOp::Lt
                        | crate::parser::BinOp::LtEq
                        | crate::parser::BinOp::Gt
                        | crate::parser::BinOp::GtEq
                        | crate::parser::BinOp::In
                        | crate::parser::BinOp::NotIn
                ) {
                    self.lower_expr_as_copy(right)
                } else {
                    self.lower_expr(right)
                };
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::BinaryOp(op.clone(), l, r)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::UnaryOp { op, operand } => {
                let o = self.lower_expr(operand);
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::UnaryOp(op.clone(), o)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Cast(operand, _ty) => {
                let op = self.lower_expr(operand);
                let tmp = self.new_tmp_for_expr(expr);

                let target_ty = self.get_type(expr.id);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::Cast(op, target_ty)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Call { callee, args } => {
                let mut arg_ops = Vec::new();
                let mut arg_kw_names: Vec<Option<String>> = Vec::new();
                let mut arg_tys: Vec<Type> = Vec::new();
                for arg in args {
                    match arg {
                        CallArg::Splat(e)
                            if self.get_type(e.id) == crate::semantic::types::Type::Int =>
                        {
                            let ptr_op = self.lower_expr(e);
                            let tmp =
                                self.new_local(crate::semantic::types::Type::Int, None, false);
                            self.push_statement(
                                StatementKind::Assign(tmp, Rvalue::PtrLoad(ptr_op)),
                                e.span,
                            );
                            arg_ops.push(Operand::Copy(tmp));
                            arg_kw_names.push(None);
                            arg_tys.push(crate::semantic::types::Type::Int);
                        }
                        CallArg::Positional(e) | CallArg::Splat(e) | CallArg::KwSplat(e) => {
                            let is_readonly_builtin =
                                if let ExprKind::Identifier(name) = &callee.kind {
                                    matches!(
                                        name.as_str(),
                                        "len"
                                            | "print"
                                            | "str"
                                            | "int"
                                            | "float"
                                            | "type"
                                            | "range"
                                            | "slice"
                                    )
                                } else {
                                    false
                                };

                            if is_readonly_builtin {
                                arg_ops.push(self.lower_expr_as_copy(e));
                            } else {
                                arg_ops.push(self.lower_expr(e));
                            }
                            arg_kw_names.push(None);
                            arg_tys.push(self.get_type(e.id).clone());
                        }
                        CallArg::Keyword(name, e) => {
                            let is_readonly_builtin = if let ExprKind::Identifier(n) = &callee.kind
                            {
                                matches!(
                                    n.as_str(),
                                    "len"
                                        | "print"
                                        | "str"
                                        | "int"
                                        | "float"
                                        | "type"
                                        | "range"
                                        | "slice"
                                )
                            } else {
                                false
                            };

                            if is_readonly_builtin {
                                arg_ops.push(self.lower_expr_as_copy(e));
                            } else {
                                arg_ops.push(self.lower_expr(e));
                            }
                            arg_kw_names.push(Some(name.clone()));
                            arg_tys.push(self.get_type(e.id).clone());
                        }
                    }
                }

                if let Some(kwarg_map) = self.expr_kwarg_maps.get(&expr.id) {
                    let mut new_arg_ops =
                        vec![Operand::Constant(Constant::Int(0)); kwarg_map.len()];
                    for (i, op) in arg_ops.iter().enumerate() {
                        if let Some(target_idx) = kwarg_map.iter().position(|&x| x == i) {
                            new_arg_ops[target_idx] = op.clone();
                        }
                    }
                    arg_ops = new_arg_ops;
                    arg_kw_names = vec![None; kwarg_map.len()];
                }

                let callee_ty = self.get_type(callee.id);
                if callee_ty == Type::PyObject {
                    let callee_op = self.lower_expr_as_copy(callee);
                    let mut pos_ops = Vec::new();
                    let mut kw_ops = Vec::new();
                    for (i, (op, kw_name)) in arg_ops
                        .into_iter()
                        .zip(arg_kw_names.into_iter())
                        .enumerate()
                    {
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
                        StatementKind::Assign(
                            args_list,
                            Rvalue::Aggregate(AggregateKind::List, pos_ops),
                        ),
                        expr.span,
                    );

                    if kw_ops.is_empty() {
                        let result = self.new_local(Type::PyObject, None, true);
                        self.push_statement(
                            StatementKind::Assign(
                                result,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_py_call".to_string(),
                                    )),
                                    args: vec![callee_op, Operand::Copy(args_list)],
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(result);
                    } else {
                        let kwargs_list =
                            self.new_local(Type::List(Box::new(Type::Any)), None, true);
                        self.push_statement(
                            StatementKind::Assign(
                                kwargs_list,
                                Rvalue::Aggregate(AggregateKind::List, kw_ops),
                            ),
                            expr.span,
                        );
                        let result = self.new_local(Type::PyObject, None, true);
                        self.push_statement(
                            StatementKind::Assign(
                                result,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_py_call_kw".to_string(),
                                    )),
                                    args: vec![
                                        callee_op,
                                        Operand::Copy(args_list),
                                        Operand::Copy(kwargs_list),
                                    ],
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(result);
                    }
                }

                if let ExprKind::Identifier(name) = &callee.kind
                    && name == "type"
                    && !args.is_empty()
                {
                    let arg_expr = match &args[0] {
                        CallArg::Positional(e)
                        | CallArg::Keyword(_, e)
                        | CallArg::Splat(e)
                        | CallArg::KwSplat(e) => e,
                    };
                    let arg_ty = self.get_type(arg_expr.id);
                    let type_str = format!("<struct '{}'>", arg_ty);
                    return Operand::Constant(Constant::Str(type_str));
                }

                if let ExprKind::Identifier(name) = &callee.kind
                    && name == "len"
                    && !args.is_empty()
                {
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

                    if current_arg_ty == Type::Str {
                        let arg_op = self.lower_expr_as_copy(arg_expr);
                        let tmp = self.new_local(Type::Int, None, false);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_str_len".to_string(),
                                    )),
                                    args: vec![arg_op],
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    } else if matches!(
                        current_arg_ty,
                        Type::List(_)
                            | Type::Tuple(_)
                            | Type::Set(_)
                            | Type::Dict(_, _)
                            | Type::Any
                    ) {
                        let arg_op = self.lower_expr_as_copy(arg_expr);
                        let tmp = self.new_local(Type::Int, None, false);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_list_len".to_string(),
                                    )),
                                    args: vec![arg_op],
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    } else if current_arg_ty == Type::PyObject {
                        let arg_op = self.lower_expr_as_copy(arg_expr);
                        let tmp = self.new_local(Type::Int, None, false);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_py_len".to_string(),
                                    )),
                                    args: vec![arg_op],
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    }
                }
                if let ExprKind::Identifier(name) = &callee.kind
                    && (name == "max" || name == "min")
                    && args.len() == 2
                {
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
                    let result_ty = self.get_type(a_expr.id);

                    // cond = (a > b) for max, (a < b) for min
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
                        expr.span,
                    );

                    let result_local = self.new_local(result_ty, None, false);
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
                            expr.span,
                        );
                    }

                    self.current_block = Some(true_bb);
                    self.push_statement(
                        StatementKind::Assign(result_local, Rvalue::Use(a_op)),
                        expr.span,
                    );
                    self.terminate_block(
                        true_bb,
                        TerminatorKind::Goto { target: exit_bb },
                        expr.span,
                    );

                    self.current_block = Some(false_bb);
                    self.push_statement(
                        StatementKind::Assign(result_local, Rvalue::Use(b_op)),
                        expr.span,
                    );
                    self.terminate_block(
                        false_bb,
                        TerminatorKind::Goto { target: exit_bb },
                        expr.span,
                    );

                    self.current_block = Some(exit_bb);
                    return self.operand_for_local(result_local);
                }

                if let ExprKind::Identifier(name) = &callee.kind {
                    if let Some((enum_name, tag)) = self.enum_variants.get(name).cloned() {
                        let type_id = Self::enum_type_id(&enum_name);
                        let tmp = self.new_tmp_for_expr(expr);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Aggregate(
                                    AggregateKind::EnumVariant(type_id, tag),
                                    arg_ops,
                                ),
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    }

                    if name == "list_new" && !args.is_empty() {
                        let arg_expr = match &args[0] {
                            CallArg::Positional(e)
                            | CallArg::Keyword(_, e)
                            | CallArg::Splat(e)
                            | CallArg::KwSplat(e) => e,
                        };
                        let arg_op = self.lower_expr(arg_expr);
                        let tmp = self.new_local(Type::List(Box::new(Type::Any)), None, false);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_list_new".to_string(),
                                    )),
                                    args: vec![arg_op],
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    }
                }

                if let ExprKind::Attr { obj, attr } = &callee.kind {
                    // PyObject method call: np.array(args) → getattr + call
                    let obj_ty = self.get_type(obj.id);
                    if obj_ty == Type::PyObject {
                        let obj_op = self.lower_expr_as_copy(obj);
                        let attr_local = self.new_local(Type::PyObject, None, true);
                        self.push_statement(
                            StatementKind::Assign(
                                attr_local,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_py_getattr".to_string(),
                                    )),
                                    args: vec![
                                        obj_op,
                                        Operand::Constant(Constant::Str(attr.clone())),
                                    ],
                                },
                            ),
                            expr.span,
                        );
                        let mut pos_ops = Vec::new();
                        let mut kw_ops = Vec::new();
                        for (i, (op, kw_name)) in arg_ops
                            .into_iter()
                            .zip(arg_kw_names.into_iter())
                            .enumerate()
                        {
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
                            StatementKind::Assign(
                                args_list,
                                Rvalue::Aggregate(AggregateKind::List, pos_ops),
                            ),
                            expr.span,
                        );

                        if kw_ops.is_empty() {
                            let result = self.new_local(Type::PyObject, None, true);
                            self.push_statement(
                                StatementKind::Assign(
                                    result,
                                    Rvalue::Call {
                                        func: Operand::Constant(Constant::Function(
                                            "__olive_py_call".to_string(),
                                        )),
                                        args: vec![
                                            Operand::Copy(attr_local),
                                            Operand::Copy(args_list),
                                        ],
                                    },
                                ),
                                expr.span,
                            );
                            return self.operand_for_local(result);
                        } else {
                            let kwargs_list =
                                self.new_local(Type::List(Box::new(Type::Any)), None, true);
                            self.push_statement(
                                StatementKind::Assign(
                                    kwargs_list,
                                    Rvalue::Aggregate(AggregateKind::List, kw_ops),
                                ),
                                expr.span,
                            );
                            let result = self.new_local(Type::PyObject, None, true);
                            self.push_statement(
                                StatementKind::Assign(
                                    result,
                                    Rvalue::Call {
                                        func: Operand::Constant(Constant::Function(
                                            "__olive_py_call_kw".to_string(),
                                        )),
                                        args: vec![
                                            Operand::Copy(attr_local),
                                            Operand::Copy(args_list),
                                            Operand::Copy(kwargs_list),
                                        ],
                                    },
                                ),
                                expr.span,
                            );
                            return self.operand_for_local(result);
                        }
                    }

                    if let ExprKind::Identifier(name) = &obj.kind {
                        let obj_ty = self.get_type(obj.id);
                        let is_struct_var = matches!(
                            obj_ty,
                            Type::Struct(_, _) | Type::TraitObject(_, _) | Type::Any
                        ) && self.lookup_var(name).is_some();
                        if !is_struct_var {
                            let mangled = format!("{}::{}", name, attr);

                            let variant_info = self.enum_variants.get(&mangled).cloned();
                            if let Some((enum_name, tag)) = variant_info {
                                let type_id = Self::enum_type_id(&enum_name);
                                let tmp = self.new_tmp_for_expr(expr);
                                self.push_statement(
                                    StatementKind::Assign(
                                        tmp,
                                        Rvalue::Aggregate(
                                            AggregateKind::EnumVariant(type_id, tag),
                                            arg_ops,
                                        ),
                                    ),
                                    expr.span,
                                );
                                return self.operand_for_local(tmp);
                            }

                            let mangled_str = mangled.clone();
                            let callee_op =
                                Operand::Constant(Constant::Function(mangled_str.clone()));
                            let call_ret_ty = self.get_type(expr.id);

                            if self.c_ffi_fns.contains(&mangled_str) {
                                if let Type::Struct(ref sname, ref targs) = call_ret_ty.clone() {
                                    if sname == "Result" && !targs.is_empty() {
                                        let inner_ok_ty = targs[0].clone();

                                        let raw_local =
                                            self.new_local(inner_ok_ty.clone(), None, false);
                                        self.push_statement(
                                            StatementKind::Assign(
                                                raw_local,
                                                Rvalue::Call {
                                                    func: callee_op,
                                                    args: arg_ops,
                                                },
                                            ),
                                            expr.span,
                                        );

                                        let is_err_local = self.new_local(Type::Bool, None, false);
                                        match &inner_ok_ty {
                                            Type::Int => {
                                                self.push_statement(
                                                    StatementKind::Assign(
                                                        is_err_local,
                                                        Rvalue::BinaryOp(
                                                            crate::parser::BinOp::Eq,
                                                            Operand::Copy(raw_local),
                                                            Operand::Constant(Constant::Int(-1)),
                                                        ),
                                                    ),
                                                    expr.span,
                                                );
                                            }
                                            Type::Ref(_) | Type::MutRef(_) | Type::Ptr(_) => {
                                                self.push_statement(
                                                    StatementKind::Assign(
                                                        is_err_local,
                                                        Rvalue::BinaryOp(
                                                            crate::parser::BinOp::Eq,
                                                            Operand::Copy(raw_local),
                                                            Operand::Constant(Constant::Int(0)),
                                                        ),
                                                    ),
                                                    expr.span,
                                                );
                                            }
                                            _ => {
                                                self.push_statement(
                                                    StatementKind::Assign(
                                                        is_err_local,
                                                        Rvalue::Use(Operand::Constant(
                                                            Constant::Bool(false),
                                                        )),
                                                    ),
                                                    expr.span,
                                                );
                                            }
                                        }

                                        let ok_bb = self.new_block();
                                        let err_bb = self.new_block();
                                        let exit_bb = self.new_block();

                                        if let Some(bb) = self.current_block {
                                            self.terminate_block(
                                                bb,
                                                TerminatorKind::SwitchInt {
                                                    discr: Operand::Copy(is_err_local),
                                                    targets: vec![(1, err_bb)],
                                                    otherwise: ok_bb,
                                                },
                                                expr.span,
                                            );
                                        }

                                        let result_local = self.new_tmp_for_expr(expr);

                                        self.current_block = Some(ok_bb);
                                        self.push_statement(
                                            StatementKind::Assign(
                                                result_local,
                                                Rvalue::Call {
                                                    func: Operand::Constant(Constant::Function(
                                                        "__olive_result_ok".to_string(),
                                                    )),
                                                    args: vec![Operand::Copy(raw_local)],
                                                },
                                            ),
                                            expr.span,
                                        );
                                        self.terminate_block(
                                            ok_bb,
                                            TerminatorKind::Goto { target: exit_bb },
                                            expr.span,
                                        );

                                        self.current_block = Some(err_bb);
                                        let err_str_local = self.new_local(Type::Str, None, false);
                                        self.push_statement(
                                            StatementKind::Assign(
                                                err_str_local,
                                                Rvalue::Use(Operand::Constant(Constant::Str(
                                                    "FFI call failed".to_string(),
                                                ))),
                                            ),
                                            expr.span,
                                        );
                                        self.push_statement(
                                            StatementKind::Assign(
                                                result_local,
                                                Rvalue::Call {
                                                    func: Operand::Constant(Constant::Function(
                                                        "__olive_result_err".to_string(),
                                                    )),
                                                    args: vec![Operand::Copy(err_str_local)],
                                                },
                                            ),
                                            expr.span,
                                        );
                                        self.terminate_block(
                                            err_bb,
                                            TerminatorKind::Goto { target: exit_bb },
                                            expr.span,
                                        );

                                        self.current_block = Some(exit_bb);
                                        return self.operand_for_local(result_local);
                                    }
                                }
                            }

                            let tmp = self.new_tmp_for_expr(expr);
                            self.push_statement(
                                StatementKind::Assign(
                                    tmp,
                                    Rvalue::Call {
                                        func: callee_op,
                                        args: arg_ops,
                                    },
                                ),
                                expr.span,
                            );
                            return self.operand_for_local(tmp);
                        }
                    }

                    let obj_op = self.lower_expr_as_copy(obj);
                    let tmp = self.new_tmp_for_expr(expr);

                    let mut method_args = vec![obj_op];
                    method_args.extend(arg_ops);

                    if attr == "copy" {
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_copy".to_string(),
                                    )),
                                    args: method_args,
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    }

                    let obj_ty = self.get_type(obj.id);
                    let method_name;

                    if let Type::Struct(struct_name, type_args) = &obj_ty {
                        let base_method_name = format!("{}::{}", struct_name, attr);
                        if !type_args.is_empty() {
                            method_name = self.monomorphize(&base_method_name, &type_args);
                        } else {
                            method_name = base_method_name;
                        }
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(method_name)),
                                    args: method_args,
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    } else if let Type::TraitObject(trait_name, _) = &obj_ty {
                        let method_idx = if let Some(t_def) = self.traits.get(trait_name) {
                            t_def
                                .methods
                                .iter()
                                .position(|(n, _)| n == attr)
                                .unwrap_or(0)
                        } else {
                            0
                        };

                        let method_ptr_tmp = self.new_local(Type::Any, None, false);
                        self.push_statement(
                            StatementKind::Assign(
                                method_ptr_tmp,
                                Rvalue::VTableLoad {
                                    vtable: method_args[0].clone(),
                                    method_idx,
                                },
                            ),
                            expr.span,
                        );

                        let data_ptr_tmp = self.new_local(Type::Any, None, false);
                        self.push_statement(
                            StatementKind::Assign(
                                data_ptr_tmp,
                                Rvalue::FatPtrData(method_args[0].clone()),
                            ),
                            expr.span,
                        );

                        method_args[0] = Operand::Copy(data_ptr_tmp);

                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Copy(method_ptr_tmp),
                                    args: method_args,
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    } else {
                        let method_name = format!("{:?}::{}", obj_ty, attr);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(method_name)),
                                    args: method_args,
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    }
                }

                let callee_ty = self.get_type(callee.id);
                if let Type::Struct(struct_name, type_args) = callee_ty {
                    let obj_tmp = self.new_unscoped_local(self.get_type(expr.id));
                    let alloc_rval = if let Some(fields) = self.struct_fields.get(&struct_name) {
                        let n = fields.len() as i64;
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_struct_alloc".to_string(),
                            )),
                            args: vec![Operand::Constant(Constant::Int(n))],
                        }
                    } else {
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_obj_new".to_string(),
                            )),
                            args: vec![],
                        }
                    };
                    self.push_statement(StatementKind::Assign(obj_tmp, alloc_rval), expr.span);

                    let base_init_name = format!("{}::__init__", struct_name);
                    let init_name = if !type_args.is_empty() {
                        self.monomorphize(&base_init_name, &type_args)
                    } else {
                        base_init_name
                    };
                    let mut init_args = vec![Operand::Copy(obj_tmp)];
                    init_args.extend(arg_ops);

                    let init_res = self.new_tmp_for_expr(expr);
                    self.push_statement(
                        StatementKind::Assign(
                            init_res,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(init_name)),
                                args: init_args,
                            },
                        ),
                        expr.span,
                    );

                    return Operand::Copy(obj_tmp);
                }

                let mut func = self.lower_expr(callee);
                let mut call_fn_name = if let ExprKind::Identifier(name) = &callee.kind {
                    Some(name.clone())
                } else if let ExprKind::Attr { obj, attr } = &callee.kind {
                    if let ExprKind::Identifier(obj_name) = &obj.kind {
                        Some(format!("{}::{}", obj_name, attr))
                    } else {
                        None
                    }
                } else {
                    None
                };

                if call_fn_name.is_none() {
                    if let Operand::Constant(Constant::Function(name)) = &func {
                        call_fn_name = Some(name.clone());
                    }
                }

                if let Some(fn_name) = &call_fn_name {
                    let callee_ty = self.get_type(callee.id);
                    if let Type::Fn(_, _, type_args) = callee_ty
                        && !type_args.is_empty()
                        && self.generic_fns.contains_key(fn_name)
                    {
                        let specialized_name = self.monomorphize(fn_name, &type_args);
                        func = Operand::Constant(Constant::Function(specialized_name.clone()));
                        call_fn_name = Some(specialized_name);
                    }
                }

                let ret_ty = self.get_type(expr.id);
                let mut is_ffi_result = false;
                let mut inner_ok_ty = Type::Any;

                if let Some(fn_name) = &call_fn_name {
                    if self.c_ffi_fns.contains(fn_name) {
                        if let Type::Struct(struct_name, type_args) = &ret_ty {
                            if struct_name == "Result" && type_args.len() >= 1 {
                                is_ffi_result = true;
                                inner_ok_ty = type_args[0].clone();
                            }
                        }
                    }
                }

                let call_result_local = if is_ffi_result {
                    self.new_local(inner_ok_ty.clone(), None, false)
                } else {
                    self.new_tmp_for_expr(expr)
                };

                let callee_ty = self.get_type(callee.id).clone();
                let param_tys = if let Type::Fn(ptys, _, _) = callee_ty {
                    ptys
                } else {
                    Vec::new()
                };

                let final_args = if let Some(name) = &call_fn_name {
                    self.pack_fn_call_args(
                        name,
                        &arg_ops,
                        &arg_tys,
                        &param_tys,
                        &arg_kw_names,
                        expr.span,
                    )
                } else {
                    let mut res = Vec::new();
                    for i in 0..arg_ops.len() {
                        let p_ty = param_tys.get(i).unwrap_or(&Type::Any);
                        res.push(self.coerce(arg_ops[i].clone(), &arg_tys[i], p_ty, expr.span));
                    }
                    res
                };

                self.push_statement(
                    StatementKind::Assign(
                        call_result_local,
                        Rvalue::Call {
                            func,
                            args: final_args,
                        },
                    ),
                    expr.span,
                );

                if is_ffi_result {
                    let is_err_tmp = self.new_local(Type::Bool, None, false);
                    match &inner_ok_ty {
                        Type::Int => {
                            self.push_statement(
                                StatementKind::Assign(
                                    is_err_tmp,
                                    Rvalue::BinaryOp(
                                        crate::parser::BinOp::Eq,
                                        Operand::Copy(call_result_local),
                                        Operand::Constant(Constant::Int(-1)),
                                    ),
                                ),
                                expr.span,
                            );
                        }
                        Type::Ref(_) | Type::MutRef(_) | Type::Ptr(_) => {
                            self.push_statement(
                                StatementKind::Assign(
                                    is_err_tmp,
                                    Rvalue::BinaryOp(
                                        crate::parser::BinOp::Eq,
                                        Operand::Copy(call_result_local),
                                        Operand::Constant(Constant::Int(0)),
                                    ),
                                ),
                                expr.span,
                            );
                        }
                        _ => {
                            self.push_statement(
                                StatementKind::Assign(
                                    is_err_tmp,
                                    Rvalue::Use(Operand::Constant(Constant::Bool(false))),
                                ),
                                expr.span,
                            );
                        }
                    }

                    let ok_bb = self.new_block();
                    let err_bb = self.new_block();
                    let exit_bb = self.new_block();

                    if let Some(bb) = self.current_block {
                        self.terminate_block(
                            bb,
                            TerminatorKind::SwitchInt {
                                discr: Operand::Copy(is_err_tmp),
                                targets: vec![(1, err_bb)],
                                otherwise: ok_bb,
                            },
                            expr.span,
                        );
                    }

                    let result_tmp = self.new_tmp_for_expr(expr);

                    self.current_block = Some(ok_bb);
                    self.push_statement(
                        StatementKind::Assign(
                            result_tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_result_ok".to_string(),
                                )),
                                args: vec![Operand::Copy(call_result_local)],
                            },
                        ),
                        expr.span,
                    );
                    self.terminate_block(
                        ok_bb,
                        TerminatorKind::Goto { target: exit_bb },
                        expr.span,
                    );

                    self.current_block = Some(err_bb);
                    let err_str_tmp = self.new_local(Type::Str, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            err_str_tmp,
                            Rvalue::Use(Operand::Constant(Constant::Str(
                                "FFI call failed".to_string(),
                            ))),
                        ),
                        expr.span,
                    );
                    self.push_statement(
                        StatementKind::Assign(
                            result_tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_result_err".to_string(),
                                )),
                                args: vec![Operand::Copy(err_str_tmp)],
                            },
                        ),
                        expr.span,
                    );
                    self.terminate_block(
                        err_bb,
                        TerminatorKind::Goto { target: exit_bb },
                        expr.span,
                    );

                    self.current_block = Some(exit_bb);
                    return self.operand_for_local(result_tmp);
                }

                self.operand_for_local(call_result_local)
            }

            ExprKind::List(elems) => {
                let ops: Vec<Operand> = elems.iter().map(|e| self.lower_expr(e)).collect();
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::Aggregate(AggregateKind::List, ops)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Tuple(elems) => {
                let ops: Vec<Operand> = elems.iter().map(|e| self.lower_expr(e)).collect();
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::Aggregate(AggregateKind::Tuple, ops)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Set(elems) => {
                let ops: Vec<Operand> = elems.iter().map(|e| self.lower_expr(e)).collect();
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::Aggregate(AggregateKind::Set, ops)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Dict(pairs) => {
                let mut ops = Vec::new();
                for (k, v) in pairs {
                    ops.push(self.lower_expr(k));
                    ops.push(self.lower_expr(v));
                }
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::Aggregate(AggregateKind::Dict, ops)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Attr { obj, attr } => {
                if let ExprKind::Identifier(name) = &obj.kind {
                    let obj_ty = self.get_type(obj.id);
                    let is_struct_or_self =
                        matches!(obj_ty, Type::Struct(_, _) | Type::Any | Type::Var(_))
                            && self.lookup_var(name).is_some();
                    if !is_struct_or_self && obj_ty != Type::PyObject {
                        let mangled = format!("{}::{}", name, attr);
                        if let Some(local) = self.lookup_var(&mangled) {
                            let ty = self.current_locals[local.0].ty.clone();
                            return if ty.is_move_type() {
                                Operand::Move(local)
                            } else {
                                Operand::Copy(local)
                            };
                        }
                        if let Some(global_op) = self.globals.get(&mangled) {
                            return global_op.clone();
                        }
                        return Operand::Constant(Constant::Function(mangled));
                    }
                }

                let obj_ty = self.get_type(obj.id);
                if obj_ty == Type::PyObject {
                    let obj_op = self.lower_expr_as_copy(obj);
                    let tmp = self.new_local(Type::PyObject, None, true);
                    self.push_statement(
                        StatementKind::Assign(
                            tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_py_getattr".to_string(),
                                )),
                                args: vec![obj_op, Operand::Constant(Constant::Str(attr.clone()))],
                            },
                        ),
                        expr.span,
                    );
                    return self.operand_for_local(tmp);
                }

                let o = self.lower_expr_as_copy(obj);
                let tmp = self.new_tmp_for_expr_with_owning(expr, false);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::GetAttr(o, attr.clone())),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Index { obj, index } => {
                let obj_ty = self.get_type(obj.id);
                let mut current_obj_ty = obj_ty;
                while let Type::Ref(inner) | Type::MutRef(inner) = current_obj_ty {
                    current_obj_ty = *inner;
                }

                if current_obj_ty == Type::Str {
                    let o = self.lower_expr_as_copy(obj);
                    let i = self.lower_expr(index);
                    let tmp = self.new_local(Type::Str, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_str_get".to_string(),
                                )),
                                args: vec![o, i],
                            },
                        ),
                        expr.span,
                    );
                    return self.operand_for_local(tmp);
                }
                let o = self.lower_expr_as_copy(obj);
                let i = self.lower_expr(index);
                let tmp = self.new_tmp_for_expr_with_owning(expr, false);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::GetIndex(o, i)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::ListComp { elt, clauses } => {
                let ty = self.get_type(expr.id);
                self.lower_comprehension(
                    None,
                    Some(elt),
                    clauses,
                    AggregateKind::List,
                    expr.span,
                    ty,
                )
            }
            ExprKind::SetComp { elt, clauses } => {
                let ty = self.get_type(expr.id);
                self.lower_comprehension(
                    None,
                    Some(elt),
                    clauses,
                    AggregateKind::Set,
                    expr.span,
                    ty,
                )
            }
            ExprKind::DictComp {
                key,
                value,
                clauses,
            } => {
                let ty = self.get_type(expr.id);
                self.lower_comprehension(
                    Some((key, value)),
                    None,
                    clauses,
                    AggregateKind::Dict,
                    expr.span,
                    ty,
                )
            }
            ExprKind::Match {
                expr: match_expr,
                cases,
            } => {
                let discr_op = self.lower_expr(match_expr);
                let discr_local = match discr_op {
                    Operand::Copy(l) | Operand::Move(l) => l,
                    _ => {
                        let tmp = self.new_local(self.get_type(match_expr.id), None, false);
                        self.push_statement(
                            StatementKind::Assign(tmp, Rvalue::Use(discr_op)),
                            match_expr.span,
                        );
                        tmp
                    }
                };

                let exit_bb = self.new_block();
                let result_ty = self.get_type(expr.id);
                let result_tmp = self.new_local(result_ty, None, false);

                for case in cases {
                    let success_bb = self.new_block();
                    let failure_bb = self.new_block();

                    let match_ty = self.get_type(match_expr.id);
                    self.lower_pattern(
                        &case.pattern,
                        discr_local,
                        &match_ty,
                        success_bb,
                        failure_bb,
                        expr.span,
                    );

                    self.current_block = Some(success_bb);
                    self.enter_scope();

                    let mut last_op = Operand::Constant(Constant::None);
                    if case.body.is_empty() {
                        self.push_statement(
                            StatementKind::Assign(result_tmp, Rvalue::Use(last_op)),
                            expr.span,
                        );
                    } else {
                        for (i, stmt) in case.body.iter().enumerate() {
                            if i == case.body.len() - 1 {
                                if let StmtKind::ExprStmt(e) = &stmt.kind {
                                    last_op = self.lower_expr(e);
                                } else {
                                    self.lower_stmt(stmt);
                                }
                                self.push_statement(
                                    StatementKind::Assign(result_tmp, Rvalue::Use(last_op.clone())),
                                    stmt.span,
                                );
                            } else {
                                self.lower_stmt(stmt);
                            }
                        }
                    }

                    self.terminate_block(
                        self.current_block.unwrap(),
                        TerminatorKind::Goto { target: exit_bb },
                        expr.span,
                    );
                    self.leave_scope();

                    self.current_block = Some(failure_bb);
                }

                self.terminate_block(
                    self.current_block.unwrap(),
                    TerminatorKind::Goto { target: exit_bb },
                    expr.span,
                );
                self.current_block = Some(exit_bb);
                Operand::Copy(result_tmp)
            }
        }
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

    pub(super) fn lower_expr_as_copy(&mut self, expr: &Expr) -> Operand {
        let op = self.lower_expr(expr);
        match op {
            Operand::Move(l) => Operand::Copy(l),
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

        // Emit py-arg coercions
        let zipped: Vec<(Operand, Option<String>, usize)> = arg_ops
            .into_iter()
            .zip(arg_kw_names.into_iter())
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
