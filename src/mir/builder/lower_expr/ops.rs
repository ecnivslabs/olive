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
            // PyObject None is a singleton, not bare 0; detect syntactically since type widens.
            let is_none_lit = |e: &Expr| matches!(e.kind, crate::parser::ast::ExprKind::Null);
            let py_operand = if is_none_lit(right) && matches!(l_ty, Type::PyObject) {
                Some(left)
            } else if is_none_lit(left) && matches!(r_ty, Type::PyObject) {
                Some(right)
            } else {
                None
            };
            if let Some(operand) = py_operand {
                let v = self.lower_expr_as_copy(operand);
                let is_none = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        is_none,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_py_is_none".to_string(),
                            )),
                            args: vec![v],
                        },
                    ),
                    span,
                );
                if matches!(op, crate::parser::BinOp::Eq) {
                    return self.operand_for_local(is_none);
                }
                let neg = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        neg,
                        Rvalue::UnaryOp(crate::parser::UnaryOp::Not, Operand::Copy(is_none)),
                    ),
                    span,
                );
                return self.operand_for_local(neg);
            }
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

            // With `None` statically on one side the result can fold to a
            // constant. Only `None` equals `None`, and a value of a scalar type
            // that can never hold null is never equal to `None`. Unions, `Any`,
            // and reference types may carry a null at runtime, so they fall
            // through to the ordinary comparison.
            let is_scalar = |t: &Type| {
                matches!(
                    t,
                    Type::Int
                        | Type::I8
                        | Type::I16
                        | Type::I32
                        | Type::U8
                        | Type::U16
                        | Type::U32
                        | Type::U64
                        | Type::Usize
                        | Type::Float
                        | Type::F32
                        | Type::Bool
                )
            };
            let l_null = matches!(l_ty, Type::Null);
            let r_null = matches!(r_ty, Type::Null);
            if l_null && r_null {
                self.lower_expr_as_copy(left);
                self.lower_expr_as_copy(right);
                return Operand::Constant(Constant::Bool(matches!(op, crate::parser::BinOp::Eq)));
            }
            if (l_null && is_scalar(&r_ty)) || (r_null && is_scalar(&l_ty)) {
                self.lower_expr_as_copy(left);
                self.lower_expr_as_copy(right);
                return Operand::Constant(Constant::Bool(!matches!(op, crate::parser::BinOp::Eq)));
            }
            // Comparing a pointer-backed value against `None` is a raw null
            // check, not a structural or Python-level comparison, so test the
            // pointer against 0 directly and never dereference it.
            if l_null || r_null {
                let operand = if l_null { right } else { left };
                let v = self.lower_expr_as_copy(operand);
                // Raw-int reinterpret before the 0 test; a PyObject-typed operand
                // would route through Python eq (boxes the 0, compares identity).
                let raw = self.new_local(Type::Int, None, false);
                self.push_statement(StatementKind::Assign(raw, Rvalue::Use(v)), span);
                let is_zero = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        is_zero,
                        Rvalue::BinaryOp(
                            crate::parser::BinOp::Eq,
                            Operand::Copy(raw),
                            Operand::Constant(Constant::Int(0)),
                        ),
                    ),
                    span,
                );
                if matches!(op, crate::parser::BinOp::Eq) {
                    return self.operand_for_local(is_zero);
                }
                let neg = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        neg,
                        Rvalue::UnaryOp(crate::parser::UnaryOp::Not, Operand::Copy(is_zero)),
                    ),
                    span,
                );
                return self.operand_for_local(neg);
            }
        }

        // Membership in an `[Any]`/`{Any}` compares the needle word against the
        // stored element words. A scalar element is boxed on the way in, so the
        // needle is boxed the same way; equal inline scalars share one word and
        // match exactly.
        if matches!(op, crate::parser::BinOp::In | crate::parser::BinOp::NotIn)
            && matches!(&r_ty, Type::List(e) | Type::Set(e) if **e == Type::Any)
        {
            let l_ty = self.get_type(left.id).clone();
            let haystack = self.lower_expr_as_copy(right);
            let needle = self.lower_expr_as_copy(left);
            let needle = self.box_into_any(needle, &l_ty, span);
            let call_tmp = self.new_local(Type::Bool, None, false);
            self.push_statement(
                StatementKind::Assign(
                    call_tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__olive_in_list".to_string())),
                        args: vec![needle, haystack],
                    },
                ),
                span,
            );
            if matches!(op, crate::parser::BinOp::In) {
                return self.operand_for_local(call_tmp);
            }
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
        // The runtime `Any` operators dispatch on a self-describing value, so a
        // concrete operand paired with an `Any` one is boxed first. Without this
        // a raw scalar word reaches the dispatch and a large odd int is
        // misread as a tagged string pointer.
        use crate::parser::BinOp;
        let any_dispatch = matches!(
            op,
            BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Mod
                | BinOp::Lt
                | BinOp::LtEq
                | BinOp::Gt
                | BinOp::GtEq
                | BinOp::Eq
                | BinOp::NotEq
        );
        let l_ty = self.get_type(left.id).clone();
        let (l, r) = if any_dispatch {
            let l = if r_ty == Type::Any && l_ty != Type::Any {
                self.box_into_any(l, &l_ty, span)
            } else {
                l
            };
            let r = if l_ty == Type::Any && r_ty != Type::Any {
                self.box_into_any(r, &r_ty, span)
            } else {
                r
            };
            (l, r)
        } else {
            (l, r)
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
