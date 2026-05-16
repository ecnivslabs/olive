use super::Transform;
use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

pub struct AlgebraicSimplification;

impl Transform for AlgebraicSimplification {
    fn name(&self) -> &'static str {
        "algebraic_simplification"
    }

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
