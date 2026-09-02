use crate::mir::*;
use rustc_hash::{FxHashMap, FxHashSet as HashSet};

pub struct Loop {
    pub header: BasicBlockId,
    pub body: HashSet<BasicBlockId>,
    pub latches: Vec<BasicBlockId>,
    pub exits: Vec<BasicBlockId>,
}

struct Cfg {
    succs: Vec<Vec<BasicBlockId>>,
    preds: Vec<Vec<BasicBlockId>>,
}

fn build_cfg(func: &MirFunction) -> Cfg {
    let n = func.basic_blocks.len();
    let mut succs: Vec<Vec<BasicBlockId>> = vec![Vec::new(); n];
    let mut preds: Vec<Vec<BasicBlockId>> = vec![Vec::new(); n];
    for (i, bb) in func.basic_blocks.iter().enumerate() {
        let mut out: Vec<BasicBlockId> = match &bb.terminator {
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
        };
        out.sort_unstable_by_key(|b| b.0);
        out.dedup();
        succs[i] = out;
        for s in &succs[i] {
            if s.0 < n {
                preds[s.0].push(BasicBlockId(i));
            }
        }
    }
    Cfg { succs, preds }
}

pub fn find_loops(func: &MirFunction) -> Vec<Loop> {
    let num_blocks = func.basic_blocks.len();
    if num_blocks == 0 {
        return Vec::new();
    }

    let cfg = build_cfg(func);
    let dom_sets = dominance_sets(&cfg, num_blocks);

    let mut loops = Vec::new();
    for (n_idx, outs) in cfg.succs.iter().enumerate() {
        let n = BasicBlockId(n_idx);
        for &d in outs {
            if d.0 < num_blocks && dom_sets[n_idx].contains(&d) {
                // Natural-loop body: header plus everything reaching the
                // latch without passing through the header.
                let mut body = HashSet::default();
                body.insert(d);
                body.insert(n);
                let mut stack = vec![n];
                while let Some(m) = stack.pop() {
                    for &p in &cfg.preds[m.0] {
                        if p != d && body.insert(p) {
                            stack.push(p);
                        }
                    }
                }

                let mut exits = Vec::new();
                for &b_id in &body {
                    for &s in &cfg.succs[b_id.0] {
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
    let num_blocks = func.basic_blocks.len();
    if num_blocks == 0 {
        return Vec::new();
    }
    let cfg = build_cfg(func);
    dominance_sets(&cfg, num_blocks)
}

/// Immediate dominators via the Cooper-Harvey-Kennedy iteration over reverse
/// postorder, then full sets walked up the idom tree. Block 0 is the entry and
/// always dominated only by itself; blocks unreachable from entry keep no
/// dominator but themselves, matching the previous dataflow result.
fn dominance_sets(cfg: &Cfg, num_blocks: usize) -> Vec<HashSet<BasicBlockId>> {
    // Reverse postorder from entry.
    let mut postorder: Vec<usize> = Vec::with_capacity(num_blocks);
    let mut visited = vec![false; num_blocks];
    visited[0] = true;
    let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
    while let Some(&mut (node, ref mut child_i)) = stack.last_mut() {
        if let Some(next) = cfg.succs[node]
            .get(*child_i)
            .filter(|s| s.0 < num_blocks && !visited[s.0])
        {
            *child_i += 1;
            visited[next.0] = true;
            stack.push((next.0, 0));
        } else if *child_i < cfg.succs[node].len() {
            *child_i += 1;
        } else {
            postorder.push(node);
            stack.pop();
        }
    }
    postorder.reverse();
    let rpo: Vec<usize> = postorder;

    let mut rpo_index = vec![usize::MAX; num_blocks];
    for (i, &b) in rpo.iter().enumerate() {
        rpo_index[b] = i;
    }

    let mut idom: Vec<Option<usize>> = vec![None; num_blocks];
    idom[0] = Some(0);

    fn intersect(mut a: usize, mut b: usize, idom: &[Option<usize>], rpo_index: &[usize]) -> usize {
        while a != b {
            while rpo_index[a] > rpo_index[b] {
                a = idom[a].unwrap_or(0);
            }
            while rpo_index[b] > rpo_index[a] {
                b = idom[b].unwrap_or(0);
            }
        }
        a
    }

    let mut changed = true;
    while changed {
        changed = false;
        for &b in rpo.iter().skip(1) {
            let Some(first_pred) = cfg.preds[b]
                .iter()
                .copied()
                .find(|p| idom[p.0].is_some())
                .map(|p| p.0)
            else {
                continue;
            };
            let mut new_idom = first_pred;
            for p in &cfg.preds[b] {
                if idom[p.0].is_some() && p.0 != first_pred {
                    new_idom = intersect(new_idom, p.0, &idom, &rpo_index);
                }
            }
            if idom[b] != Some(new_idom) {
                idom[b] = Some(new_idom);
                changed = true;
            }
        }
    }

    let mut sets: Vec<HashSet<BasicBlockId>> =
        (0..num_blocks).map(|_| HashSet::default()).collect();
    for b in 0..num_blocks {
        let mut cur = b;
        loop {
            sets[b].insert(BasicBlockId(cur));
            match idom[cur] {
                Some(d) if d != cur => cur = d,
                _ => break,
            }
        }
    }
    sets
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
        let doms = dominators(&f);
        assert_eq!(doms.len(), 1);
        assert!(doms[0].contains(&BasicBlockId(0)));
    }

    #[test]
    fn dominators_linear() {
        let f = func("f", vec![bb(goto(1)), bb(TerminatorKind::Return)]);
        let doms = dominators(&f);
        assert_eq!(doms.len(), 2);
        assert!(doms[0].contains(&BasicBlockId(0)));
        assert!(doms[1].contains(&BasicBlockId(0)));
        assert!(doms[1].contains(&BasicBlockId(1)));
    }

    #[test]
    fn dominators_branch() {
        let f = func("f", vec![bb(goto(1)), bb(goto(2)), bb(goto(0))]);
        let doms = dominators(&f);
        assert_eq!(doms.len(), 3);
        // block 0 dominates all
        assert!(doms[1].contains(&BasicBlockId(0)));
        assert!(doms[2].contains(&BasicBlockId(0)));
    }

    #[test]
    fn dominators_diamond() {
        // entry -> a, entry -> b, a -> join, b -> join: neither a nor b dominates join.
        let f = func(
            "f",
            vec![
                bb(TerminatorKind::SwitchInt {
                    discr: Operand::Copy(Local(0)),
                    targets: vec![(1, BasicBlockId(1))],
                    otherwise: BasicBlockId(2),
                }),
                bb(goto(3)),
                bb(goto(3)),
                bb(TerminatorKind::Return),
            ],
        );
        let doms = dominators(&f);
        assert!(doms[3].contains(&BasicBlockId(0)));
        assert!(!doms[3].contains(&BasicBlockId(1)));
        assert!(!doms[3].contains(&BasicBlockId(2)));
    }

    #[test]
    fn find_loops_nested() {
        // outer: 0->1->2->1, 1->3(exit); inner header 1 nested in outer loop.
        let f = func(
            "f",
            vec![
                bb(goto(1)),
                bb(goto(2)),
                bb(TerminatorKind::SwitchInt {
                    discr: Operand::Copy(Local(0)),
                    targets: vec![(1, BasicBlockId(1))],
                    otherwise: BasicBlockId(3),
                }),
                bb(TerminatorKind::Return),
            ],
        );
        let loops = find_loops(&f);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].header, BasicBlockId(1));
        assert!(loops[0].body.contains(&BasicBlockId(1)));
        assert!(loops[0].body.contains(&BasicBlockId(2)));
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
    fn find_loops_multi_backedge_same_header() {
        // Two distinct latch blocks both jumping back to header 0.
        let f = func(
            "f",
            vec![
                bb(TerminatorKind::SwitchInt {
                    discr: Operand::Copy(Local(0)),
                    targets: vec![(1, BasicBlockId(1)), (2, BasicBlockId(2))],
                    otherwise: BasicBlockId(3),
                }),
                bb(goto(4)),
                bb(goto(4)),
                bb(TerminatorKind::Return),
                bb(TerminatorKind::SwitchInt {
                    discr: Operand::Copy(Local(0)),
                    targets: vec![(1, BasicBlockId(0))],
                    otherwise: BasicBlockId(3),
                }),
            ],
        );
        let loops = find_loops(&f);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].header, BasicBlockId(0));
        assert!(loops[0].body.contains(&BasicBlockId(1)));
        assert!(loops[0].body.contains(&BasicBlockId(2)));
        assert!(loops[0].body.contains(&BasicBlockId(4)));
        assert!(loops[0].exits.contains(&BasicBlockId(3)));
    }

    // Reference dominator dataflow: the O(n^2) predecessor-intersection fix
    // point the production code replaced. Kept only to differentially test
    // the CHK implementation on fully-reachable CFGs.
    fn reference_dominators(func: &MirFunction) -> Vec<HashSet<BasicBlockId>> {
        let num_blocks = func.basic_blocks.len();
        if num_blocks == 0 {
            return Vec::new();
        }
        let cfg = build_cfg(func);
        let mut dominators = vec![HashSet::default(); num_blocks];
        for i in 0..num_blocks {
            for b in 0..num_blocks {
                dominators[i].insert(BasicBlockId(b));
            }
        }
        dominators[0] = [BasicBlockId(0)].into_iter().collect();
        let mut changed = true;
        while changed {
            changed = false;
            for i in 1..num_blocks {
                let preds = &cfg.preds[i];
                let new_dom = if preds.is_empty() {
                    [BasicBlockId(i)].into_iter().collect()
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

    fn rand(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn differential_dominators_and_loops_random_reachable_cfgs() {
        let mut seed = 0x9E3779B97F4A7C15u64;
        for case in 0..300 {
            let n = 2 + (rand(&mut seed) % 24) as usize;

            // Every block gets a Goto or SwitchInt whose targets all stay in
            // range and are chosen so entry reaches every block.
            let blocks: Vec<BasicBlock> = (0..n)
                .map(|i| {
                    let term = if i + 1 == n || rand(&mut seed) % 3 == 0 {
                        TerminatorKind::Return
                    } else {
                        // Forward edge to the next block keeps the graph
                        // connected from entry; extra random edges add
                        // cycles, joins, and multi-latch loops.
                        let otherwise = BasicBlockId(i + 1);
                        let extra = rand(&mut seed) as usize % n;
                        if extra == i {
                            goto(i + 1)
                        } else {
                            TerminatorKind::SwitchInt {
                                discr: Operand::Copy(Local(0)),
                                targets: vec![(extra as i64, BasicBlockId(extra))],
                                otherwise,
                            }
                        }
                    };
                    bb(term)
                })
                .collect();

            // Both algorithms agree only on blocks reachable from entry;
            // skip generated CFGs containing dead regions (covered by the
            // unreachable-block contract tests below).
            let f = func("f", blocks);
            {
                let cfg = build_cfg(&f);
                let mut seen = vec![false; n];
                seen[0] = true;
                let mut queue = vec![0usize];
                while let Some(v) = queue.pop() {
                    for s in &cfg.succs[v] {
                        if !seen[s.0] {
                            seen[s.0] = true;
                            queue.push(s.0);
                        }
                    }
                }
                if seen.iter().any(|t| !t) {
                    continue;
                }
            }

            let got_doms = dominators(&f);
            let want_doms = reference_dominators(&f);
            assert_eq!(
                got_doms, want_doms,
                "case {case}: dominators diverged for {n} blocks"
            );

            let loops = find_loops(&f);
            for lp in &loops {
                assert!(lp.body.contains(&lp.header));
                for b in &lp.body {
                    assert!(
                        want_doms[b.0].contains(&lp.header),
                        "case {case}: header {:?} must dominate body block {:?}",
                        lp.header,
                        b
                    );
                }
                for l in &lp.latches {
                    assert!(lp.body.contains(l), "case {case}: latch not in body");
                }
                let body_succ_outside: Vec<BasicBlockId> = lp
                    .exits
                    .iter()
                    .copied()
                    .filter(|e| lp.body.contains(e))
                    .collect();
                assert!(
                    body_succ_outside.is_empty(),
                    "case {case}: exit list contains a body block"
                );
                for e in &lp.exits {
                    assert!(!lp.body.contains(e), "case {case}: exit inside body");
                }
            }
        }
    }

    #[test]
    fn unreachable_block_dominated_only_by_itself() {
        // Entry plus a dead self-loop: block 1 is unreachable.
        let f = func("f", vec![bb(TerminatorKind::Return), bb(goto(1))]);
        let doms = dominators(&f);
        assert_eq!(doms[0], [BasicBlockId(0)].into_iter().collect());
        assert_eq!(doms[1], [BasicBlockId(1)].into_iter().collect());
    }

    #[test]
    fn unreachable_cycle_yields_no_loops() {
        // Reachable 0->1 and an unreachable cycle 2->3->2. The old
        // "all blocks dominate" initialization made find_loops report
        // phantom loops inside dead code; dead blocks now keep only the
        // trivial self-dominator, so no loop can form there.
        let f = func(
            "f",
            vec![
                bb(goto(1)),
                bb(TerminatorKind::Return),
                bb(goto(3)),
                bb(goto(2)),
            ],
        );
        let loops = find_loops(&f);
        assert!(loops.is_empty(), "dead cycle must not produce loops");

        let doms = dominators(&f);
        assert_eq!(doms[2], [BasicBlockId(2)].into_iter().collect());
        assert_eq!(doms[3], [BasicBlockId(3)].into_iter().collect());
    }
}
