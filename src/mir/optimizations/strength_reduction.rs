use super::Transform;
use crate::mir::*;

pub struct StrengthReduction;

impl Transform for StrengthReduction {
    fn name(&self) -> &'static str {
        "strength_reduction"
    }
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    use crate::parser::BinOp::*;
                    match rval {
                        Rvalue::BinaryOp(Mul, op, Operand::Constant(Constant::Int(c)))
                        | Rvalue::BinaryOp(Mul, Operand::Constant(Constant::Int(c)), op)
                            if *c > 2 && (*c as u64).is_power_of_two() =>
                        {
                            let shift = (*c as u64).trailing_zeros() as i64;
                            let saved_op = op.clone();
                            *rval = Rvalue::BinaryOp(
                                Shl,
                                saved_op,
                                Operand::Constant(Constant::Int(shift)),
                            );
                            changed = true;
                        }
                        Rvalue::BinaryOp(Div, op, Operand::Constant(Constant::Int(c)))
                        | Rvalue::BinaryOp(FloorDiv, op, Operand::Constant(Constant::Int(c)))
                            if *c > 1 && (*c as u64).is_power_of_two() =>
                        {
                            let shift = (*c as u64).trailing_zeros() as i64;
                            let saved_op = op.clone();
                            *rval = Rvalue::BinaryOp(
                                Shr,
                                saved_op,
                                Operand::Constant(Constant::Int(shift)),
                            );
                            changed = true;
                        }
                        Rvalue::BinaryOp(Mod, op, Operand::Constant(Constant::Int(c)))
                            if *c > 1 && (*c as u64).is_power_of_two() =>
                        {
                            let mask = *c - 1;
                            let saved_op = op.clone();
                            *rval = Rvalue::BinaryOp(
                                And,
                                saved_op,
                                Operand::Constant(Constant::Int(mask)),
                            );
                            changed = true;
                        }
                        // x * 3 -> (x << 1) + x — handled by Cranelift's own lowering,
                        // so we skip it here to avoid code bloat.
                        // x * 2 is handled by peephole (x + x).
                        _ => {}
                    }
                }
            }
        }
        changed
    }
}
