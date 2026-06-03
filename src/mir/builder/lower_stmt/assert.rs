use super::super::MirBuilder;
use crate::mir::ir::*;
use crate::parser::{BinOp, Expr, ExprKind};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    /// Lowers `assert test[, msg]`. A top-level comparison (`==`, `!=`, `<`,
    /// `<=`, `>`, `>=`) captures both operand values on the failing path and
    /// the fault prints `left: <value>, right: <value>` beside the source
    /// caret; anything else just prints the expression via the caret (the
    /// existing `render_source` already shows it, so there is nothing extra
    /// to build). Zero cost on the passing path: every fault string is built
    /// inside the `fail` block, which the passing branch never reaches.
    pub(super) fn lower_assert_stmt(&mut self, test: &Expr, msg: &Option<Expr>) {
        let span = test.span;
        let cmp = match &test.kind {
            ExprKind::BinOp { left, op, right }
                if matches!(
                    op,
                    BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq
                ) =>
            {
                Some((left.as_ref(), op.clone(), right.as_ref()))
            }
            _ => None,
        };

        let test_op = if let Some((left, op, right)) = &cmp {
            self.last_cmp_operands = None;
            let result = self.lower_binop_expr(left, op, right, span, test.id);
            self.truthify(result, &Type::Bool, span)
        } else {
            let raw = self.lower_expr(test);
            let test_ty = self.get_type(test.id);
            self.truthify(raw, &test_ty, test.span)
        };

        // `lower_binop_expr` stashes the two values it compared right before
        // it returns (`ops.rs`); read that back now, before anything else
        // lowered here can overwrite it.
        let cmp_display = match &cmp {
            Some((left, _, right)) => match self.last_cmp_operands.take() {
                Some((l, r)) => {
                    let l_ty = self.get_type(left.id);
                    let r_ty = self.get_type(right.id);
                    Some((l, l_ty, r, r_ty))
                }
                None => None,
            },
            None => None,
        };

        let user_msg = match msg {
            Some(m) => {
                let v = self.lower_expr_as_copy(m);
                let ty = self.get_type(m.id);
                Some((v, ty))
            }
            None => None,
        };

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
                span,
            );
        }

        self.current_block = Some(fail_bb);
        self.emit_assert_fail(cmp_display, user_msg, span);
        self.terminate_block(fail_bb, TerminatorKind::Unreachable, Span::default());

        self.current_block = Some(pass_bb);
    }

    /// Builds the fault message and calls into the runtime
    /// (`__olive_assert_fail`), which renders it beside the source caret and
    /// aborts. Only ever reached on the failing branch.
    fn emit_assert_fail(
        &mut self,
        cmp_display: Option<(Operand, Type, Operand, Type)>,
        user_msg: Option<(Operand, Type)>,
        span: Span,
    ) {
        let mut text = self.str_literal("assertion failed", span);

        if let Some((m, m_ty)) = user_msg {
            let m_str = self.stringify_for_display(m, &m_ty, span);
            let sep = self.str_literal(": ", span);
            text = self.concat_str(text, sep, span);
            text = self.concat_str(text, m_str, span);
        }

        if let Some((l, l_ty, r, r_ty)) = cmp_display {
            let l_str = self.stringify_for_display(l, &l_ty, span);
            let r_str = self.stringify_for_display(r, &r_ty, span);
            let prefix = self.str_literal(" (left: ", span);
            let mid = self.str_literal(", right: ", span);
            let suffix = self.str_literal(")", span);
            text = self.concat_str(text, prefix, span);
            text = self.concat_str(text, l_str, span);
            text = self.concat_str(text, mid, span);
            text = self.concat_str(text, r_str, span);
            text = self.concat_str(text, suffix, span);
        }

        let loc = match self.file_names.get(&span.file_id) {
            Some(file) => format!("{}:{}:{}", file, span.line, span.col),
            None => format!("{}:{}", span.line, span.col),
        };
        let loc_op = self.str_literal(&loc, span);

        let sink = self.new_local(Type::Null, None, false);
        self.push_statement(
            StatementKind::Assign(
                sink,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_assert_fail".to_string())),
                    args: vec![text, loc_op],
                },
            ),
            span,
        );
    }

    /// Renders `op` (of static type `ty`) to a `str` operand, matching what
    /// the `str()` builtin produces: a struct's own `__str__` when it has
    /// one, the type-driven runtime dispatch otherwise.
    fn stringify_for_display(&mut self, op: Operand, ty: &Type, span: Span) -> Operand {
        if let Some(s) = self.lower_struct_str_call(op.clone(), ty, span) {
            return s;
        }
        let tmp = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("str".to_string())),
                    args: vec![op],
                },
            ),
            span,
        );
        self.operand_for_local(tmp)
    }

    fn str_literal(&mut self, s: &str, span: Span) -> Operand {
        let tmp = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Use(Operand::Constant(Constant::Str(s.to_string()))),
            ),
            span,
        );
        self.operand_for_local(tmp)
    }

    fn concat_str(&mut self, a: Operand, b: Operand, span: Span) -> Operand {
        let tmp = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_str_concat".to_string())),
                    args: vec![a, b],
                },
            ),
            span,
        );
        self.operand_for_local(tmp)
    }
}
