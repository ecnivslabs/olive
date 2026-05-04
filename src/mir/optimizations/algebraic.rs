use super::Transform;
use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

pub struct AlgebraicSimplification;

impl Transform for AlgebraicSimplification {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut assign_counts: HashMap<Local, usize> = HashMap::default();
        let mut def_map: HashMap<Local, Rvalue> = HashMap::default();

        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    *assign_counts.entry(*dest).or_insert(0) += 1;
                    def_map.insert(*dest, rval.clone());
                }
            }
        }

        let single_def: HashMap<Local, Rvalue> = def_map
            .into_iter()
            .filter(|(l, _)| assign_counts.get(l) == Some(&1))
            .collect();

        let mut changed = false;

        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    use crate::parser::BinOp::*;
                    match rval {
                        Rvalue::BinaryOp(
                            Div,
                            Operand::Copy(src),
                            Operand::Constant(Constant::Int(b)),
                        ) if *b != 0 => {
                            if let Some(Rvalue::BinaryOp(
                                Mul,
                                mul_lhs,
                                Operand::Constant(Constant::Int(a)),
                            )) = single_def.get(src)
                            {
                                if *a % *b == 0 {
                                    let factor = *a / *b;
                                    *rval = Rvalue::BinaryOp(
                                        Mul,
                                        mul_lhs.clone(),
                                        Operand::Constant(Constant::Int(factor)),
                                    );
                                    changed = true;
                                }
                            } else if let Some(Rvalue::BinaryOp(
                                Mul,
                                Operand::Constant(Constant::Int(a)),
                                mul_rhs,
                            )) = single_def.get(src)
                                && *a % *b == 0
                            {
                                let factor = *a / *b;
                                *rval = Rvalue::BinaryOp(
                                    Mul,
                                    mul_rhs.clone(),
                                    Operand::Constant(Constant::Int(factor)),
                                );
                                changed = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        changed
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
    fn div_mul_factor_simplified() {
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
        AlgebraicSimplification.run(&mut f);
        match &f.basic_blocks[0].statements[1].kind {
            StatementKind::Assign(
                _,
                Rvalue::BinaryOp(crate::parser::BinOp::Mul, _, Operand::Constant(Constant::Int(2))),
            ) => {}
            _ => panic!("expected Mul by 2"),
        }
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
