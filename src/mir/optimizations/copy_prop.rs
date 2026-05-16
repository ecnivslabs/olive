use super::Transform;
use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

pub struct CopyPropagation;

impl Transform for CopyPropagation {
    fn name(&self) -> &'static str {
        "copy_propagation"
    }
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut assign_counts: HashMap<Local, usize> = HashMap::default();
        let mut copy_assignments: HashMap<Local, Local> = HashMap::default();

        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    *assign_counts.entry(*dest).or_insert(0) += 1;
                    if let Rvalue::Use(Operand::Copy(src)) = rval {
                        copy_assignments.insert(*dest, *src);
                    }
                }
            }
        }

        let mut safe_copies: HashMap<Local, Local> = HashMap::default();
        for (dest, src) in copy_assignments {
            if assign_counts.get(&dest) == Some(&1)
                && (*assign_counts.get(&src).unwrap_or(&0) <= 1 || src.0 <= func.arg_count)
            {
                safe_copies.insert(dest, src);
            }
        }

        if safe_copies.is_empty() {
            return false;
        }

        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                match &mut stmt.kind {
                    StatementKind::Assign(_, rval) => {
                        changed |= self.propagate_copies_in_rvalue(rval, &safe_copies);
                    }
                    StatementKind::SetIndex(obj, idx, val) => {
                        changed |= self.propagate_copies_in_operand(obj, &safe_copies);
                        changed |= self.propagate_copies_in_operand(idx, &safe_copies);
                        changed |= self.propagate_copies_in_operand(val, &safe_copies);
                    }
                    StatementKind::SetAttr(obj, _, val) => {
                        changed |= self.propagate_copies_in_operand(obj, &safe_copies);
                        changed |= self.propagate_copies_in_operand(val, &safe_copies);
                    }
                    StatementKind::VectorStore(obj, idx, val) => {
                        changed |= self.propagate_copies_in_operand(obj, &safe_copies);
                        changed |= self.propagate_copies_in_operand(idx, &safe_copies);
                        changed |= self.propagate_copies_in_operand(val, &safe_copies);
                    }
                    _ => {}
                }
            }
            if let Some(term) = &mut bb.terminator
                && let TerminatorKind::SwitchInt { discr, .. } = &mut term.kind
            {
                changed |= self.propagate_copies_in_operand(discr, &safe_copies);
            }
        }
        changed
    }
}

impl CopyPropagation {
    fn propagate_copies_in_rvalue(&self, rval: &mut Rvalue, map: &HashMap<Local, Local>) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => {
                self.propagate_copies_in_operand(op, map)
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
                let mut changed = self.propagate_copies_in_operand(l, map);
                changed |= self.propagate_copies_in_operand(r, map);
                changed
            }
            Rvalue::Call { func, args } => {
                let mut changed = self.propagate_copies_in_operand(func, map);
                for arg in args {
                    changed |= self.propagate_copies_in_operand(arg, map);
                }
                changed
            }
            Rvalue::Aggregate(_, ops) => {
                let mut changed = false;
                for op in ops {
                    changed |= self.propagate_copies_in_operand(op, map);
                }
                changed
            }
            Rvalue::PtrLoad(op) => self.propagate_copies_in_operand(op, map),
            Rvalue::VectorSplat(op, _) => self.propagate_copies_in_operand(op, map),
            Rvalue::VectorLoad(obj, idx, _) => {
                let mut changed = self.propagate_copies_in_operand(obj, map);
                changed |= self.propagate_copies_in_operand(idx, map);
                changed
            }
            Rvalue::VectorFMA(a, b, c) => {
                let mut changed = self.propagate_copies_in_operand(a, map);
                changed |= self.propagate_copies_in_operand(b, map);
                changed |= self.propagate_copies_in_operand(c, map);
                changed
            }
            _ => false,
        }
    }

    fn propagate_copies_in_operand(&self, op: &mut Operand, map: &HashMap<Local, Local>) -> bool {
        if let Operand::Copy(l) = op
            && let Some(new_l) = map.get(l)
        {
            *op = Operand::Copy(*new_l);
            return true;
        }
        false
    }
}
