use super::Transform;
use crate::mir::*;
use crate::semantic::types::Type as OliveType;

fn is_pyobj_op(local_types: &[OliveType], op: &Operand) -> bool {
    match op {
        Operand::Copy(l) | Operand::Move(l) => {
            matches!(local_types[l.0], OliveType::PyObject)
        }
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
