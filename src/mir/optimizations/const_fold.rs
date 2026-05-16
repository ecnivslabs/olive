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
                            Add => Some(Constant::Int((*a).wrapping_add(*b))),
                            Sub => Some(Constant::Int((*a).wrapping_sub(*b))),
                            Mul => Some(Constant::Int((*a).wrapping_mul(*b))),
                            Div => {
                                if *b != 0 {
                                    Some(Constant::Int(*a / *b))
                                } else {
                                    None
                                }
                            }
                            Mod => {
                                if *b != 0 {
                                    Some(Constant::Int(*a % *b))
                                } else {
                                    None
                                }
                            }
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
