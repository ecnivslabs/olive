use super::Transform;
use crate::mir::*;

pub struct AlgebraicSimplification;

impl Transform for AlgebraicSimplification {
    fn run(&self, _func: &mut MirFunction) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp() -> crate::span::Span {
        crate::span::Span {
            file_id: 0,
            line: 0,
            col: 0,
            start: 0,
            end: 0,
        }
    }

    fn assign(l: usize, rv: Rvalue) -> Statement {
        Statement {
            kind: StatementKind::Assign(Local(l), rv),
            span: sp(),
        }
    }

    fn func(blocks: Vec<BasicBlock>) -> MirFunction {
        MirFunction {
            name: "f".into(),
            locals: vec![],
            basic_blocks: blocks,
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    fn bb(stmts: Vec<Statement>, kind: TerminatorKind) -> BasicBlock {
        BasicBlock {
            statements: stmts,
            terminator: Some(Terminator { kind, span: sp() }),
        }
    }

    #[test]
    fn no_change_no_match() {
        let mut f = func(vec![bb(
            vec![assign(0, Rvalue::Use(Operand::Constant(Constant::Int(42))))],
            TerminatorKind::Return,
        )]);
        assert!(!AlgebraicSimplification.run(&mut f));
    }

    #[test]
    fn div_mul_factor_stays_unchanged_without_range_proof() {
        let mut f = func(vec![bb(
            vec![
                assign(
                    1,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Mul,
                        Operand::Copy(Local(2)),
                        Operand::Constant(Constant::Int(8)),
                    ),
                ),
                assign(
                    0,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Div,
                        Operand::Copy(Local(1)),
                        Operand::Constant(Constant::Int(4)),
                    ),
                ),
            ],
            TerminatorKind::Return,
        )]);
        assert!(!AlgebraicSimplification.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[1].kind,
            StatementKind::Assign(_, Rvalue::BinaryOp(crate::parser::BinOp::Div, ..))
        ));
    }

    #[test]
    fn no_simplify_when_factor_not_divisible() {
        let mut f = func(vec![bb(
            vec![
                assign(
                    1,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Mul,
                        Operand::Copy(Local(2)),
                        Operand::Constant(Constant::Int(7)),
                    ),
                ),
                assign(
                    0,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Div,
                        Operand::Copy(Local(1)),
                        Operand::Constant(Constant::Int(4)),
                    ),
                ),
            ],
            TerminatorKind::Return,
        )]);
        assert!(!AlgebraicSimplification.run(&mut f));
    }

    #[test]
    fn div_by_const_no_numeric_mul_before() {
        let mut f = func(vec![bb(
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Div,
                    Operand::Copy(Local(3)),
                    Operand::Constant(Constant::Int(2)),
                ),
            )],
            TerminatorKind::Return,
        )]);
        assert!(!AlgebraicSimplification.run(&mut f));
    }
}
