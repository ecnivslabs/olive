use super::Transform;
use crate::mir::*;
use std::collections::HashSet;

pub struct DeadCodeElimination;

impl Transform for DeadCodeElimination {
    fn name(&self) -> &'static str {
        "dead_code_elimination"
    }
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut used = HashSet::new();
        used.insert(Local(0));

        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                match &stmt.kind {
                    StatementKind::Assign(_, rval) => self.record_rvalue_usage(rval, &mut used),
                    StatementKind::SetAttr(obj, _, val) => {
                        self.record_operand_usage(obj, &mut used);
                        self.record_operand_usage(val, &mut used);
                    }
                    StatementKind::SetIndex(obj, idx, val) => {
                        self.record_operand_usage(obj, &mut used);
                        self.record_operand_usage(idx, &mut used);
                        self.record_operand_usage(val, &mut used);
                    }
                    StatementKind::VectorStore(obj, idx, val) => {
                        self.record_operand_usage(obj, &mut used);
                        self.record_operand_usage(idx, &mut used);
                        self.record_operand_usage(val, &mut used);
                    }
                    _ => {}
                }
            }
            if let Some(term) = &bb.terminator {
                match &term.kind {
                    TerminatorKind::SwitchInt { discr, .. } => {
                        self.record_operand_usage(discr, &mut used)
                    }
                    _ => {}
                }
            }
        }

        let mut changed = false;
        for bb in &mut func.basic_blocks {
            let old_len = bb.statements.len();
            bb.statements.retain(|stmt| {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    if matches!(rval, Rvalue::Call { .. }) {
                        return true;
                    }
                    used.contains(dest)
                } else {
                    true
                }
            });
            if bb.statements.len() != old_len {
                changed = true;
            }
        }
        changed
    }
}

impl DeadCodeElimination {
    fn record_rvalue_usage(&self, rval: &Rvalue, used: &mut HashSet<Local>) {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => {
                self.record_operand_usage(op, used)
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
                self.record_operand_usage(l, used);
                self.record_operand_usage(r, used);
            }
            Rvalue::Call { func, args } => {
                self.record_operand_usage(func, used);
                for arg in args {
                    self.record_operand_usage(arg, used);
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for op in ops {
                    self.record_operand_usage(op, used);
                }
            }
            Rvalue::Ref(l) | Rvalue::MutRef(l) => {
                used.insert(*l);
            }
            Rvalue::VectorSplat(op, _) => self.record_operand_usage(op, used),
            Rvalue::VectorLoad(obj, idx, _) => {
                self.record_operand_usage(obj, used);
                self.record_operand_usage(idx, used);
            }
            Rvalue::VectorFMA(a, b, c) => {
                self.record_operand_usage(a, used);
                self.record_operand_usage(b, used);
                self.record_operand_usage(c, used);
            }
        }
    }

    fn record_operand_usage(&self, op: &Operand, used: &mut HashSet<Local>) {
        if let Operand::Copy(l) | Operand::Move(l) = op {
            used.insert(*l);
        }
    }
}
