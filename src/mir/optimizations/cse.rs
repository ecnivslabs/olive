use super::Transform;
use crate::mir::*;

pub struct CommonSubexpressionElimination;

impl Transform for CommonSubexpressionElimination {
    fn name(&self) -> &'static str {
        "common_subexpression_elimination"
    }
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            let mut available_expressions: Vec<(Rvalue, Local)> = Vec::new();
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(dest, rval) = &mut stmt.kind {
                    if matches!(
                        rval,
                        Rvalue::BinaryOp(..) | Rvalue::UnaryOp(..) | Rvalue::Use(..)
                    ) {
                        let mut found = None;
                        for (expr, local) in &available_expressions {
                            if expr == rval {
                                found = Some(*local);
                                break;
                            }
                        }

                        if let Some(existing_local) = found {
                            let new_rval = Rvalue::Use(Operand::Copy(existing_local));
                            if *rval != new_rval {
                                *rval = new_rval;
                                changed = true;
                            }
                        } else {
                            available_expressions.push((rval.clone(), *dest));
                        }
                    }

                    available_expressions.retain(|(expr, _)| !self.uses_local(expr, *dest));
                } else if matches!(
                    stmt.kind,
                    StatementKind::SetIndex(..) | StatementKind::SetAttr(..) | StatementKind::VectorStore(..)
                ) {
                    // Invalidate all heap-based expressions.
                    available_expressions.retain(|(expr, _)| {
                        !matches!(expr, Rvalue::GetIndex(..) | Rvalue::GetAttr(..))
                    });
                } else if let StatementKind::Assign(_, Rvalue::Call { .. }) = &stmt.kind {
                    available_expressions.clear();
                }
            }
        }
        changed
    }
}

impl CommonSubexpressionElimination {
    fn uses_local(&self, rval: &Rvalue, local: Local) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => self.is_local(op, local),
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => self.is_local(l, local) || self.is_local(r, local),
            Rvalue::VectorSplat(op, _) => self.is_local(op, local),
            Rvalue::VectorLoad(obj, idx, _) => self.is_local(obj, local) || self.is_local(idx, local),
            Rvalue::VectorFMA(a, b, c) => self.is_local(a, local) || self.is_local(b, local) || self.is_local(c, local),
            _ => false,
        }
    }

    fn is_local(&self, op: &Operand, local: Local) -> bool {
        if let Operand::Copy(l) | Operand::Move(l) = op {
            return *l == local;
        }
        false
    }
}
