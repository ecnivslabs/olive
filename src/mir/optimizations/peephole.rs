use super::Transform;
use crate::mir::*;
use crate::parser::ast::UnaryOp;

pub struct PeepholeOptimize;

impl PeepholeOptimize {
    fn eliminate_double_not(bb: &mut BasicBlock) -> bool {
        use rustc_hash::FxHashMap;
        let mut not_defs: FxHashMap<Local, Operand> = FxHashMap::default();
        let mut changed = false;
        for stmt in &mut bb.statements {
            match &mut stmt.kind {
                StatementKind::Assign(out, Rvalue::UnaryOp(UnaryOp::Not, inner)) => {
                    let inner_local = match inner {
                        Operand::Copy(l) | Operand::Move(l) => *l,
                        _ => {
                            let _ = not_defs;
                            return changed;
                        }
                    };
                    if let Some(src) = not_defs.get(&inner_local).cloned() {
                        *stmt = Statement {
                            kind: StatementKind::Assign(*out, Rvalue::Use(src)),
                            span: stmt.span,
                        };
                        changed = true;
                    } else {
                        not_defs.insert(*out, inner.clone());
                    }
                }
                StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {}
                _ => {
                    not_defs.clear();
                }
            }
        }
        changed
    }
}

impl Transform for PeepholeOptimize {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            changed |= Self::eliminate_double_not(bb);
        }
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    use crate::parser::BinOp::*;
                    match rval {
                        Rvalue::BinaryOp(Add, op, Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(Add, Operand::Constant(Constant::Int(0)), op)
                        | Rvalue::BinaryOp(Sub, op, Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(Mul, op, Operand::Constant(Constant::Int(1)))
                        | Rvalue::BinaryOp(Mul, Operand::Constant(Constant::Int(1)), op)
                        | Rvalue::BinaryOp(Div, op, Operand::Constant(Constant::Int(1))) => {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(Mul, _, op @ Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(Mul, op @ Operand::Constant(Constant::Int(0)), _) => {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(Div, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Int(1)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Mul, op, Operand::Constant(Constant::Int(2)))
                        | Rvalue::BinaryOp(Mul, Operand::Constant(Constant::Int(2)), op) => {
                            *rval = Rvalue::BinaryOp(Add, op.clone(), op.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(Eq, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(true)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(NotEq, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(false)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Lt, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(false)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Gt, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(false)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(LtEq, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(true)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(GtEq, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(true)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Sub, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Int(0)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Shl, op, Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(Shr, op, Operand::Constant(Constant::Int(0))) => {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(And, _, Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(And, Operand::Constant(Constant::Int(0)), _) => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Int(0)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Or, op, Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(Or, Operand::Constant(Constant::Int(0)), op) => {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(And, l, r) if l == r => {
                            *rval = Rvalue::Use(l.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(Or, l, r) if l == r => {
                            *rval = Rvalue::Use(l.clone());
                            changed = true;
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
    use crate::parser::BinOp;

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

    fn func(stmts: Vec<Statement>) -> MirFunction {
        MirFunction {
            name: "f".into(),
            locals: vec![],
            basic_blocks: vec![BasicBlock {
                statements: stmts,
                terminator: Some(Terminator {
                    kind: TerminatorKind::Return,
                    span: sp(),
                }),
            }],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    #[test]
    fn add_zero() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Add,
                Operand::Copy(Local(1)),
                Operand::Constant(Constant::Int(0)),
            ),
        )]);
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(_))
        ));
    }

    #[test]
    fn mul_one() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Mul,
                Operand::Copy(Local(1)),
                Operand::Constant(Constant::Int(1)),
            ),
        )]);
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(_))
        ));
    }

    #[test]
    fn mul_zero() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Mul,
                Operand::Copy(Local(1)),
                Operand::Constant(Constant::Int(0)),
            ),
        )]);
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Int(0))))
        ));
    }

    #[test]
    fn div_one() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Div,
                Operand::Copy(Local(1)),
                Operand::Constant(Constant::Int(1)),
            ),
        )]);
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(_))
        ));
    }

    #[test]
    fn sub_self() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(BinOp::Sub, Operand::Copy(Local(1)), Operand::Copy(Local(1))),
        )]);
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Int(0))))
        ));
    }

    #[test]
    fn eq_self() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(BinOp::Eq, Operand::Copy(Local(1)), Operand::Copy(Local(1))),
        )]);
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Bool(true))))
        ));
    }

    #[test]
    fn neq_self() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::NotEq,
                Operand::Copy(Local(1)),
                Operand::Copy(Local(1)),
            ),
        )]);
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Bool(false))))
        ));
    }

    #[test]
    fn div_self() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(BinOp::Div, Operand::Copy(Local(1)), Operand::Copy(Local(1))),
        )]);
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Int(1))))
        ));
    }

    #[test]
    fn and_zero() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::And,
                Operand::Copy(Local(1)),
                Operand::Constant(Constant::Int(0)),
            ),
        )]);
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Int(0))))
        ));
    }

    #[test]
    fn or_zero() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Or,
                Operand::Copy(Local(1)),
                Operand::Constant(Constant::Int(0)),
            ),
        )]);
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Copy(Local(1))))
        ));
    }

    #[test]
    fn no_change_for_non_pattern() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(BinOp::Add, Operand::Copy(Local(1)), Operand::Copy(Local(2))),
        )]);
        assert!(!PeepholeOptimize.run(&mut f));
    }

    #[test]
    fn shl_zero() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Shl,
                Operand::Copy(Local(1)),
                Operand::Constant(Constant::Int(0)),
            ),
        )]);
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(_))
        ));
    }

    #[test]
    fn shr_zero() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Shr,
                Operand::Copy(Local(1)),
                Operand::Constant(Constant::Int(0)),
            ),
        )]);
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(_))
        ));
    }
}
