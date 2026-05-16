use super::Transform;
use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

pub struct ConstantPropagation;

impl Transform for ConstantPropagation {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut assign_counts: HashMap<Local, usize> = HashMap::default();
        let mut constant_assignments: HashMap<Local, Constant> = HashMap::default();

        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    *assign_counts.entry(*dest).or_insert(0) += 1;
                    if let Rvalue::Use(Operand::Constant(c)) = rval {
                        constant_assignments.insert(*dest, c.clone());
                    }
                }
            }
        }

        let mut safe_constants: HashMap<Local, Constant> = HashMap::default();
        for (local, count) in &assign_counts {
            if *count == 1
                && let Some(c) = constant_assignments.get(local)
            {
                safe_constants.insert(*local, c.clone());
            }
        }

        let mut changed = false;

        if !safe_constants.is_empty() {
            for bb in &mut func.basic_blocks {
                for stmt in &mut bb.statements {
                    if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                        changed |= self.propagate_constants_in_rvalue(rval, &safe_constants);
                    } else if let StatementKind::SetIndex(obj, idx, val) = &mut stmt.kind {
                        changed |= self.propagate_constants_in_operand(obj, &safe_constants);
                        changed |= self.propagate_constants_in_operand(idx, &safe_constants);
                        changed |= self.propagate_constants_in_operand(val, &safe_constants);
                    } else if let StatementKind::SetAttr(obj, _, val) = &mut stmt.kind {
                        changed |= self.propagate_constants_in_operand(obj, &safe_constants);
                        changed |= self.propagate_constants_in_operand(val, &safe_constants);
                    } else if let StatementKind::VectorStore(obj, idx, val) = &mut stmt.kind {
                        changed |= self.propagate_constants_in_operand(obj, &safe_constants);
                        changed |= self.propagate_constants_in_operand(idx, &safe_constants);
                        changed |= self.propagate_constants_in_operand(val, &safe_constants);
                    }
                }
                if let Some(term) = &mut bb.terminator
                    && let TerminatorKind::SwitchInt { discr, .. } = &mut term.kind
                {
                    changed |= self.propagate_constants_in_operand(discr, &safe_constants);
                }
            }
        }

        for bb in &mut func.basic_blocks {
            let mut local_consts: HashMap<Local, Constant> = HashMap::default();
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(dest, rval) = &mut stmt.kind {
                    changed |= self.propagate_constants_in_rvalue(rval, &local_consts);

                    if let Rvalue::Use(Operand::Constant(c)) = rval {
                        local_consts.insert(*dest, c.clone());
                    } else {
                        local_consts.remove(dest);
                    }
                }
            }
        }

        changed
    }
}

impl ConstantPropagation {
    fn propagate_constants_in_rvalue(
        &self,
        rval: &mut Rvalue,
        map: &HashMap<Local, Constant>,
    ) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => {
                self.propagate_constants_in_operand(op, map)
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
                let mut changed = self.propagate_constants_in_operand(l, map);
                changed |= self.propagate_constants_in_operand(r, map);
                changed
            }
            Rvalue::Call { func, args } => {
                let mut changed = self.propagate_constants_in_operand(func, map);
                for arg in args {
                    changed |= self.propagate_constants_in_operand(arg, map);
                }
                changed
            }
            Rvalue::Aggregate(_, ops) => {
                let mut changed = false;
                for op in ops {
                    changed |= self.propagate_constants_in_operand(op, map);
                }
                changed
            }
            Rvalue::PtrLoad(op) => self.propagate_constants_in_operand(op, map),
            Rvalue::VectorSplat(op, _) => self.propagate_constants_in_operand(op, map),
            Rvalue::VectorLoad(obj, idx, _) => {
                let mut changed = self.propagate_constants_in_operand(obj, map);
                changed |= self.propagate_constants_in_operand(idx, map);
                changed
            }
            Rvalue::VectorFMA(a, b, c) => {
                let mut changed = self.propagate_constants_in_operand(a, map);
                changed |= self.propagate_constants_in_operand(b, map);
                changed |= self.propagate_constants_in_operand(c, map);
                changed
            }
            _ => false,
        }
    }

    fn propagate_constants_in_operand(
        &self,
        op: &mut Operand,
        map: &HashMap<Local, Constant>,
    ) -> bool {
        if let Operand::Copy(l) | Operand::Move(l) = op
            && let Some(c) = map.get(l)
        {
            *op = Operand::Constant(c.clone());
            return true;
        }
        false
    }
}
