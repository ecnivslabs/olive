use super::Transform;
use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

pub struct GlobalValueNumbering;

impl Transform for GlobalValueNumbering {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;

        let mut value_map: HashMap<Rvalue, (Local, usize)> = HashMap::default();
        let mut assign_counts: HashMap<Local, usize> = HashMap::default();

        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, _) = &stmt.kind {
                    *assign_counts.entry(*dest).or_insert(0) += 1;
                }
            }
        }

        for bb_idx in 0..func.basic_blocks.len() {
            value_map.clear();
            let mut i = 0;
            while i < func.basic_blocks[bb_idx].statements.len() {
                let stmt = &func.basic_blocks[bb_idx].statements[i];
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    let dest = *dest;
                    if matches!(rval, Rvalue::BinaryOp(..) | Rvalue::UnaryOp(..))
                        && self.operands_stable(rval, &assign_counts, func.arg_count)
                    {
                        if let Some(&(existing, _)) = value_map.get(rval) {
                            if existing != dest {
                                let new_rval = Rvalue::Use(Operand::Copy(existing));
                                if func.basic_blocks[bb_idx].statements[i].kind
                                    != StatementKind::Assign(dest, new_rval.clone())
                                {
                                    func.basic_blocks[bb_idx].statements[i].kind =
                                        StatementKind::Assign(dest, new_rval);
                                    changed = true;
                                }
                            }
                        } else if assign_counts.get(&dest) == Some(&1) {
                            value_map.insert(rval.clone(), (dest, bb_idx));
                        }
                    }

                    value_map.retain(|expr, _| !self.uses_local(expr, dest));
                }

                if matches!(
                    func.basic_blocks[bb_idx].statements[i].kind,
                    StatementKind::SetIndex(..)
                        | StatementKind::SetAttr(..)
                        | StatementKind::VectorStore(..)
                ) {
                    value_map.retain(|expr, _| {
                        !matches!(expr, Rvalue::GetIndex(..) | Rvalue::GetAttr(..))
                    });
                }

                if matches!(
                    &func.basic_blocks[bb_idx].statements[i].kind,
                    StatementKind::Assign(_, Rvalue::Call { .. })
                ) {
                    value_map.clear();
                }

                i += 1;
            }
        }
        changed
    }
}

impl GlobalValueNumbering {
    fn operands_stable(
        &self,
        rval: &Rvalue,
        counts: &HashMap<Local, usize>,
        arg_count: usize,
    ) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => self.op_stable(op, counts, arg_count),
            Rvalue::BinaryOp(_, l, r) => {
                self.op_stable(l, counts, arg_count) && self.op_stable(r, counts, arg_count)
            }
            _ => false,
        }
    }

    fn op_stable(&self, op: &Operand, counts: &HashMap<Local, usize>, arg_count: usize) -> bool {
        match op {
            Operand::Constant(_) => true,
            Operand::Copy(l) | Operand::Move(l) => l.0 <= arg_count || counts.get(l) == Some(&1),
        }
    }

    fn uses_local(&self, rval: &Rvalue, local: Local) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => self.is_local(op, local),
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
                self.is_local(l, local) || self.is_local(r, local)
            }
            _ => false,
        }
    }

    fn is_local(&self, op: &Operand, local: Local) -> bool {
        matches!(op, Operand::Copy(l) | Operand::Move(l) if *l == local)
    }
}
