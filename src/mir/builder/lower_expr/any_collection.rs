use super::super::MirBuilder;
use crate::mir::ir::*;
use crate::parser::BinOp;
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    pub(super) fn erase_list_elements(
        &mut self,
        source: Operand,
        element: &Type,
        span: Span,
    ) -> Operand {
        let length = self.new_unscoped_local(Type::Int);
        self.push_statement(
            StatementKind::Assign(
                length,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_len".into())),
                    args: vec![source.clone()],
                },
            ),
            span,
        );
        let result = self.new_local(Type::List(Box::new(Type::Any)), None, false);
        self.push_statement(
            StatementKind::Assign(
                result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_new".into())),
                    args: vec![Operand::Copy(length)],
                },
            ),
            span,
        );
        let index = self.new_unscoped_local(Type::Int);
        self.push_statement(
            StatementKind::Assign(index, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            span,
        );
        let condition = self.new_block();
        let body = self.new_block();
        let done = self.new_block();
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: condition },
            span,
        );
        self.current_block = Some(condition);
        let more = self.new_unscoped_local(Type::Bool);
        self.push_statement(
            StatementKind::Assign(
                more,
                Rvalue::BinaryOp(BinOp::Lt, Operand::Copy(index), Operand::Copy(length)),
            ),
            span,
        );
        self.terminate_block(
            condition,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(more),
                targets: vec![(1, body)],
                otherwise: done,
            },
            span,
        );
        self.current_block = Some(body);
        let item = self.new_unscoped_local_with_owning(element.clone(), false);
        self.push_statement(
            StatementKind::Assign(item, Rvalue::GetIndex(source, Operand::Copy(index), true)),
            span,
        );
        let erased = self.box_into_any(Operand::Copy(item), element, span);
        self.push_statement(
            StatementKind::SetIndex(Operand::Copy(result), Operand::Copy(index), erased, true),
            span,
        );
        self.push_statement(
            StatementKind::Assign(
                index,
                Rvalue::BinaryOp(
                    BinOp::Add,
                    Operand::Copy(index),
                    Operand::Constant(Constant::Int(1)),
                ),
            ),
            span,
        );
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: condition },
            span,
        );
        self.current_block = Some(done);
        Operand::Copy(result)
    }
}
