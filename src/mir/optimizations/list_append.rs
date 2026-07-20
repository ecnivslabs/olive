use super::Transform;
use crate::mir::loop_utils::dominators;
use crate::mir::{AggregateKind, BasicBlockId, Constant, Local, MirFunction, Operand};
use crate::mir::{Rvalue, Statement, StatementKind, TerminatorKind};
use rustc_hash::FxHashSet as HashSet;

/// Rewrites `xs = xs + [a, b, ...]` into direct appends.
///
/// Ownership already turns that concat into `__olive_list_concat_move`, so the
/// deep copy is gone, but the right operand is still a freshly built list that
/// exists only to be drained and freed one statement later. In an append loop
/// that is an allocation and a free per iteration. Pushing the literal's
/// elements straight onto the left list drops both.
///
/// The literal and the concat rarely share a block: conditional-drop flags put
/// a diamond between them. So the match spans blocks, guarded by dominance plus
/// an interference walk over everything that can run in between.
pub struct ListAppend;

fn operand_local(op: &Operand) -> Option<Local> {
    match op {
        Operand::Copy(l) | Operand::Move(l) => Some(*l),
        Operand::Constant(_) => None,
    }
}

fn reads(stmt: &StatementKind, t: Local) -> bool {
    let mut locals = Vec::new();
    match stmt {
        StatementKind::Assign(dst, rv) => {
            if *dst == t {
                return true;
            }
            super::rvalue_operand_locals(rv, &mut locals);
        }
        StatementKind::SetAttr(o, _, v) | StatementKind::PtrStore(o, v) => {
            locals.extend(operand_local(o));
            locals.extend(operand_local(v));
        }
        StatementKind::SetIndex(o, i, v, _) | StatementKind::VectorStore(o, i, v) => {
            locals.extend(operand_local(o));
            locals.extend(operand_local(i));
            locals.extend(operand_local(v));
        }
        StatementKind::GenCheck { value, generation } => {
            locals.push(*value);
            locals.push(*generation);
        }
        StatementKind::StorageLive(_) | StatementKind::StorageDead(_) | StatementKind::Drop(_) => {}
    }
    locals.contains(&t)
}

/// Whether `stmt` can change what any local in `watched` holds. The element
/// operands and the left list are read at the concat instead of at the literal
/// now, so anything in between that touches them blocks the fold.
fn clobbers(stmt: &StatementKind, watched: &HashSet<Local>) -> bool {
    match stmt {
        StatementKind::Assign(dst, _) => watched.contains(dst),
        StatementKind::Drop(l) => watched.contains(l),
        StatementKind::SetAttr(o, _, _) | StatementKind::PtrStore(o, _) => {
            operand_local(o).is_some_and(|l| watched.contains(&l))
        }
        StatementKind::SetIndex(o, _, _, _) | StatementKind::VectorStore(o, _, _) => {
            operand_local(o).is_some_and(|l| watched.contains(&l))
        }
        StatementKind::StorageDead(l) => watched.contains(l),
        StatementKind::StorageLive(_) | StatementKind::GenCheck { .. } => false,
    }
}

fn is_marker_for(stmt: &StatementKind, t: Local) -> bool {
    matches!(
        stmt,
        StatementKind::StorageLive(x) | StatementKind::StorageDead(x) | StatementKind::Drop(x)
            if *x == t
    )
}

fn successors(func: &MirFunction, bb: usize) -> Vec<usize> {
    let Some(term) = &func.basic_blocks[bb].terminator else {
        return Vec::new();
    };
    match &term.kind {
        TerminatorKind::Goto { target } => vec![target.0],
        TerminatorKind::SwitchInt {
            targets, otherwise, ..
        } => targets
            .iter()
            .map(|(_, t)| t.0)
            .chain(std::iter::once(otherwise.0))
            .collect(),
        TerminatorKind::Return | TerminatorKind::Unreachable => Vec::new(),
    }
}

struct Site {
    def_bb: usize,
    def_idx: usize,
    use_bb: usize,
    use_idx: usize,
    dst: Local,
    left: Local,
    temp: Local,
    ops: Vec<Operand>,
}

impl ListAppend {
    /// `temp` may be referenced only by its definition and the concat. Storage
    /// and drop markers are exempt: the rewrite deletes them with the literal.
    fn temp_is_private(func: &MirFunction, s: &Site) -> bool {
        for (bi, block) in func.basic_blocks.iter().enumerate() {
            for (si, stmt) in block.statements.iter().enumerate() {
                if (bi == s.def_bb && si == s.def_idx) || (bi == s.use_bb && si == s.use_idx) {
                    continue;
                }
                if is_marker_for(&stmt.kind, s.temp) {
                    continue;
                }
                if reads(&stmt.kind, s.temp) {
                    return false;
                }
            }
            if let Some(term) = &block.terminator
                && let TerminatorKind::SwitchInt { discr, .. } = &term.kind
                && operand_local(discr) == Some(s.temp)
            {
                return false;
            }
        }
        true
    }

    /// Walks every statement that can execute between the literal and the
    /// concat, and reports whether any of them clobbers a value the appends
    /// now read later than before.
    fn path_is_clear(func: &MirFunction, s: &Site) -> bool {
        let mut watched: HashSet<Local> = HashSet::default();
        watched.insert(s.left);
        watched.extend(s.ops.iter().filter_map(operand_local));

        if s.def_bb == s.use_bb {
            return !func.basic_blocks[s.def_bb].statements[s.def_idx + 1..s.use_idx]
                .iter()
                .any(|st| clobbers(&st.kind, &watched));
        }

        if func.basic_blocks[s.def_bb].statements[s.def_idx + 1..]
            .iter()
            .any(|st| clobbers(&st.kind, &watched))
        {
            return false;
        }
        if func.basic_blocks[s.use_bb].statements[..s.use_idx]
            .iter()
            .any(|st| clobbers(&st.kind, &watched))
        {
            return false;
        }

        // Blocks strictly between the two, reached without going through the
        // concat's block.
        let mut seen: HashSet<usize> = HashSet::default();
        let mut stack: Vec<usize> = successors(func, s.def_bb);
        while let Some(bb) = stack.pop() {
            if bb == s.use_bb || !seen.insert(bb) {
                continue;
            }
            if func.basic_blocks[bb]
                .statements
                .iter()
                .any(|st| clobbers(&st.kind, &watched))
            {
                return false;
            }
            stack.extend(successors(func, bb));
        }
        true
    }

    fn find_site(func: &MirFunction, doms: &[HashSet<BasicBlockId>]) -> Option<Site> {
        for (use_bb, block) in func.basic_blocks.iter().enumerate() {
            for (use_idx, stmt) in block.statements.iter().enumerate() {
                let StatementKind::Assign(dst, Rvalue::Call { func: callee, args }) = &stmt.kind
                else {
                    continue;
                };
                if !matches!(callee, Operand::Constant(Constant::Function(n))
                    if n == "__olive_list_concat_move")
                {
                    continue;
                }
                let [Operand::Move(left), Operand::Copy(temp)] = args.as_slice() else {
                    continue;
                };

                let Some((def_bb, def_idx, ops)) = Self::find_literal(func, doms, *temp, use_bb)
                else {
                    continue;
                };
                let site = Site {
                    def_bb,
                    def_idx,
                    use_bb,
                    use_idx,
                    dst: *dst,
                    left: *left,
                    temp: *temp,
                    ops,
                };
                if Self::temp_is_private(func, &site) && Self::path_is_clear(func, &site) {
                    return Some(site);
                }
            }
        }
        None
    }

    /// The unique list literal defining `temp`, in a block that dominates the
    /// concat so the literal always runs first.
    fn find_literal(
        func: &MirFunction,
        doms: &[HashSet<BasicBlockId>],
        temp: Local,
        use_bb: usize,
    ) -> Option<(usize, usize, Vec<Operand>)> {
        let mut found = None;
        for (bi, block) in func.basic_blocks.iter().enumerate() {
            for (si, stmt) in block.statements.iter().enumerate() {
                if let StatementKind::Assign(d, Rvalue::Aggregate(AggregateKind::List, ops)) =
                    &stmt.kind
                    && *d == temp
                {
                    if found.is_some() {
                        return None;
                    }
                    found = Some((bi, si, ops.clone()));
                }
            }
        }
        let (def_bb, def_idx, ops) = found?;
        if !doms[use_bb].contains(&BasicBlockId(def_bb)) {
            return None;
        }
        Some((def_bb, def_idx, ops))
    }

    fn apply(func: &mut MirFunction, s: Site) {
        let span = func.basic_blocks[s.use_bb].statements[s.use_idx].span;
        let mut appends: Vec<Statement> = Vec::with_capacity(s.ops.len().max(1));
        if s.ops.is_empty() {
            appends.push(Statement {
                kind: StatementKind::Assign(s.dst, Rvalue::Use(Operand::Move(s.left))),
                span,
            });
        } else {
            for (i, op) in s.ops.into_iter().enumerate() {
                let src = if i == 0 { s.left } else { s.dst };
                appends.push(Statement {
                    kind: StatementKind::Assign(
                        s.dst,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_list_push".to_string(),
                            )),
                            args: vec![Operand::Move(src), op],
                        },
                    ),
                    span,
                });
            }
        }

        func.basic_blocks[s.use_bb]
            .statements
            .splice(s.use_idx..=s.use_idx, appends);
        func.basic_blocks[s.def_bb].statements.remove(s.def_idx);

        // The literal never exists now, so its storage and drop markers would
        // act on an uninitialized local.
        for block in func.basic_blocks.iter_mut() {
            block.statements.retain(|st| !is_marker_for(&st.kind, s.temp));
        }
    }
}

impl Transform for ListAppend {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        loop {
            let doms = dominators(func);
            let Some(site) = Self::find_site(func, &doms) else {
                break;
            };
            Self::apply(func, site);
            changed = true;
        }
        changed
    }
}
