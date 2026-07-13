use super::Transform;
use crate::mir::*;

pub struct ConstantFolding;

impl Transform for ConstantFolding {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    if let Rvalue::BinaryOp(
                        op,
                        Operand::Constant(Constant::Int(a)),
                        Operand::Constant(Constant::Int(b)),
                    ) = rval
                    {
                        use crate::parser::BinOp::*;
                        let res = match op {
                            // checked_* returns None on overflow (including the
                            // i64::MIN / -1 and % -1 corners, which would
                            // otherwise panic this compiler's own `/`/`%`), so
                            // the statement is left unfolded and falls through
                            // to codegen's runtime overflow fault instead of
                            // silently folding to a wrapped constant.
                            Add => (*a).checked_add(*b).map(Constant::Int),
                            Sub => (*a).checked_sub(*b).map(Constant::Int),
                            Mul => (*a).checked_mul(*b).map(Constant::Int),
                            Div => (*a).checked_div(*b).map(Constant::Int),
                            Mod => (*a).checked_rem(*b).map(Constant::Int),
                            Eq => Some(Constant::Bool(*a == *b)),
                            NotEq => Some(Constant::Bool(*a != *b)),
                            Lt => Some(Constant::Bool(*a < *b)),
                            LtEq => Some(Constant::Bool(*a <= *b)),
                            Gt => Some(Constant::Bool(*a > *b)),
                            GtEq => Some(Constant::Bool(*a >= *b)),
                            Shl => Some(Constant::Int((*a).wrapping_shl(*b as u32))),
                            Shr => Some(Constant::Int((*a).wrapping_shr(*b as u32))),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(val));
                            changed = true;
                        }
                    } else if let Rvalue::BinaryOp(
                        op,
                        Operand::Constant(Constant::Float(a_bits)),
                        Operand::Constant(Constant::Float(b_bits)),
                    ) = rval
                    {
                        let a = f64::from_bits(*a_bits);
                        let b = f64::from_bits(*b_bits);
                        use crate::parser::BinOp::*;
                        let res = match op {
                            Add => Some(Constant::Float((a + b).to_bits())),
                            Sub => Some(Constant::Float((a - b).to_bits())),
                            Mul => Some(Constant::Float((a * b).to_bits())),
                            Div => Some(Constant::Float((a / b).to_bits())),
                            Eq => Some(Constant::Bool(a == b)),
                            NotEq => Some(Constant::Bool(a != b)),
                            Lt => Some(Constant::Bool(a < b)),
                            LtEq => Some(Constant::Bool(a <= b)),
                            Gt => Some(Constant::Bool(a > b)),
                            GtEq => Some(Constant::Bool(a >= b)),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(val));
                            changed = true;
                        }
                    } else if let Rvalue::BinaryOp(
                        op,
                        Operand::Constant(Constant::Bool(a)),
                        Operand::Constant(Constant::Bool(b)),
                    ) = rval
                    {
                        use crate::parser::BinOp::*;
                        let res = match op {
                            Eq => Some(Constant::Bool(*a == *b)),
                            NotEq => Some(Constant::Bool(*a != *b)),
                            And => Some(Constant::Bool(*a && *b)),
                            Or => Some(Constant::Bool(*a || *b)),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(val));
                            changed = true;
                        }
                    } else if let Rvalue::UnaryOp(op, Operand::Constant(c)) = rval {
                        use crate::parser::UnaryOp::*;
                        let res = match (op, c) {
                            (Neg, Constant::Int(a)) => Some(Constant::Int(-*a)),
                            (Neg, Constant::Float(a)) => {
                                Some(Constant::Float((-f64::from_bits(*a)).to_bits()))
                            }
                            (Not, Constant::Bool(a)) => Some(Constant::Bool(!*a)),
                            (Not, Constant::Int(a)) => Some(Constant::Bool(*a == 0)),
                            (Pos, Constant::Int(a)) => Some(Constant::Int(*a)),
                            (Pos, Constant::Float(a)) => Some(Constant::Float(*a)),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(val));
                            changed = true;
                        }
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
    fn fold_add() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Add,
                Operand::Constant(Constant::Int(2)),
                Operand::Constant(Constant::Int(3)),
            ),
        )]);
        assert!(ConstantFolding.run(&mut f));
        let k = match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(k))) => k,
            _ => panic!(),
        };
        assert_eq!(*k, Constant::Int(5));
    }

    #[test]
    fn fold_sub() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Sub,
                Operand::Constant(Constant::Int(10)),
                Operand::Constant(Constant::Int(3)),
            ),
        )]);
        assert!(ConstantFolding.run(&mut f));
        let k = match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(k))) => k,
            _ => panic!(),
        };
        assert_eq!(*k, Constant::Int(7));
    }

    #[test]
    fn fold_mul() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Mul,
                Operand::Constant(Constant::Int(6)),
                Operand::Constant(Constant::Int(7)),
            ),
        )]);
        assert!(ConstantFolding.run(&mut f));
        let k = match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(k))) => k,
            _ => panic!(),
        };
        assert_eq!(*k, Constant::Int(42));
    }

    #[test]
    fn fold_eq_true() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Eq,
                Operand::Constant(Constant::Int(1)),
                Operand::Constant(Constant::Int(1)),
            ),
        )]);
        assert!(ConstantFolding.run(&mut f));
        let k = match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(k))) => k,
            _ => panic!(),
        };
        assert_eq!(*k, Constant::Bool(true));
    }

    #[test]
    fn fold_eq_false() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Eq,
                Operand::Constant(Constant::Int(1)),
                Operand::Constant(Constant::Int(2)),
            ),
        )]);
        assert!(ConstantFolding.run(&mut f));
        let k = match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(k))) => k,
            _ => panic!(),
        };
        assert_eq!(*k, Constant::Bool(false));
    }

    #[test]
    fn fold_lt() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Lt,
                Operand::Constant(Constant::Int(1)),
                Operand::Constant(Constant::Int(2)),
            ),
        )]);
        assert!(ConstantFolding.run(&mut f));
        let k = match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(k))) => k,
            _ => panic!(),
        };
        assert_eq!(*k, Constant::Bool(true));
    }

    #[test]
    fn fold_and() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::And,
                Operand::Constant(Constant::Bool(true)),
                Operand::Constant(Constant::Bool(false)),
            ),
        )]);
        assert!(ConstantFolding.run(&mut f));
        let k = match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(k))) => k,
            _ => panic!(),
        };
        assert_eq!(*k, Constant::Bool(false));
    }

    #[test]
    fn fold_unary_neg() {
        let mut f = func(vec![assign(
            0,
            Rvalue::UnaryOp(
                crate::parser::UnaryOp::Neg,
                Operand::Constant(Constant::Int(5)),
            ),
        )]);
        assert!(ConstantFolding.run(&mut f));
        let k = match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(k))) => k,
            _ => panic!(),
        };
        assert_eq!(*k, Constant::Int(-5));
    }

    #[test]
    fn fold_unary_not() {
        let mut f = func(vec![assign(
            0,
            Rvalue::UnaryOp(
                crate::parser::UnaryOp::Not,
                Operand::Constant(Constant::Bool(true)),
            ),
        )]);
        assert!(ConstantFolding.run(&mut f));
        let k = match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(k))) => k,
            _ => panic!(),
        };
        assert_eq!(*k, Constant::Bool(false));
    }

    #[test]
    fn fold_no_change() {
        let mut f = func(vec![assign(
            0,
            Rvalue::BinaryOp(
                BinOp::Add,
                Operand::Copy(Local(0)),
                Operand::Constant(Constant::Int(1)),
            ),
        )]);
        assert!(!ConstantFolding.run(&mut f));
    }
}
