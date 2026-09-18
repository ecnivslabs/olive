use super::Transform;
use crate::mir::*;

pub struct ConstantFolding;

impl Transform for ConstantFolding {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(dest, rval) = &mut stmt.kind {
                    // A folded `Float` constant keeps f64 bits, which is a
                    // lie when the destination is `F32`: downstream codegen
                    // trusts the static type and would store the wide bits
                    // into an f32 slot (or forward them where f32 bits
                    // belong). Wrap in a narrowing `Cast` instead so the
                    // value stays canonical; `Float` destinations and exact
                    // int/bool folds are unaffected.
                    let narrow_f32 = func
                        .locals
                        .get(dest.0)
                        .is_some_and(|d| matches!(d.ty, crate::semantic::types::Type::F32));
                    let fold_float = |val: Constant| -> Rvalue {
                        if narrow_f32 && matches!(val, Constant::Float(_)) {
                            Rvalue::Cast(Operand::Constant(val), crate::semantic::types::Type::F32)
                        } else {
                            Rvalue::Use(Operand::Constant(val))
                        }
                    };
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
                            *rval = fold_float(val);
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
                            (Neg, Constant::Int(a)) => a.checked_neg().map(Constant::Int),
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
                            *rval = fold_float(val);
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
    fn fold_unary_neg_min_leaves_operation_for_codegen() {
        let mut f = func(vec![assign(
            0,
            Rvalue::UnaryOp(
                crate::parser::UnaryOp::Neg,
                Operand::Constant(Constant::Int(i64::MIN)),
            ),
        )]);
        assert!(!ConstantFolding.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::UnaryOp(..))
        ));
    }

    #[test]
    fn fold_float_neg_into_f32_narrows() {
        // A folded `Float` constant keeps f64 bits; into an `F32`
        // destination that lies about the payload width downstream, so the
        // fold wraps in a narrowing `Cast` instead of a bare `Use`.
        let mut f = func(vec![assign(
            0,
            Rvalue::UnaryOp(
                crate::parser::UnaryOp::Neg,
                Operand::Constant(Constant::Float(f64::to_bits(1.0))),
            ),
        )]);
        f.locals = vec![LocalDecl {
            ty: crate::semantic::types::Type::F32,
            name: None,
            span: sp(),
            is_mut: false,
            is_owning: false,
        }];
        assert!(ConstantFolding.run(&mut f));
        match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(
                _,
                Rvalue::Cast(Operand::Constant(Constant::Float(bits)), ty),
            ) => {
                assert_eq!(f64::from_bits(*bits), -1.0);
                assert_eq!(*ty, crate::semantic::types::Type::F32);
            }
            _ => panic!(),
        }
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
