use crate::mir::*;
use rustc_hash::{FxHashMap, FxHashSet as HashSet};

pub struct Loop {
    pub header: BasicBlockId,
    pub body: HashSet<BasicBlockId>,
    pub latches: Vec<BasicBlockId>,
    pub exits: Vec<BasicBlockId>,
}

pub fn find_loops(func: &MirFunction) -> Vec<Loop> {
    let mut loops = Vec::new();
    let num_blocks = func.basic_blocks.len();
    if num_blocks == 0 {
        return loops;
    }

    let dominators = compute_dominators(func);

    for (n_idx, bb) in func.basic_blocks.iter().enumerate() {
        let n = BasicBlockId(n_idx);
        for &d in &successors(bb) {
            if dominators[n.0].contains(&d) {
                let mut body = HashSet::default();
                body.insert(d);
                body.insert(n);

                let mut stack = vec![n];
                while let Some(m) = stack.pop() {
                    for p in predecessors(func, m) {
                        if p != d && !body.contains(&p) {
                            body.insert(p);
                            stack.push(p);
                        }
                    }
                }

                let mut exits = Vec::new();
                for &b_id in &body {
                    for &s in &successors(&func.basic_blocks[b_id.0]) {
                        if !body.contains(&s) {
                            exits.push(s);
                        }
                    }
                }

                loops.push(Loop {
                    header: d,
                    body,
                    latches: vec![n],
                    exits,
                });
            }
        }
    }
    loops
}

fn compute_dominators(func: &MirFunction) -> Vec<HashSet<BasicBlockId>> {
    let num_blocks = func.basic_blocks.len();
    let all_blocks: HashSet<BasicBlockId> = (0..num_blocks).map(BasicBlockId).collect();
    let mut dominators = vec![all_blocks.clone(); num_blocks];

    if num_blocks > 0 {
        dominators[0] = [BasicBlockId(0)].iter().cloned().collect();
    }

    let mut changed = true;
    while changed {
        changed = false;
        for i in 1..num_blocks {
            let preds = predecessors(func, BasicBlockId(i));
            let new_dom = if preds.is_empty() {
                let mut set = HashSet::default();
                set.insert(BasicBlockId(i));
                set
            } else {
                let mut set = dominators[preds[0].0].clone();
                for p in preds.iter().skip(1) {
                    set = set.intersection(&dominators[p.0]).cloned().collect();
                }
                set.insert(BasicBlockId(i));
                set
            };

            if new_dom != dominators[i] {
                dominators[i] = new_dom;
                changed = true;
            }
        }
    }
    dominators
}

fn predecessors(func: &MirFunction, target: BasicBlockId) -> Vec<BasicBlockId> {
    let mut preds = Vec::new();
    for (i, bb) in func.basic_blocks.iter().enumerate() {
        if successors(bb).contains(&target) {
            preds.push(BasicBlockId(i));
        }
    }
    preds
}

pub fn clone_blocks(
    func: &mut MirFunction,
    blocks: &HashSet<BasicBlockId>,
) -> FxHashMap<BasicBlockId, BasicBlockId> {
    let mut map = FxHashMap::default();

    for &id in blocks {
        let new_id = BasicBlockId(func.basic_blocks.len());
        map.insert(id, new_id);
        func.basic_blocks.push(BasicBlock {
            statements: Vec::new(),
            terminator: None,
        });
    }

    for &id in blocks {
        let new_id = *map.get(&id).unwrap();
        let old_bb = func.basic_blocks[id.0].clone();

        let mut new_bb = old_bb;
        if let Some(term) = &mut new_bb.terminator {
            match &mut term.kind {
                TerminatorKind::Goto { target } => {
                    if let Some(&new_target) = map.get(target) {
                        *target = new_target;
                    }
                }
                TerminatorKind::SwitchInt {
                    targets, otherwise, ..
                } => {
                    for (_, t) in targets {
                        if let Some(&new_target) = map.get(t) {
                            *t = new_target;
                        }
                    }
                    if let Some(&new_target) = map.get(otherwise) {
                        *otherwise = new_target;
                    }
                }
                _ => {}
            }
        }
        func.basic_blocks[new_id.0] = new_bb;
    }

    map
}

fn successors(bb: &BasicBlock) -> Vec<BasicBlockId> {
    match &bb.terminator {
        Some(t) => match &t.kind {
            TerminatorKind::Goto { target } => vec![*target],
            TerminatorKind::SwitchInt {
                targets, otherwise, ..
            } => {
                let mut s: Vec<_> = targets.iter().map(|(_, b)| *b).collect();
                s.push(*otherwise);
                s
            }
            _ => vec![],
        },
        None => vec![],
    }
}
