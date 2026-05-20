use super::{AssignRec, BorrowEdges, EdgeKind, LocalClass, RvClass, push_local};
use crate::mir::*;
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Gives each dynamically-owned local a shadow bool (set after each owning
/// assignment, cleared after each borrowing one, tested at its drops).
/// Escapes are now handled by `insert_escape_copies` (deep copy), never by
/// alias marks, so no mark emission or flag update needed for Escape records.
pub(super) fn insert_flags_and_marks(
    func: &mut MirFunction,
    _classes: &[LocalClass],
    mixed: &HashSet<Local>,
    records: &[AssignRec],
    transfers: &HashSet<usize>,
) -> (bool, HashMap<Local, Local>) {
    let mut flag_of: HashMap<Local, Local> = HashMap::default();
    let mut ordered: Vec<Local> = mixed.iter().copied().collect();
    ordered.sort_unstable_by_key(|l| l.0);
    let mut flags = Vec::with_capacity(ordered.len());
    for l in ordered {
        let flag = push_local(func, Type::Bool);
        flag_of.insert(l, flag);
        flags.push(flag);
    }

    // (bb, idx) -> flag updates to insert after that statement.
    let mut updates: HashMap<(usize, usize), Vec<(Local, bool)>> = HashMap::default();
    for (i, rec) in records.iter().enumerate() {
        let Some(&flag) = flag_of.get(&rec.dst) else {
            continue;
        };
        // Escapes are handled by insert_escape_copies (deep copy). The local
        // keeps whatever ownership it had (the container gets a copy, not a
        // transfer), so no flag update needed.
        if rec.class == RvClass::Escape {
            continue;
        }
        let owns = match rec.class {
            RvClass::UseCopy(_) => transfers.contains(&i),
            RvClass::Own => true,
            RvClass::Borrow(_) | RvClass::Neutral => false,
            _ => unreachable!(),
        };
        updates
            .entry((rec.bb, rec.idx))
            .or_default()
            .push((flag, owns));
    }

    if flags.is_empty() {
        return (false, flag_of);
    }

    for (bb_idx, bb) in func.basic_blocks.iter_mut().enumerate() {
        let old = std::mem::take(&mut bb.statements);
        let mut rebuilt = Vec::with_capacity(old.len() + 4);
        if bb_idx == 0 {
            for &flag in &flags {
                rebuilt.push(Statement {
                    kind: StatementKind::StorageLive(flag),
                    span: Span::default(),
                });
                rebuilt.push(Statement {
                    kind: StatementKind::Assign(
                        flag,
                        Rvalue::Use(Operand::Constant(Constant::Bool(false))),
                    ),
                    span: Span::default(),
                });
            }
        }
        for (idx, stmt) in old.into_iter().enumerate() {
            let span = stmt.span;
            rebuilt.push(stmt);
            if let Some(list) = updates.get(&(bb_idx, idx)) {
                for &(flag, owns) in list {
                    rebuilt.push(Statement {
                        kind: StatementKind::Assign(
                            flag,
                            Rvalue::Use(Operand::Constant(Constant::Bool(owns))),
                        ),
                        span,
                    });
                }
            }
        }
        bb.statements = rebuilt;
    }

    (true, flag_of)
}

/// Transitive owning roots a view might alias; `interior` marks a path that crossed an element/field borrow.
fn owning_roots(
    start: Local,
    borrow_edges: &BorrowEdges,
    classes: &[LocalClass],
    builder_owning: &[bool],
) -> (HashSet<Local>, bool) {
    let mut roots = HashSet::default();
    let mut interior = false;
    let mut stack = vec![start];
    let mut seen: HashSet<Local> = HashSet::default();
    while let Some(l) = stack.pop() {
        if !seen.insert(l) {
            continue;
        }
        if l != start
            && builder_owning[l.0]
            && matches!(classes[l.0], LocalClass::Owner | LocalClass::Mixed)
        {
            roots.insert(l);
            continue;
        }
        if let Some(srcs) = borrow_edges.get(&l) {
            for (src, kind) in srcs {
                if *kind == EdgeKind::Interior {
                    interior = true;
                }
                stack.push(*src);
            }
        }
    }
    (roots, interior)
}

/// At a view's return: alias deletes the root's drop, interior copies out first, else guard by pointer compare.
///
/// Invariant: the MIR builder places every root drop in the same basic block
/// as its `_return = value` assignment, so this pass only scans the current
/// block. Cross-block drops cannot be produced by the builder; if that ever
/// changes, process_return_sites must follow the drop into successor blocks.
pub(super) fn process_return_sites(
    func: &mut MirFunction,
    classes: &[LocalClass],
    borrow_edges: &BorrowEdges,
    builder_owning: &[bool],
) -> bool {
    let mut changed = false;
    let mut guards: Vec<(usize, usize)> = Vec::new();
    let mut copies: Vec<(usize, usize, Local)> = Vec::new();

    for bb_idx in 0..func.basic_blocks.len() {
        let mut ret_local: Option<Local> = None;
        let mut assign_idx = 0usize;
        for (idx, stmt) in func.basic_blocks[bb_idx].statements.iter().enumerate() {
            if let StatementKind::Assign(dst, Rvalue::Use(op)) = &stmt.kind
                && dst.0 == 0
                && let Operand::Copy(l) | Operand::Move(l) = op
            {
                ret_local = Some(*l);
                assign_idx = idx;
            }
        }
        let Some(v) = ret_local else { continue };
        if v.0 >= classes.len() || !matches!(classes[v.0], LocalClass::View | LocalClass::Mixed) {
            continue;
        }
        let (roots, interior) = owning_roots(v, borrow_edges, classes, builder_owning);
        if roots.is_empty() {
            continue;
        }
        if interior {
            copies.push((bb_idx, assign_idx, v));
            continue;
        }
        let single_pure = roots.len() == 1 && classes[v.0] == LocalClass::View;

        let mut idx = assign_idx + 1;
        while idx < func.basic_blocks[bb_idx].statements.len() {
            let is_root_drop = matches!(
                &func.basic_blocks[bb_idx].statements[idx].kind,
                StatementKind::Drop(r) if roots.contains(r)
            );
            if is_root_drop {
                if single_pure {
                    func.basic_blocks[bb_idx].statements.remove(idx);
                    changed = true;
                    continue;
                }
                guards.push((bb_idx, idx));
            }
            idx += 1;
        }
    }

    // Splitting appends blocks and truncates the split block, so handling
    // sites bottom-up keeps the remaining indices valid.
    guards.sort_unstable_by(|a, b| b.cmp(a));
    for (bb_idx, idx) in guards {
        guard_drop_with_ptr_ne(func, bb_idx, idx);
        changed = true;
    }

    for (bb_idx, idx, v) in copies {
        let tmp = push_local(func, func.locals[v.0].ty.clone());
        let span = func.basic_blocks[bb_idx].statements[idx].span;
        let copy_call = Statement {
            kind: StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_copy_typed".into())),
                    args: vec![Operand::Copy(v)],
                },
            ),
            span,
        };
        func.basic_blocks[bb_idx].statements[idx] = Statement {
            kind: StatementKind::Assign(Local(0), Rvalue::Use(Operand::Move(tmp))),
            span,
        };
        func.basic_blocks[bb_idx].statements.insert(idx, copy_call);
        changed = true;
    }
    changed
}

/// Rewrites `Drop(r)` into `if r as raw word != _return { Drop(r) }`.
/// Uses Int-typed temps so codegen emits raw pointer compare, never deep
/// value comparison.
fn guard_drop_with_ptr_ne(func: &mut MirFunction, bb_idx: usize, drop_idx: usize) {
    let StatementKind::Drop(dropped) = func.basic_blocks[bb_idx].statements[drop_idx].kind else {
        return;
    };
    let span = func.basic_blocks[bb_idx].statements[drop_idx].span;

    let t1 = push_local(func, Type::Int);
    let t2 = push_local(func, Type::Int);
    let cond = push_local(func, Type::Bool);

    let mut tail = func.basic_blocks[bb_idx].statements.split_off(drop_idx);
    let drop_stmt = tail.remove(0);
    let term = func.basic_blocks[bb_idx].terminator.take();

    let cont_id = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: tail,
        terminator: term,
    });
    let drop_id = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: vec![drop_stmt],
        terminator: Some(Terminator {
            kind: TerminatorKind::Goto { target: cont_id },
            span,
        }),
    });

    let bb = &mut func.basic_blocks[bb_idx];
    for (l, rv) in [
        (t1, Rvalue::Use(Operand::Copy(dropped))),
        (t2, Rvalue::Use(Operand::Copy(Local(0)))),
    ] {
        bb.statements.push(Statement {
            kind: StatementKind::StorageLive(l),
            span,
        });
        bb.statements.push(Statement {
            kind: StatementKind::Assign(l, rv),
            span,
        });
    }
    bb.statements.push(Statement {
        kind: StatementKind::StorageLive(cond),
        span,
    });
    bb.statements.push(Statement {
        kind: StatementKind::Assign(
            cond,
            Rvalue::BinaryOp(
                crate::parser::BinOp::NotEq,
                Operand::Copy(t1),
                Operand::Copy(t2),
            ),
        ),
        span,
    });
    bb.terminator = Some(Terminator {
        kind: TerminatorKind::SwitchInt {
            discr: Operand::Copy(cond),
            targets: vec![(1, drop_id)],
            otherwise: cont_id,
        },
        span,
    });
}

/// Rewrites `Drop(l)` of a dynamically-owned local into
/// `if l$owned { Drop(l) }`. Collects every unguarded-drop site in one pass,
/// then applies guards bottom-up: guarding splits the block and appends new
/// blocks, so descending `(bb, idx)` order keeps earlier indices valid for
/// sites still pending in the same block.
pub(super) fn apply_drop_guards(
    func: &mut MirFunction,
    mixed: &HashSet<Local>,
    flag_of: &HashMap<Local, Local>,
) -> bool {
    if mixed.is_empty() {
        return false;
    }
    let mut targets: Vec<(usize, usize, Local)> = Vec::new();
    for (bb_idx, bb) in func.basic_blocks.iter().enumerate() {
        for (idx, stmt) in bb.statements.iter().enumerate() {
            if let StatementKind::Drop(l) = &stmt.kind
                && mixed.contains(l)
            {
                targets.push((bb_idx, idx, *l));
            }
        }
    }
    if targets.is_empty() {
        return false;
    }
    targets.sort_unstable_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    for (bb_idx, idx, l) in targets {
        guard_drop_with_flag(func, bb_idx, idx, flag_of[&l]);
    }
    true
}

fn guard_drop_with_flag(func: &mut MirFunction, bb_idx: usize, drop_idx: usize, flag: Local) {
    let span = func.basic_blocks[bb_idx].statements[drop_idx].span;

    let mut tail = func.basic_blocks[bb_idx].statements.split_off(drop_idx);
    let drop_stmt = tail.remove(0);
    let term = func.basic_blocks[bb_idx].terminator.take();

    let cont_id = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: tail,
        terminator: term,
    });
    let drop_id = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: vec![drop_stmt],
        terminator: Some(Terminator {
            kind: TerminatorKind::Goto { target: cont_id },
            span,
        }),
    });

    func.basic_blocks[bb_idx].terminator = Some(Terminator {
        kind: TerminatorKind::SwitchInt {
            discr: Operand::Copy(flag),
            targets: vec![(1, drop_id)],
            otherwise: cont_id,
        },
        span,
    });
}
