use super::Transform;
use crate::mir::*;

pub struct SimplifyCfg;

impl Transform for SimplifyCfg {
    fn name(&self) -> &'static str {
        "simplify_cfg"
    }
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        changed |= self.branch_simplification(func);
        changed |= self.unreachable_block_elimination(func);
        changed
    }
}

impl SimplifyCfg {
    fn branch_simplification(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            if let Some(term) = &mut bb.terminator {
                if let TerminatorKind::SwitchInt {
                    discr,
                    targets,
                    otherwise,
                } = &term.kind
                {
                    if let Operand::Constant(Constant::Bool(b)) = discr {
                        let val = if *b { 1 } else { 0 };
                        let goto_target = targets
                            .iter()
                            .find(|(v, _)| *v == val)
                            .map(|(_, t)| *t)
                            .unwrap_or(*otherwise);
                        term.kind = TerminatorKind::Goto {
                            target: goto_target,
                        };
                        changed = true;
                    } else if let Operand::Constant(Constant::Int(val)) = discr {
                        let v = *val;
                        let goto_target = targets
                            .iter()
                            .find(|(tv, _)| *tv == v)
                            .map(|(_, t)| *t)
                            .unwrap_or(*otherwise);
                        term.kind = TerminatorKind::Goto {
                            target: goto_target,
                        };
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    fn unreachable_block_elimination(&self, func: &mut MirFunction) -> bool {
        let n = func.basic_blocks.len();
        if n <= 1 {
            return false;
        }

        let mut reachable = vec![false; n];
        reachable[0] = true;
        let mut worklist = vec![0usize];

        while let Some(idx) = worklist.pop() {
            if let Some(term) = &func.basic_blocks[idx].terminator {
                let succs: Vec<usize> = match &term.kind {
                    TerminatorKind::Goto { target } => vec![target.0],
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        let mut s: Vec<usize> = targets.iter().map(|(_, t)| t.0).collect();
                        s.push(otherwise.0);
                        s
                    }
                    _ => vec![],
                };
                for s in succs {
                    if s < n && !reachable[s] {
                        reachable[s] = true;
                        worklist.push(s);
                    }
                }
            }
        }

        let any_unreachable = reachable.iter().any(|r| !r);
        if !any_unreachable {
            return false;
        }

        // Build remapping for block indices.
        let mut remap = vec![0usize; n];
        let mut new_idx = 0;
        for i in 0..n {
            if reachable[i] {
                remap[i] = new_idx;
                new_idx += 1;
            }
        }

        // Remap terminators.
        for i in 0..n {
            if !reachable[i] {
                continue;
            }
            if let Some(term) = &mut func.basic_blocks[i].terminator {
                match &mut term.kind {
                    TerminatorKind::Goto { target } => {
                        target.0 = remap[target.0];
                    }
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        for (_, t) in targets.iter_mut() {
                            t.0 = remap[t.0];
                        }
                        otherwise.0 = remap[otherwise.0];
                    }
                    _ => {}
                }
            }
        }

        // Remove unreachable blocks.
        let mut idx = 0;
        func.basic_blocks.retain(|_| {
            let keep = reachable[idx];
            idx += 1;
            keep
        });

        true
    }
}
