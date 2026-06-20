use super::super::MirBuilder;
use crate::mir::ir::*;
use crate::parser::Expr;
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    pub(super) fn lower_binop_expr(
        &mut self,
        left: &Expr,
        op: &crate::parser::BinOp,
        right: &Expr,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let r_ty = self.get_type(right.id).clone();

        // `null` in an `Any` is a boxed sentinel, not a bare 0, so test it via
        // the runtime null check (negated for `!=`).
        if matches!(op, crate::parser::BinOp::Eq | crate::parser::BinOp::NotEq) {
            let l_ty = self.get_type(left.id);
            let any_operand = match (&l_ty, &r_ty) {
                (Type::Any, Type::Null) => Some(left),
                (Type::Null, Type::Any) => Some(right),
                _ => None,
            };
            if let Some(operand) = any_operand {
                let v = self.lower_expr_as_copy(operand);
                let is_null = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        is_null,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_any_is_null".to_string(),
                            )),
                            args: vec![v],
                        },
                    ),
                    span,
                );
                if matches!(op, crate::parser::BinOp::Eq) {
                    return self.operand_for_local(is_null);
                }
                let neg = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        neg,
                        Rvalue::UnaryOp(crate::parser::UnaryOp::Not, Operand::Copy(is_null)),
                    ),
                    span,
                );
                return self.operand_for_local(neg);
            }
        }

        if r_ty == Type::Str && matches!(op, crate::parser::BinOp::In | crate::parser::BinOp::NotIn)
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
                span,
            );

            if matches!(op, crate::parser::BinOp::In) {
                return self.operand_for_local(call_tmp);
            } else {
                let not_tmp = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        not_tmp,
                        Rvalue::UnaryOp(crate::parser::UnaryOp::Not, Operand::Copy(call_tmp)),
                    ),
                    span,
                );
                return self.operand_for_local(not_tmp);
            }
        }

        if matches!(op, crate::parser::BinOp::And | crate::parser::BinOp::Or) {
            let tmp = self.new_local(self.get_type(expr_id), None, false);
            let l = self.lower_expr(left);
            // Result is the original left value, but branch on its truthiness so
            // a boxed `Any` is tested by value, not by its pointer word.
            let l_ty = self.get_type(left.id);
            self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(l.clone())), span);
            let l_disc = self.truthify(l, &l_ty, span);

            let rhs_bb = self.new_block();
            let merge_bb = self.new_block();

            if let Some(bb) = self.current_block {
                if matches!(op, crate::parser::BinOp::And) {
                    self.terminate_block(
                        bb,
                        TerminatorKind::SwitchInt {
                            discr: l_disc,
                            targets: vec![(1, rhs_bb)],
                            otherwise: merge_bb,
                        },
                        span,
                    );
                } else {
                    self.terminate_block(
                        bb,
                        TerminatorKind::SwitchInt {
                            discr: l_disc,
                            targets: vec![(0, rhs_bb)],
                            otherwise: merge_bb,
                        },
                        span,
                    );
                }
            }

            self.current_block = Some(rhs_bb);
            let r = self.lower_expr(right);
            self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(r)), span);
            if let Some(bb) = self.current_block {
                self.terminate_block(bb, TerminatorKind::Goto { target: merge_bb }, span);
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
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::BinaryOp(op.clone(), l, r)),
            span,
        );
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_unary_op_expr(
        &mut self,
        op: &crate::parser::UnaryOp,
        operand: &Expr,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let o = self.lower_expr(operand);
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::UnaryOp(op.clone(), o)),
            span,
        );
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_cast_expr(
        &mut self,
        operand: &Expr,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let op = self.lower_expr(operand);
        let tmp = self.new_local(self.get_type(expr_id), None, false);

        let target_ty = self.get_type(expr_id);
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::Cast(op, target_ty)),
            span,
        );
        self.operand_for_local(tmp)
    }
}
