use super::super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{Expr, StmtKind};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    pub(super) fn lower_ternary_expr(
        &mut self,
        cond: &Expr,
        then: &Expr,
        otherwise: &Expr,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        let c = self.lower_expr(cond);
        let c_ty = self.get_type(cond.id);
        let disc = self.truthify(c, &c_ty, span);

        let then_bb = self.new_block();
        let else_bb = self.new_block();
        let merge_bb = self.new_block();

        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::SwitchInt {
                    discr: disc,
                    targets: vec![(0, else_bb)],
                    otherwise: then_bb,
                },
                span,
            );
        }

        self.current_block = Some(then_bb);
        let t = self.lower_expr(then);
        self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(t)), span);
        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: merge_bb }, span);
        }

        self.current_block = Some(else_bb);
        let o = self.lower_expr(otherwise);
        self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(o)), span);
        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: merge_bb }, span);
        }

        self.current_block = Some(merge_bb);
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_try_expr(&mut self, inner: &Expr, span: Span, _expr_id: usize) -> Operand {
        if self.is_py_call(inner) {
            return self.lower_try_py(inner, span);
        }

        let inner_op = self.lower_expr_as_copy(inner);
        let inner_ty = self.get_type(inner.id);

        let is_error = |ty: &Type| -> bool {
            match ty {
                Type::Struct(name, _, _) | Type::Enum(name, _) => {
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
                        Type::Struct(name, _, _) => {
                            error_type_id = Self::struct_type_id(name);
                            break;
                        }
                        Type::Enum(name, _) => {
                            error_type_id = Self::enum_type_id(name);
                            break;
                        }
                        _ => {}
                    }
                }
            }
        } else if is_error(&inner_ty) {
            match &inner_ty {
                Type::Struct(name, _, _) => {
                    error_type_id = Self::struct_type_id(name);
                }
                Type::Enum(name, _) => {
                    error_type_id = Self::enum_type_id(name);
                }
                _ => {}
            }
        } else if inner_ty.is_py_value() {
            error_type_id = Self::enum_type_id("Error");
        }

        if error_type_id == -1 {
            return inner_op;
        }

        let type_id_tmp = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(type_id_tmp, Rvalue::GetTypeId(inner_op.clone())),
            span,
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
            span,
        );

        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::SwitchInt {
                    discr: Operand::Copy(is_err_tmp),
                    targets: vec![(1, error_bb)],
                    otherwise: success_bb,
                },
                span,
            );
        }

        self.current_block = Some(error_bb);
        self.push_statement(
            StatementKind::Assign(Local(0), Rvalue::Use(inner_op.clone())),
            span,
        );
        self.emit_defers();
        self.terminate_block(error_bb, TerminatorKind::Return, span);

        self.current_block = Some(success_bb);

        inner_op
    }

    pub(super) fn lower_try_py(&mut self, inner: &Expr, span: Span) -> Operand {
        let result_op = self.lower_py_call_safe(inner);

        let is_ok_tmp = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                is_ok_tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_result_is_ok".to_string())),
                    args: vec![result_op.clone()],
                },
            ),
            span,
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
                span,
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
            span,
        );

        // Prefix the Python traceback with the Olive call site, then wrap it in
        // the built-in `Error(msg)` enum variant so the result is matchable as
        // `case Error(e)` and carries its source location.
        let located_msg = self.prepend_call_loc(err_msg_tmp, span);
        let err_tmp = self.new_local(Type::Enum("Error".to_string(), vec![]), None, false);
        self.push_statement(
            StatementKind::Assign(
                err_tmp,
                Rvalue::Aggregate(
                    AggregateKind::EnumVariant(Self::enum_type_id("Error"), 0),
                    vec![Operand::Copy(located_msg)],
                ),
            ),
            span,
        );

        self.push_statement(
            StatementKind::Assign(Local(0), Rvalue::Use(Operand::Copy(err_tmp))),
            span,
        );
        self.emit_defers();
        self.terminate_block(error_bb, TerminatorKind::Return, span);

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
            span,
        );

        Operand::Copy(payload_tmp)
    }

    pub(super) fn lower_await_expr(&mut self, inner: &Expr, expr_id: usize) -> Operand {
        let inner_op = self.lower_expr(inner);
        let result_ty = self.get_type(expr_id);
        let tmp = self.new_local(result_ty, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_await".to_string())),
                    args: vec![inner_op],
                },
            ),
            inner.span,
        );
        Operand::Copy(tmp)
    }

    pub(super) fn lower_async_block_expr(
        &mut self,
        body: &[crate::parser::Stmt],
        span: Span,
    ) -> Operand {
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
                    func: Operand::Constant(Constant::Function("__olive_make_future".to_string())),
                    args: vec![last_op],
                },
            ),
            span,
        );
        self.leave_scope();
        Operand::Copy(tmp)
    }

    pub(super) fn lower_match_expr(
        &mut self,
        match_expr: &Expr,
        cases: &[crate::parser::MatchCase],
        span: Span,
        expr_id: usize,
    ) -> Operand {
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
        let result_ty = self.get_type(expr_id);
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
                span,
            );

            self.current_block = Some(success_bb);
            self.enter_scope();

            // A guard runs with the pattern's bindings in scope; if it is false the
            // arm is skipped and matching falls through to the next case.
            if let Some(guard) = &case.guard {
                let g = self.lower_expr(guard);
                let g_ty = self.get_type(guard.id);
                let disc = self.truthify(g, &g_ty, span);
                let body_bb = self.new_block();
                self.terminate_block(
                    self.current_block.unwrap(),
                    TerminatorKind::SwitchInt {
                        discr: disc,
                        targets: vec![(0, failure_bb)],
                        otherwise: body_bb,
                    },
                    span,
                );
                self.current_block = Some(body_bb);
            }

            let mut last_op = Operand::Constant(Constant::None);
            if case.body.is_empty() {
                self.push_statement(
                    StatementKind::Assign(result_tmp, Rvalue::Use(last_op)),
                    span,
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
                span,
            );
            self.leave_scope();

            self.current_block = Some(failure_bb);
        }

        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: exit_bb },
            span,
        );
        self.current_block = Some(exit_bb);
        Operand::Copy(result_tmp)
    }
}
