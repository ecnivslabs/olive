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

/// For each block, the set of blocks that dominate it (including itself).
pub fn dominators(func: &MirFunction) -> Vec<HashSet<BasicBlockId>> {
    compute_dominators(func)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sp() -> crate::span::Span {
        crate::span::Span {
            file_id: 0,
            line: 0,
            col: 0,
            start: 0,
            end: 0,
        }
    }

    fn func(name: &str, blocks: Vec<BasicBlock>) -> MirFunction {
        MirFunction {
            name: name.into(),
            locals: vec![],
            basic_blocks: blocks,
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    fn bb(term: TerminatorKind) -> BasicBlock {
        BasicBlock {
            statements: vec![],
            terminator: Some(Terminator {
                kind: term,
                span: sp(),
            }),
        }
    }

    fn goto(target: usize) -> TerminatorKind {
        TerminatorKind::Goto {
            target: BasicBlockId(target),
        }
    }

    #[test]
    fn dominators_single_block() {
        let f = func("f", vec![bb(TerminatorKind::Return)]);
        let doms = compute_dominators(&f);
        assert_eq!(doms.len(), 1);
        assert!(doms[0].contains(&BasicBlockId(0)));
    }

    #[test]
    fn dominators_linear() {
        let f = func("f", vec![bb(goto(1)), bb(TerminatorKind::Return)]);
        let doms = compute_dominators(&f);
        assert_eq!(doms.len(), 2);
        assert!(doms[0].contains(&BasicBlockId(0)));
        assert!(doms[1].contains(&BasicBlockId(0)));
        assert!(doms[1].contains(&BasicBlockId(1)));
    }

    #[test]
    fn dominators_branch() {
        let f = func("f", vec![bb(goto(1)), bb(goto(2)), bb(goto(0))]);
        let doms = compute_dominators(&f);
        assert_eq!(doms.len(), 3);
        // block 0 dominates all
        assert!(doms[1].contains(&BasicBlockId(0)));
        assert!(doms[2].contains(&BasicBlockId(0)));
    }

    #[test]
    fn find_loops_none() {
        let f = func("f", vec![bb(goto(1)), bb(TerminatorKind::Return)]);
        let loops = find_loops(&f);
        assert!(loops.is_empty());
    }

    #[test]
    fn find_loops_simple_backedge() {
        // block0 -> block1 -> block0 = loop
        let f = func("f", vec![bb(goto(1)), bb(goto(0))]);
        let loops = find_loops(&f);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].header, BasicBlockId(0));
    }

    #[test]
    fn find_loops_empty() {
        let f = func("f", vec![]);
        let loops = find_loops(&f);
        assert!(loops.is_empty());
    }

    #[test]
    fn clone_blocks_duplicates() {
        let f = func("f", vec![bb(goto(1)), bb(TerminatorKind::Return)]);
        let mut f2 = f;
        let mut body = HashSet::default();
        body.insert(BasicBlockId(1));
        let map = clone_blocks(&mut f2, &body);
        assert_eq!(f2.basic_blocks.len(), 3); // original 2 + 1 cloned
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&BasicBlockId(1)));
    }

    #[test]
    fn clone_blocks_remaps_goto() {
        let mut f = func("f", vec![bb(goto(1)), bb(TerminatorKind::Return)]);
        let mut body = HashSet::default();
        body.insert(BasicBlockId(0));
        body.insert(BasicBlockId(1));
        let _map = clone_blocks(&mut f, &body);
        // blocks 0 and 1 cloned, so we have 4 blocks
        assert_eq!(f.basic_blocks.len(), 4);
    }

    #[test]
    fn loop_exits_detected() {
        // block0 -> block1, block1 -> block2 (exit), block1 -> block0 (backedge)
        let f = func(
            "f",
            vec![
                bb(goto(1)),
                bb(TerminatorKind::SwitchInt {
                    discr: Operand::Copy(Local(0)),
                    targets: vec![(0, BasicBlockId(0))],
                    otherwise: BasicBlockId(2),
                }),
                bb(TerminatorKind::Return),
            ],
        );
        let loops = find_loops(&f);
        assert_eq!(loops.len(), 1);
        // the exit should be block 2
        assert!(
            loops[0].exits.contains(&BasicBlockId(2)),
            "loop exit should be block 2"
        );
    }

    #[test]
    fn predecessor_linear() {
        let f = func("f", vec![bb(goto(1)), bb(TerminatorKind::Return)]);
        let preds_1 = predecessors(&f, BasicBlockId(1));
        assert_eq!(preds_1, vec![BasicBlockId(0)]);
    }

    #[test]
    fn predecessor_switch() {
        let f = func("f", vec![bb(goto(1)), bb(TerminatorKind::Return)]);
        let preds_0 = predecessors(&f, BasicBlockId(0));
        assert!(preds_0.is_empty());
    }
}
