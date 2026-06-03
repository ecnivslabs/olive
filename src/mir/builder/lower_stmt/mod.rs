use super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{ExprKind, Stmt, StmtKind};
use crate::semantic::types::Type;
use crate::span::Span;

mod assign;
mod functions;
#[cfg(test)]
mod tests;

impl<'a> MirBuilder<'a> {
    /// Same `GlobalData`+`PtrStore` shape `const` uses, so reads resolve via
    /// `self.globals` instead of falling through to a bogus call-by-name.
    fn store_module_global(
        &mut self,
        name: &str,
        rval: Operand,
        ty: Type,
        is_mut: bool,
        span: Span,
    ) {
        if !is_mut && matches!(rval, Operand::Constant(_)) {
            self.globals.insert(name.to_string(), rval);
            return;
        }
        let global_name = name.to_string();
        if !self.global_vars.contains(&global_name) {
            self.global_vars.push(global_name.clone());
        }
        let global_op = Operand::Constant(Constant::GlobalData(global_name));
        self.globals.insert(name.to_string(), global_op.clone());

        let local = self.new_local(ty, None, false);
        self.push_statement(StatementKind::Assign(local, Rvalue::Use(rval)), span);
        self.push_statement(
            StatementKind::PtrStore(global_op, Operand::Copy(local)),
            span,
        );
    }

    /// True if expr is a single-element index into a list/tuple/set (yields a non-owning view).
    fn is_collection_index(&self, expr: &crate::parser::Expr) -> bool {
        let ExprKind::Index { obj, index } = &expr.kind else {
            return false;
        };
        if matches!(index.kind, ExprKind::Slice { .. }) {
            return false;
        }
        let mut ty = self.get_type(obj.id);
        while let Type::Ref(inner) | Type::MutRef(inner) = ty {
            ty = *inner;
        }
        matches!(ty, Type::List(_) | Type::Tuple(_) | Type::Set(_))
    }

    pub(super) fn lower_stmt(&mut self, stmt: &Stmt) {
        self.lower_stmt_with_tail(stmt, false);
    }

    pub(super) fn emit_defers(&mut self) {
        let defers = std::mem::take(&mut self.defer_stack);
        for expr in defers.iter().rev() {
            self.lower_expr(expr);
        }
        self.defer_stack = defers;

        let py_defers = std::mem::take(&mut self.py_exit_stack);
        for (exit_local, span) in py_defers.iter().rev() {
            self.emit_py_exit_call(*exit_local, *span);
        }
        self.py_exit_stack = py_defers;
    }

    fn emit_py_exit_call(&mut self, exit_method: Local, span: Span) {
        let none = Operand::Constant(Constant::None);
        let args_list = self.new_local(Type::List(Box::new(Type::Any)), None, true);
        self.push_statement(
            StatementKind::Assign(
                args_list,
                Rvalue::Aggregate(
                    AggregateKind::List,
                    vec![none.clone(), none.clone(), none.clone()],
                ),
            ),
            span,
        );
        let tmp = self.new_local(Type::PyObject, None, true);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_py_call".to_string())),
                    args: vec![Operand::Copy(exit_method), Operand::Copy(args_list)],
                },
            ),
            span,
        );
    }

    pub(super) fn lower_stmt_with_tail(&mut self, stmt: &Stmt, is_tail: bool) {
        if self.is_terminated() {
            return;
        }

        match &stmt.kind {
            StmtKind::Let {
                name,
                value,
                is_mut,
                type_ann,
                ..
            } => {
                let mut rval = self.lower_expr(value);
                let val_ty = self.get_type(value.id).clone();
                let ty = if let Some(ann) = type_ann {
                    self.resolve_type_expr(ann)
                } else {
                    val_ty.clone()
                };
                rval = self.coerce(rval, &val_ty, &ty, value.span);

                if self.at_module_scope() {
                    self.store_module_global(name, rval, ty, *is_mut, stmt.span);
                    return;
                }

                let local = self.declare_var(name.clone(), ty, *is_mut);
                // Collection index yields a non-owning view; taking ownership drops a live element.
                if self.is_collection_index(value) {
                    self.current_locals[local.0].is_owning = false;
                }
                self.push_statement(StatementKind::Assign(local, Rvalue::Use(rval)), stmt.span);
            }

            StmtKind::MultiLet {
                names,
                value,
                is_mut,
                starred,
                ..
            } => {
                if let Some(starred_idx) = starred {
                    let rval = self.lower_expr(value);
                    let (rhs_local, len_local, elem_ty) =
                        self.lower_starred_destructure_source(value, rval, names.len(), value.span);
                    for (i, name) in names.iter().enumerate() {
                        let op = self.starred_slot_operand(
                            rhs_local,
                            len_local,
                            &elem_ty,
                            i,
                            *starred_idx,
                            names.len(),
                            value.span,
                        );
                        let bind_ty = if i == *starred_idx {
                            Type::List(Box::new(elem_ty.clone()))
                        } else {
                            elem_ty.clone()
                        };
                        if self.at_module_scope() {
                            self.store_module_global(name, op, bind_ty, *is_mut, stmt.span);
                            continue;
                        }
                        let local = self.declare_var(name.clone(), bind_ty, *is_mut);
                        self.push_statement(
                            StatementKind::Assign(local, Rvalue::Use(op)),
                            stmt.span,
                        );
                    }
                    return;
                }
                let rval = self.lower_expr(value);
                let rhs_local = self.new_tmp_for_expr(value);
                self.push_statement(
                    StatementKind::Assign(rhs_local, Rvalue::Use(rval)),
                    value.span,
                );
                let rhs_ty = self.current_locals[rhs_local.0].ty.clone();
                for (i, name) in names.iter().enumerate() {
                    let idx_op = Operand::Constant(Constant::Int(i as i64));
                    let elem_ty = match &rhs_ty {
                        Type::PyObject => Type::PyObject,
                        Type::List(inner) => *inner.clone(),
                        Type::Tuple(elems) => elems.get(i).cloned().unwrap_or(Type::Any),
                        _ => Type::Any,
                    };
                    let elem_tmp = self.new_local(elem_ty.clone(), None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            elem_tmp,
                            Rvalue::GetIndex(Operand::Copy(rhs_local), idx_op, false),
                        ),
                        value.span,
                    );
                    if self.at_module_scope() {
                        self.store_module_global(
                            name,
                            Operand::Copy(elem_tmp),
                            elem_ty,
                            *is_mut,
                            stmt.span,
                        );
                        continue;
                    }
                    let local = self.declare_var(name.clone(), elem_ty, *is_mut);
                    self.push_statement(
                        StatementKind::Assign(local, Rvalue::Use(Operand::Copy(elem_tmp))),
                        stmt.span,
                    );
                }
            }

            StmtKind::Const { name, value, .. } => {
                let rval = self.lower_expr(value);
                if let Operand::Constant(_) = &rval {
                    self.globals.insert(name.clone(), rval);
                } else {
                    let global_name = name.clone();
                    if !self.global_vars.contains(&global_name) {
                        self.global_vars.push(global_name.clone());
                    }
                    let global_op = Operand::Constant(Constant::GlobalData(global_name));
                    self.globals.insert(name.clone(), global_op.clone());

                    let local = self.new_tmp_for_expr(value);
                    self.push_statement(StatementKind::Assign(local, Rvalue::Use(rval)), stmt.span);
                    self.push_statement(
                        StatementKind::PtrStore(global_op, Operand::Copy(local)),
                        stmt.span,
                    );
                }
            }

            StmtKind::MultiConst { names, value, .. } => {
                let rval = self.lower_expr(value);
                let rhs_local = self.new_tmp_for_expr(value);
                self.push_statement(
                    StatementKind::Assign(rhs_local, Rvalue::Use(rval)),
                    value.span,
                );
                let rhs_ty = self.current_locals[rhs_local.0].ty.clone();
                for (i, name) in names.iter().enumerate() {
                    let idx_op = Operand::Constant(Constant::Int(i as i64));
                    let elem_ty = match &rhs_ty {
                        Type::PyObject => Type::PyObject,
                        Type::List(inner) => *inner.clone(),
                        Type::Tuple(elems) => elems.get(i).cloned().unwrap_or(Type::Any),
                        _ => Type::Any,
                    };
                    let elem_tmp = self.new_local(elem_ty, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            elem_tmp,
                            Rvalue::GetIndex(Operand::Copy(rhs_local), idx_op, false),
                        ),
                        value.span,
                    );

                    let global_name = name.clone();
                    if !self.global_vars.contains(&global_name) {
                        self.global_vars.push(global_name.clone());
                    }
                    let global_op = Operand::Constant(Constant::GlobalData(global_name));
                    self.globals.insert(name.clone(), global_op.clone());

                    self.push_statement(
                        StatementKind::PtrStore(global_op, Operand::Copy(elem_tmp)),
                        stmt.span,
                    );
                }
            }

            StmtKind::ExprStmt(expr) => {
                if is_tail {
                    let mut rval = self.lower_expr(expr);
                    let expr_ty = self.get_type(expr.id).clone();
                    let ret_ty = self.current_locals[0].ty.clone();
                    rval = self.coerce(rval, &expr_ty, &ret_ty, expr.span);
                    let exclude = match rval {
                        Operand::Copy(l) | Operand::Move(l) => Some(l),
                        _ => None,
                    };
                    self.push_statement(
                        StatementKind::Assign(Local(0), Rvalue::Use(rval)),
                        expr.span,
                    );
                    if let Some(bb) = self.current_block {
                        self.emit_open_scope_drops(0, exclude);
                        self.emit_defers();
                        self.terminate_block(bb, TerminatorKind::Return, expr.span);
                    }
                    self.current_block = Some(self.new_block());
                } else {
                    let rval = self.lower_expr(expr);
                    let tmp = self.new_local(Type::Any, None, true);
                    self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(rval)), expr.span);
                }
            }

            StmtKind::Assign { target, value } => {
                self.lower_assign(target, value);
            }

            StmtKind::AugAssign { target, op, value } => {
                let bin_op = match op {
                    crate::parser::AugOp::Add => crate::parser::BinOp::Add,
                    crate::parser::AugOp::Sub => crate::parser::BinOp::Sub,
                    crate::parser::AugOp::Mul => crate::parser::BinOp::Mul,
                    crate::parser::AugOp::Div => crate::parser::BinOp::Div,
                    crate::parser::AugOp::Mod => crate::parser::BinOp::Mod,
                    crate::parser::AugOp::Pow => crate::parser::BinOp::Pow,
                    crate::parser::AugOp::Shl => crate::parser::BinOp::Shl,
                    crate::parser::AugOp::Shr => crate::parser::BinOp::Shr,
                    crate::parser::AugOp::BitOr => crate::parser::BinOp::BitOr,
                    crate::parser::AugOp::BitAnd => crate::parser::BinOp::BitAnd,
                    crate::parser::AugOp::BitXor => crate::parser::BinOp::BitXor,
                };
                let target_ty = self.get_type(target.id).clone();
                let lhs_op = self.lower_expr(target);
                let rhs_op = self.lower_expr(value);
                let tmp = self.new_local(Type::Any, None, true);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::BinaryOp(bin_op, lhs_op, rhs_op)),
                    stmt.span,
                );

                match &target.kind {
                    ExprKind::Identifier(name) => {
                        if let Some(local) = self.lookup_var(name) {
                            self.push_statement(
                                StatementKind::Assign(local, Rvalue::Use(Operand::Copy(tmp))),
                                stmt.span,
                            );
                        } else if let Some(Operand::Constant(Constant::GlobalData(_))) =
                            self.globals.get(name)
                        {
                            self.store_module_global(
                                name,
                                Operand::Copy(tmp),
                                target_ty.clone(),
                                true,
                                stmt.span,
                            );
                        }
                    }
                    ExprKind::Attr { obj, attr } => {
                        let obj_ty = self.get_type(obj.id).clone();
                        let obj_op = self.lower_expr_as_copy(obj);
                        if obj_ty.is_py_value() {
                            let rval =
                                self.emit_to_py_arg(Operand::Copy(tmp), &target_ty, stmt.span);
                            let dummy = self.new_local(Type::Any, None, false);
                            self.push_statement(
                                StatementKind::Assign(
                                    dummy,
                                    Rvalue::Call {
                                        func: Operand::Constant(Constant::Function(
                                            "__olive_py_setattr".to_string(),
                                        )),
                                        args: vec![
                                            obj_op,
                                            Operand::Constant(Constant::Str(attr.clone())),
                                            rval,
                                        ],
                                    },
                                ),
                                stmt.span,
                            );
                        } else {
                            self.push_statement(
                                StatementKind::SetAttr(obj_op, attr.clone(), Operand::Copy(tmp)),
                                stmt.span,
                            );
                        }
                    }
                    ExprKind::Index { obj, index } => {
                        let obj_ty = self.get_type(obj.id).clone();
                        let obj_op = self.lower_expr_as_copy(obj);
                        let idx_op = self.lower_expr(index);
                        if obj_ty.is_py_value() {
                            let rval =
                                self.emit_to_py_arg(Operand::Copy(tmp), &target_ty, stmt.span);
                            let dummy = self.new_local(Type::Any, None, false);
                            self.push_statement(
                                StatementKind::Assign(
                                    dummy,
                                    Rvalue::Call {
                                        func: Operand::Constant(Constant::Function(
                                            "__olive_py_setitem".to_string(),
                                        )),
                                        args: vec![obj_op, idx_op, rval],
                                    },
                                ),
                                stmt.span,
                            );
                        } else {
                            self.push_statement(
                                StatementKind::SetIndex(obj_op, idx_op, Operand::Copy(tmp), false),
                                stmt.span,
                            );
                        }
                    }
                    ExprKind::Deref(ptr_expr) => {
                        let ptr_op = self.lower_expr(ptr_expr);
                        self.push_statement(
                            StatementKind::PtrStore(ptr_op, Operand::Copy(tmp)),
                            stmt.span,
                        );
                    }
                    _ => {}
                }
            }

            StmtKind::Return(Some(expr)) => {
                let mut rval = self.lower_expr(expr);
                let expr_ty = self.get_type(expr.id).clone();
                let ret_ty = self.current_locals[0].ty.clone();
                rval = self.coerce(rval, &expr_ty, &ret_ty, stmt.span);
                let exclude = match rval {
                    Operand::Copy(l) | Operand::Move(l) => Some(l),
                    _ => None,
                };
                self.push_statement(
                    StatementKind::Assign(Local(0), Rvalue::Use(rval)),
                    stmt.span,
                );
                if let Some(bb) = self.current_block {
                    self.emit_open_loop_iter_frees();
                    self.emit_open_scope_drops(0, exclude);
                    if let Some((_, _, exit_bb)) = self.memo_context {
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: exit_bb },
                            stmt.span,
                        );
                    } else {
                        self.emit_defers();
                        self.terminate_block(bb, TerminatorKind::Return, stmt.span);
                    }
                }
                self.current_block = Some(self.new_block());
            }

            StmtKind::Return(None) => {
                if let Some(bb) = self.current_block {
                    self.emit_open_loop_iter_frees();
                    self.emit_open_scope_drops(0, None);
                    if let Some((_, _, exit_bb)) = self.memo_context {
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: exit_bb },
                            stmt.span,
                        );
                    } else {
                        self.emit_defers();
                        self.terminate_block(bb, TerminatorKind::Return, stmt.span);
                    }
                }
                self.current_block = Some(self.new_block());
            }

            StmtKind::Defer(expr) => {
                self.defer_stack.push(expr.clone());
            }

            StmtKind::If {
                condition,
                then_body,
                elif_clauses,
                else_body,
            } => {
                self.lower_if(condition, then_body, elif_clauses, else_body, is_tail);
            }

            StmtKind::While {
                condition,
                body,
                else_body,
            } => {
                self.lower_while(condition, body, else_body);
            }

            StmtKind::For {
                target,
                iter,
                body,
                else_body,
            } => {
                self.lower_for(target, iter, body, else_body);
            }

            StmtKind::With { items, body } => {
                let mut exit_calls = Vec::new();
                let py_exit_start = self.py_exit_stack.len();

                for (i, item) in items.iter().enumerate() {
                    let ctx_op = self.lower_expr(&item.context_expr);
                    let ctx_ty = self.get_type(item.context_expr.id).clone();

                    if let Type::Struct(name, _, _) = ctx_ty {
                        let has_drop = self
                            .functions
                            .iter()
                            .any(|f| f.name == format!("{}::__drop__", name))
                            || self
                                .generic_fns
                                .contains_key(&format!("{}::__drop__", name));

                        let enter_mangled = format!("{}::__enter__", name);
                        let has_enter = self.functions.iter().any(|f| f.name == *enter_mangled)
                            || self.generic_fns.contains_key(&enter_mangled);

                        let ctx_tmp = self.new_tmp_for_expr(&item.context_expr);
                        self.push_statement(
                            StatementKind::Assign(ctx_tmp, Rvalue::Use(ctx_op.clone())),
                            item.context_expr.span,
                        );

                        if has_enter {
                            let exit_mangled = format!("{}::__exit__", name);

                            let enter_func = Operand::Constant(Constant::Function(enter_mangled));
                            let enter_rval = Rvalue::Call {
                                func: enter_func,
                                args: vec![Operand::Copy(ctx_tmp)],
                            };

                            if let Some(alias_expr) = &item.alias
                                && let crate::parser::ExprKind::Identifier(alias_name) =
                                    &alias_expr.kind
                            {
                                let alias_ty = self.get_type(alias_expr.id).clone();
                                let local = self.declare_var(alias_name.clone(), alias_ty, false);
                                if has_drop {
                                    self.current_locals[local.0].is_owning = false;
                                }
                                self.push_statement(
                                    StatementKind::Assign(local, enter_rval),
                                    item.context_expr.span,
                                );
                            } else {
                                let tmp = self.new_local(Type::Any, None, false);
                                self.push_statement(
                                    StatementKind::Assign(tmp, enter_rval),
                                    item.context_expr.span,
                                );
                            }

                            let dummy_ident = crate::parser::Expr {
                                id: 0,
                                kind: crate::parser::ExprKind::Identifier(exit_mangled),
                                span: item.context_expr.span,
                            };

                            let ctx_name = format!("$ctx_{}", i);

                            let call_expr = crate::parser::Expr {
                                id: 0,
                                kind: crate::parser::ExprKind::Call {
                                    callee: Box::new(dummy_ident),
                                    args: vec![crate::parser::CallArg::Positional(
                                        crate::parser::Expr {
                                            id: item.context_expr.id,
                                            kind: crate::parser::ExprKind::Identifier(
                                                ctx_name.clone(),
                                            ),
                                            span: item.context_expr.span,
                                        },
                                    )],
                                },
                                span: item.context_expr.span,
                            };

                            exit_calls.push((ctx_tmp, call_expr, ctx_name, has_drop));
                        } else {
                            // No __enter__: bind alias directly, defer close() if available
                            let close_mangled = format!("{}::close", name);
                            let has_close = self.functions.iter().any(|f| f.name == *close_mangled);

                            if let Some(alias_expr) = &item.alias
                                && let crate::parser::ExprKind::Identifier(alias_name) =
                                    &alias_expr.kind
                            {
                                let alias_ty = self.get_type(alias_expr.id).clone();
                                let local = self.declare_var(alias_name.clone(), alias_ty, false);
                                if has_drop {
                                    self.current_locals[local.0].is_owning = false;
                                }
                                self.push_statement(
                                    StatementKind::Assign(
                                        local,
                                        Rvalue::Use(Operand::Copy(ctx_tmp)),
                                    ),
                                    item.context_expr.span,
                                );
                            }

                            if has_close {
                                let ctx_name = format!("$ctx_{}", i);
                                let close_ident = crate::parser::Expr {
                                    id: 0,
                                    kind: crate::parser::ExprKind::Identifier(close_mangled),
                                    span: item.context_expr.span,
                                };
                                let close_call = crate::parser::Expr {
                                    id: 0,
                                    kind: crate::parser::ExprKind::Call {
                                        callee: Box::new(close_ident),
                                        args: vec![crate::parser::CallArg::Positional(
                                            crate::parser::Expr {
                                                id: item.context_expr.id,
                                                kind: crate::parser::ExprKind::Identifier(
                                                    ctx_name.clone(),
                                                ),
                                                span: item.context_expr.span,
                                            },
                                        )],
                                    },
                                    span: item.context_expr.span,
                                };
                                // Flushed at the `with` block's own end below,
                                // same as the `__enter__` path -- pushing onto
                                // `self.defer_stack` directly here would instead
                                // wait for the *function's* return (that stack
                                // backs `defer` statements), so `close()` would
                                // run too late whenever code follows the block.
                                exit_calls.push((ctx_tmp, close_call, ctx_name, has_drop));
                            }
                        }
                    } else if ctx_ty.is_py_value() {
                        let ctx_tmp = self.new_tmp_for_expr(&item.context_expr);
                        self.push_statement(
                            StatementKind::Assign(ctx_tmp, Rvalue::Use(ctx_op.clone())),
                            item.context_expr.span,
                        );

                        let enter_method = self.new_local(Type::PyObject, None, true);
                        self.push_statement(
                            StatementKind::Assign(
                                enter_method,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_py_getattr".to_string(),
                                    )),
                                    args: vec![
                                        Operand::Copy(ctx_tmp),
                                        Operand::Constant(Constant::Str("__enter__".to_string())),
                                    ],
                                },
                            ),
                            item.context_expr.span,
                        );

                        let empty_list =
                            self.new_local(Type::List(Box::new(Type::Any)), None, true);
                        self.push_statement(
                            StatementKind::Assign(
                                empty_list,
                                Rvalue::Aggregate(AggregateKind::List, vec![]),
                            ),
                            item.context_expr.span,
                        );

                        let enter_result = self.new_local(Type::PyObject, None, true);
                        self.push_statement(
                            StatementKind::Assign(
                                enter_result,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_py_call".to_string(),
                                    )),
                                    args: vec![
                                        Operand::Copy(enter_method),
                                        Operand::Copy(empty_list),
                                    ],
                                },
                            ),
                            item.context_expr.span,
                        );

                        if let Some(alias_expr) = &item.alias {
                            if let crate::parser::ExprKind::Identifier(alias_name) =
                                &alias_expr.kind
                            {
                                let alias_ty = self.get_type(alias_expr.id).clone();
                                let local = self.declare_var(alias_name.clone(), alias_ty, false);
                                self.push_statement(
                                    StatementKind::Assign(
                                        local,
                                        Rvalue::Use(Operand::Copy(enter_result)),
                                    ),
                                    item.context_expr.span,
                                );
                            }
                        } else {
                            let tmp = self.new_local(Type::Any, None, false);
                            self.push_statement(
                                StatementKind::Assign(
                                    tmp,
                                    Rvalue::Use(Operand::Copy(enter_result)),
                                ),
                                item.context_expr.span,
                            );
                        }

                        let exit_method = self.new_local(Type::PyObject, None, true);
                        self.push_statement(
                            StatementKind::Assign(
                                exit_method,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_py_getattr".to_string(),
                                    )),
                                    args: vec![
                                        Operand::Copy(ctx_tmp),
                                        Operand::Constant(Constant::Str("__exit__".to_string())),
                                    ],
                                },
                            ),
                            item.context_expr.span,
                        );

                        self.py_exit_stack
                            .push((exit_method, item.context_expr.span));
                    }
                }

                for (ctx_tmp, call_expr, ctx_name, _) in &exit_calls {
                    self.var_map
                        .last_mut()
                        .unwrap()
                        .insert(ctx_name.clone(), *ctx_tmp);
                    self.defer_stack.push(call_expr.clone());
                }

                for s in body {
                    self.lower_stmt(s);
                }

                for _ in 0..exit_calls.len() {
                    if let Some(expr) = self.defer_stack.pop() {
                        self.lower_expr(&expr);
                    }
                }

                for (ctx_tmp, _, _, has_drop) in &exit_calls {
                    if *has_drop {
                        self.current_locals[ctx_tmp.0].is_owning = false;
                    }
                }

                while self.py_exit_stack.len() > py_exit_start {
                    if let Some((exit_method, span)) = self.py_exit_stack.pop() {
                        self.emit_py_exit_call(exit_method, span);
                    }
                }
            }

            StmtKind::Break => {
                if let Some(ctx) = self.loop_stack.last() {
                    let exit = ctx.exit;
                    let depth = ctx.scope_depth;
                    if let Some(bb) = self.current_block {
                        self.emit_open_scope_drops(depth, None);
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: exit },
                            Span::default(),
                        );
                    }
                    self.current_block = Some(self.new_block());
                }
            }

            StmtKind::Continue => {
                if let Some(ctx) = self.loop_stack.last() {
                    let header = ctx.header;
                    let depth = ctx.scope_depth;
                    if let Some(bb) = self.current_block {
                        self.emit_open_scope_drops(depth, None);
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: header },
                            Span::default(),
                        );
                    }
                    self.current_block = Some(self.new_block());
                }
            }

            StmtKind::Fn { type_params, .. } => {
                if type_params.is_empty() {
                    self.lower_fn_def(stmt);
                }
            }

            StmtKind::Trait { .. } => {}

            StmtKind::TypeAlias { .. } => {}

            StmtKind::Impl {
                type_params,
                type_name,
                body,
                ..
            } => {
                let type_base_name = Self::type_expr_base_name(type_name);
                if !type_params.is_empty() {
                    for s in body {
                        if let StmtKind::Fn {
                            name: fn_name,
                            type_params: fn_type_params,
                            params,
                            return_type,
                            body: fn_body,
                            decorators,
                            is_async,
                        } = &s.kind
                        {
                            let mangled_name = format!("{}::{}", type_base_name, fn_name);
                            let mut merged_type_params = type_params.clone();
                            for tp in fn_type_params {
                                if !merged_type_params.contains(tp) {
                                    merged_type_params.push(tp.clone());
                                }
                            }
                            let generic_fn = crate::parser::Stmt {
                                kind: StmtKind::Fn {
                                    name: mangled_name.clone(),
                                    type_params: merged_type_params,
                                    params: params.clone(),
                                    return_type: return_type.clone(),
                                    body: fn_body.clone(),
                                    decorators: decorators.clone(),
                                    is_async: *is_async,
                                },
                                span: s.span,
                            };
                            self.generic_fns.insert(mangled_name, generic_fn);
                        }
                    }
                    return;
                }
                for s in body {
                    if let StmtKind::Fn {
                        name: fn_name,
                        type_params,
                        ..
                    } = &s.kind
                    {
                        if !type_params.is_empty() {
                            continue;
                        }
                        let mangled_name = format!("{}::{}", type_base_name, fn_name);
                        let mut impl_stmt = s.clone();
                        if let StmtKind::Fn {
                            name: ref mut n, ..
                        } = impl_stmt.kind
                        {
                            *n = mangled_name;
                        }
                        self.lower_fn_def(&impl_stmt);
                    } else if let StmtKind::Const {
                        name: const_name,
                        value,
                        ..
                    } = &s.kind
                    {
                        let mangled = format!("{}::{}", type_base_name, const_name);
                        let rval = self.lower_expr(value);
                        if let Operand::Constant(_) = &rval {
                            self.globals.insert(mangled, rval);
                        }
                    }
                }
            }

            StmtKind::Assert { test, msg } => {
                let raw = self.lower_expr(test);
                let test_ty = self.get_type(test.id);
                let test_op = self.truthify(raw, &test_ty, test.span);
                if let Some(m) = msg {
                    self.lower_expr(m);
                }
                let pass_bb = self.new_block();
                let fail_bb = self.new_block();
                if let Some(bb) = self.current_block {
                    self.terminate_block(
                        bb,
                        TerminatorKind::SwitchInt {
                            discr: test_op,
                            targets: vec![(1, pass_bb)],
                            otherwise: fail_bb,
                        },
                        test.span,
                    );
                }
                self.terminate_block(fail_bb, TerminatorKind::Unreachable, Span::default());
                self.current_block = Some(pass_bb);
            }

            StmtKind::Struct {
                name,
                fields,
                type_params,
                body,
                ..
            } => {
                if !type_params.is_empty() {
                    let init_name = format!("{}::__init__", name);
                    let mut params = vec![crate::parser::Param {
                        name: "self".to_string(),
                        type_ann: None,
                        is_mut: false,
                        default: None,
                        kind: crate::parser::ParamKind::Regular,
                        span: Span::default(),
                    }];
                    for f in fields {
                        params.push(crate::parser::Param {
                            name: f.name.clone(),
                            type_ann: f.type_ann.clone(),
                            is_mut: false,
                            default: None,
                            kind: crate::parser::ParamKind::Regular,
                            span: Span::default(),
                        });
                    }

                    self.generic_fns.insert(init_name, stmt.clone());
                    return;
                }

                let has_user_init = body.iter().any(|s| {
                    if let StmtKind::Fn { name: fn_name, .. } = &s.kind {
                        fn_name == "__init__"
                    } else {
                        false
                    }
                });

                if !has_user_init {
                    let init_name = format!("{}::__init__", name);
                    let n_params = fields.len() + 1;

                    let saved_name = std::mem::take(&mut self.current_name);
                    let saved_locals = std::mem::take(&mut self.current_locals);
                    let saved_blocks = std::mem::take(&mut self.current_blocks);
                    let saved_block = self.current_block.take();
                    let saved_var_map = std::mem::take(&mut self.var_map);
                    let saved_loop_stack = std::mem::take(&mut self.loop_stack);
                    let saved_scope_locals_init = std::mem::take(&mut self.scope_locals);
                    let saved_arg_count = self.current_arg_count;

                    self.start_function(init_name, n_params, Type::Null);

                    let self_local = self.new_local(
                        Type::Struct(name.clone(), Vec::new(), false),
                        Some("self".to_string()),
                        false,
                    );
                    self.current_locals[self_local.0].is_owning = false;
                    let mut field_locals = Vec::new();
                    for field in fields {
                        let field_ty = field
                            .type_ann
                            .as_ref()
                            .map(|ann| self.resolve_type_expr(ann))
                            .unwrap_or(Type::Any);
                        let fl = self.new_local(field_ty, Some(field.name.clone()), false);
                        self.current_locals[fl.0].is_owning = false;
                        field_locals.push((field.name.clone(), fl));
                    }

                    for (field_name, fl) in &field_locals {
                        self.push_statement(
                            StatementKind::SetAttr(
                                Operand::Copy(self_local),
                                field_name.clone(),
                                Operand::Copy(*fl),
                            ),
                            Span::default(),
                        );
                    }

                    if let Some(bb) = self.current_block {
                        self.terminate_block(bb, TerminatorKind::Return, Span::default());
                    }

                    self.finish_function();

                    self.current_name = saved_name;
                    self.current_locals = saved_locals;
                    self.current_blocks = saved_blocks;
                    self.current_block = saved_block;
                    self.var_map = saved_var_map;
                    self.loop_stack = saved_loop_stack;
                    self.scope_locals = saved_scope_locals_init;
                    self.current_arg_count = saved_arg_count;
                }

                for s in body {
                    if let StmtKind::Fn { name: fn_name, .. } = &s.kind {
                        let mangled = format!("{}::{}", name, fn_name);
                        let mut impl_stmt = s.clone();
                        if let StmtKind::Fn {
                            name: ref mut n, ..
                        } = impl_stmt.kind
                        {
                            *n = mangled;
                        }
                        self.lower_fn_def(&impl_stmt);
                    } else if let StmtKind::Const {
                        name: const_name,
                        value,
                        ..
                    } = &s.kind
                    {
                        let mangled = format!("{}::{}", name, const_name);
                        let rval = self.lower_expr(value);
                        if let Operand::Constant(_) = &rval {
                            self.globals.insert(mangled, rval);
                        }
                    }
                }
            }

            StmtKind::Pass
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. }
            | StmtKind::NativeImport { .. } => {}

            StmtKind::PyImport { module, alias, .. } => {
                let module_op = Operand::Constant(Constant::Str(module.clone()));
                if self.current_name == "__main__" {
                    if !self.global_vars.contains(alias) {
                        self.global_vars.push(alias.clone());
                    }
                    let global_name = alias.clone();
                    self.globals.insert(
                        alias.clone(),
                        Operand::Constant(Constant::GlobalData(global_name.clone())),
                    );
                    let local = self.new_local(Type::PyObject, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            local,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_py_import".to_string(),
                                )),
                                args: vec![module_op],
                            },
                        ),
                        stmt.span,
                    );
                    self.push_statement(
                        StatementKind::PtrStore(
                            Operand::Constant(Constant::GlobalData(global_name)),
                            Operand::Copy(local),
                        ),
                        stmt.span,
                    );
                } else {
                    let local = self.declare_var(alias.clone(), Type::PyObject, false);
                    self.push_statement(
                        StatementKind::Assign(
                            local,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_py_import".to_string(),
                                )),
                                args: vec![module_op],
                            },
                        ),
                        stmt.span,
                    );
                }
            }

            StmtKind::UnsafeBlock(body) => {
                for s in body {
                    self.lower_stmt(s);
                }
            }

            StmtKind::Enum { name, variants, .. } => {
                for (i, variant) in variants.iter().enumerate() {
                    let mangled = format!("{}::{}", name, variant.name);
                    self.enum_variants.insert(mangled, (name.clone(), i));
                    self.enum_variants
                        .insert(variant.name.clone(), (name.clone(), i));
                }
            }
        }
    }
}
