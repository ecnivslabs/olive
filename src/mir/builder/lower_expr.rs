use super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{CallArg, Expr, ExprKind, StmtKind};
use crate::semantic::types::Type;

impl<'a> MirBuilder<'a> {
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
                    let op = self.lower_expr(e);
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
                let inner_op = self.lower_expr(inner);
                let tag_tmp = self.new_local(Type::Int, None, false);
                self.push_statement(
                    StatementKind::Assign(tag_tmp, Rvalue::GetTag(inner_op.clone())),
                    expr.span,
                );

                let success_bb = self.new_block();
                let error_bb = self.new_block();

                if let Some(bb) = self.current_block {
                    self.terminate_block(
                        bb,
                        TerminatorKind::SwitchInt {
                            discr: Operand::Copy(tag_tmp),
                            targets: vec![(0, success_bb)],
                            otherwise: error_bb,
                        },
                        expr.span,
                    );
                }

                self.current_block = Some(error_bb);
                self.push_statement(
                    StatementKind::Assign(Local(0), Rvalue::Use(inner_op.clone())),
                    expr.span,
                );
                self.terminate_block(error_bb, TerminatorKind::Return, expr.span);

                self.current_block = Some(success_bb);
                let payload_tmp = self.new_local(Type::Any, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        payload_tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_enum_get".to_string(),
                            )),
                            args: vec![inner_op, Operand::Constant(Constant::Int(0))],
                        },
                    ),
                    expr.span,
                );

                Operand::Copy(payload_tmp)
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
                for s in body {
                    self.lower_stmt(s);
                    if let StmtKind::ExprStmt(e) = &s.kind {
                        last_op = self.lower_expr(e);
                    }
                }
                self.leave_scope();
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
                } else if let Some(global_op) = self.globals.get(name) {
                    global_op.clone()
                } else {
                    Operand::Constant(Constant::Function(name.clone()))
                }
            }

            ExprKind::BinOp { left, op, right } => {
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

            ExprKind::Call { callee, args } => {
                let mut arg_ops = Vec::new();
                let mut arg_kw_names: Vec<Option<String>> = Vec::new();
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
                        }
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
                    }
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
                    if let ExprKind::Identifier(name) = &obj.kind {
                        let obj_ty = self.get_type(obj.id);
                        let is_struct_var = matches!(obj_ty, Type::Struct(_, _) | Type::Any)
                            && self.lookup_var(name).is_some();
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

                            let callee_op = Operand::Constant(Constant::Function(mangled));
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
                    let mut method_name = attr.clone();

                    if let Type::Struct(struct_name, type_args) = obj_ty {
                        let base_method_name = format!("{}::{}", struct_name, attr);
                        if !type_args.is_empty() {
                            method_name = self.monomorphize(&base_method_name, &type_args);
                        } else {
                            method_name = base_method_name;
                        }
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
                } else {
                    None
                };

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

                let tmp = self.new_tmp_for_expr(expr);
                let final_args = if let Some(name) = &call_fn_name {
                    self.pack_fn_call_args(name, &arg_ops, &arg_kw_names, expr.span)
                } else {
                    arg_ops
                };
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func,
                            args: final_args,
                        },
                    ),
                    expr.span,
                );
                self.operand_for_local(tmp)
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
                    let is_struct_or_self = matches!(obj_ty, Type::Struct(_, _) | Type::Any)
                        && self.lookup_var(name).is_some();
                    if !is_struct_or_self {
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
                let o = self.lower_expr_as_copy(obj);
                let tmp = self.new_tmp_for_expr(expr);
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
                let tmp = self.new_tmp_for_expr(expr);
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

    pub(super) fn lower_expr_as_copy(&mut self, expr: &Expr) -> Operand {
        let op = self.lower_expr(expr);
        match op {
            Operand::Move(l) => Operand::Copy(l),
            _ => op,
        }
    }
}
