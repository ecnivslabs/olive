use super::Transform;
use crate::mir::*;

pub struct PeepholeOptimize;

impl Transform for PeepholeOptimize {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
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
