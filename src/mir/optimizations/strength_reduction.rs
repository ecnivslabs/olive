use super::Transform;
use crate::mir::*;
use crate::semantic::types::Type as OliveType;

fn is_pyobj_op(local_types: &[OliveType], op: &Operand) -> bool {
    match op {
        Operand::Copy(l) | Operand::Move(l) => local_types[l.0].is_py_value(),
        _ => false,
    }
}

pub struct StrengthReduction;

impl Transform for StrengthReduction {
    fn run(&self, func: &mut MirFunction) -> bool {
        let local_types: Vec<OliveType> = func.locals.iter().map(|l| l.ty.clone()).collect();
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    use crate::parser::BinOp::*;
                    let replacement = match &*rval {
                        Rvalue::BinaryOp(Mul, op, Operand::Constant(Constant::Int(c)))
                        | Rvalue::BinaryOp(Mul, Operand::Constant(Constant::Int(c)), op)
                            if *c > 2
                                && (*c as u64).is_power_of_two()
                                && !is_pyobj_op(&local_types, op) =>
                        {
                            let shift = (*c as u64).trailing_zeros() as i64;
                            Some(Rvalue::BinaryOp(
                                Shl,
                                op.clone(),
                                Operand::Constant(Constant::Int(shift)),
                            ))
                        }
                        Rvalue::BinaryOp(Div, op, Operand::Constant(Constant::Int(c)))
                            if *c > 1
                                && (*c as u64).is_power_of_two()
                                && !is_pyobj_op(&local_types, op) =>
                        {
                            let shift = (*c as u64).trailing_zeros() as i64;
                            Some(Rvalue::BinaryOp(
                                Shr,
                                op.clone(),
                                Operand::Constant(Constant::Int(shift)),
                            ))
                        }
                        Rvalue::BinaryOp(Mod, op, Operand::Constant(Constant::Int(c)))
                            if *c > 1
                                && (*c as u64).is_power_of_two()
                                && !is_pyobj_op(&local_types, op) =>
                        {
                            let mask = *c - 1;
                            Some(Rvalue::BinaryOp(
                                And,
                                op.clone(),
                                Operand::Constant(Constant::Int(mask)),
                            ))
                        }
                        _ => None,
                    };
                    if let Some(new_rval) = replacement {
                        *rval = new_rval;
                        changed = true;
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

    fn func(locals: Vec<LocalDecl>, stmts: Vec<Statement>) -> MirFunction {
        MirFunction {
            name: "f".into(),
            locals,
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

    fn local_decl(ty: crate::semantic::types::Type) -> LocalDecl {
        LocalDecl {
            ty,
            name: None,
            span: sp(),
            is_mut: false,
            is_owning: false,
        }
    }

    #[test]
    fn mul_power_of_two_to_shift() {
        let mut f = func(
            vec![local_decl(crate::semantic::types::Type::Int)],
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Mul,
                    Operand::Copy(Local(0)),
                    Operand::Constant(Constant::Int(8)),
                ),
            )],
        );
        assert!(StrengthReduction.run(&mut f));
        match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(
                _,
                Rvalue::BinaryOp(crate::parser::BinOp::Shl, _, Operand::Constant(Constant::Int(3))),
            ) => {}
            _ => panic!("expected Shl by 3"),
        }
    }

    #[test]
    fn div_power_of_two_to_shift() {
        let mut f = func(
            vec![local_decl(crate::semantic::types::Type::Int)],
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Div,
                    Operand::Copy(Local(0)),
                    Operand::Constant(Constant::Int(16)),
                ),
            )],
        );
        assert!(StrengthReduction.run(&mut f));
        match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(
                _,
                Rvalue::BinaryOp(crate::parser::BinOp::Shr, _, Operand::Constant(Constant::Int(4))),
            ) => {}
            _ => panic!("expected Shr by 4"),
        }
    }

    #[test]
    fn mod_power_of_two_to_and() {
        let mut f = func(
            vec![local_decl(crate::semantic::types::Type::Int)],
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Mod,
                    Operand::Copy(Local(0)),
                    Operand::Constant(Constant::Int(8)),
                ),
            )],
        );
        assert!(StrengthReduction.run(&mut f));
        match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(
                _,
                Rvalue::BinaryOp(crate::parser::BinOp::And, _, Operand::Constant(Constant::Int(7))),
            ) => {}
            _ => panic!("expected And by 7"),
        }
    }

    #[test]
    fn no_change_non_power_of_two() {
        let mut f = func(
            vec![local_decl(crate::semantic::types::Type::Int)],
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Mul,
                    Operand::Copy(Local(0)),
                    Operand::Constant(Constant::Int(7)),
                ),
            )],
        );
        assert!(!StrengthReduction.run(&mut f));
    }

    #[test]
    fn no_change_pyobj_type() {
        let mut f = func(
            vec![local_decl(crate::semantic::types::Type::PyObject)],
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Mul,
                    Operand::Copy(Local(0)),
                    Operand::Constant(Constant::Int(8)),
                ),
            )],
        );
        assert!(!StrengthReduction.run(&mut f));
    }
}
