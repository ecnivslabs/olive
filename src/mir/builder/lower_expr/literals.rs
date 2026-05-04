use super::super::MirBuilder;
use crate::mir::ir::*;
use crate::parser::Expr;
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    pub(super) fn lower_fstr_expr(&mut self, exprs: &[Expr], span: Span) -> Operand {
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
                    span,
                );
                current_res = Some(Operand::Copy(tmp));
            } else {
                current_res = Some(str_op);
            }
        }

        current_res.unwrap()
    }
}
