use super::summaries::{runtime_escape, task_boundary_escape};
use super::{LocalClass, push_local};
use crate::mir::*;
use crate::span::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cell::RefCell;

/// Why a compiler-inserted copy was needed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub enum CopyReason {
    EscapeBorrow,
    InteriorReturn,
    TaskBoundary,
    SpawnCapture,
}

/// One compiler-inserted copy site.
#[derive(Clone, Debug)]
pub struct CopySite {
    pub span: Span,
    pub copied_type: String,
    pub reason: CopyReason,
    pub function: String,
}

/// (bb, idx) -> copies to prepend: which operand, its source, its owning
/// temp, and which runtime copy function to call (`__olive_copy_typed` or,
/// for a task-boundary-crossing argument, `__olive_relocate_typed`).
type CopyPlan = HashMap<(usize, usize), Vec<(CopySlot, Local, Local, &'static str)>>;

/// Which operand of a store statement a copy redirects.
#[derive(Clone, Copy)]
enum CopySlot {
    /// The value operand of a `SetIndex`, `SetAttr`, or `PtrStore`.
    Val,
    /// Element `pos` of an aggregate literal.
    Agg(usize),
    /// Argument `pos` of an escaping call.
    Arg(usize),
    /// The source operand of a plain `dst = Use(op)` assignment.
    UseVal,
}

/// Deep-copies every non-owning value stored into a container so the container
/// owns an independent copy. No value is ever shared between two owners: an
/// owning path transfers by move, a non-owning path deep-copies. Eliminates
/// alias marks (no SHARED_BIT, no RC) and the quarantine leak with them.
/// When `explain_copies` is true, records each copy site into `sites`.
#[allow(clippy::too_many_arguments)]
pub(super) fn insert_escape_copies(
    func: &mut MirFunction,
    classes: &[LocalClass],
    builder_owning: &[bool],
    heap: &[bool],
    param_escapes: &HashMap<String, Vec<bool>>,
    _reassign: &HashSet<Local>,
    explain_copies: bool,
    sites: &RefCell<Vec<CopySite>>,
) -> bool {
    let needs_copy = |l: Local| -> bool {
        l.0 != 0
            && l.0 < heap.len()
            && heap[l.0]
            && (classes.get(l.0) == Some(&LocalClass::View)
                || !builder_owning[l.0]
                || classes.get(l.0) == Some(&LocalClass::Mixed))
    };

    let mut hits: Vec<(usize, usize, CopySlot, Local, &'static str)> = Vec::new();
    for (bb_idx, bb) in func.basic_blocks.iter().enumerate() {
        for (idx, stmt) in bb.statements.iter().enumerate() {
            match &stmt.kind {
                StatementKind::SetIndex(_, _, Operand::Copy(l), _)
                | StatementKind::SetAttr(_, _, Operand::Copy(l))
                | StatementKind::PtrStore(_, Operand::Copy(l))
                    if needs_copy(*l) =>
                {
                    hits.push((bb_idx, idx, CopySlot::Val, *l, "__olive_copy_typed"));
                }
                StatementKind::Assign(_, Rvalue::Aggregate(kind, ops))
                    if *kind != AggregateKind::FatPtr =>
                {
                    for (pos, op) in ops.iter().enumerate() {
                        if let Operand::Copy(l) = op
                            && needs_copy(*l)
                        {
                            hits.push((bb_idx, idx, CopySlot::Agg(pos), *l, "__olive_copy_typed"));
                        }
                    }
                }
                // A plain `dst = view` where `dst` is a real (non-view) owner:
                // e.g. a match/ternary arm whose tail is exactly a narrowed
                // binding. The view's Cast is bit-identical to its source, so
                // without a deep copy here `dst` and the view's root end up
                // aliasing the same buffer -- both later drop it.
                StatementKind::Assign(dst, Rvalue::Use(Operand::Copy(l)))
                    if needs_copy(*l)
                        && dst.0 < heap.len()
                        && heap[dst.0]
                        && builder_owning[dst.0]
                        && classes.get(dst.0) != Some(&LocalClass::View) =>
                {
                    hits.push((bb_idx, idx, CopySlot::UseVal, *l, "__olive_copy_typed"));
                }
                StatementKind::Assign(
                    _,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(callee)),
                        args,
                    },
                ) => {
                    for (pos, op) in args.iter().enumerate() {
                        let escapes = runtime_escape(callee, pos)
                            || param_escapes
                                .get(callee)
                                .is_some_and(|v| v.get(pos) == Some(&true));
                        if escapes
                            && let Operand::Copy(l) = op
                            && needs_copy(*l)
                        {
                            // A value crossing a real task boundary
                            // (`chan_send`/`mutex_new`/`mutex_unlock`) needs
                            // the copy to land in the shared escape arena,
                            // not the sending function's own arena -- see
                            // the E5.6 write-up in roadmap.md.
                            let copy_fn = if task_boundary_escape(callee, pos) {
                                "__olive_relocate_typed"
                            } else {
                                "__olive_copy_typed"
                            };
                            hits.push((bb_idx, idx, CopySlot::Arg(pos), *l, copy_fn));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if hits.is_empty() {
        return false;
    }

    let mut plan: CopyPlan = HashMap::default();
    for (bb_idx, idx, slot, l, copy_fn) in hits {
        let tmp = push_local(func, func.locals[l.0].ty.clone());
        if explain_copies {
            sites.borrow_mut().push(CopySite {
                span: func.basic_blocks[bb_idx].statements[idx].span,
                copied_type: format!("{}", func.locals[l.0].ty),
                reason: CopyReason::EscapeBorrow,
                function: func.name.clone(),
            });
        }
        plan.entry((bb_idx, idx))
            .or_default()
            .push((slot, l, tmp, copy_fn));
    }

    for (bb_idx, bb) in func.basic_blocks.iter_mut().enumerate() {
        let old = std::mem::take(&mut bb.statements);
        let mut rebuilt = Vec::with_capacity(old.len());
        for (idx, mut stmt) in old.into_iter().enumerate() {
            if let Some(list) = plan.get(&(bb_idx, idx)) {
                for &(slot, src, tmp, copy_fn) in list {
                    rebuilt.push(Statement {
                        kind: StatementKind::Assign(
                            tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(copy_fn.into())),
                                args: vec![Operand::Copy(src)],
                            },
                        ),
                        span: stmt.span,
                    });
                    redirect_operand(&mut stmt.kind, slot, tmp);
                }
            }
            rebuilt.push(stmt);
        }
        bb.statements = rebuilt;
    }
    true
}

fn redirect_operand(kind: &mut StatementKind, slot: CopySlot, tmp: Local) {
    let target = Operand::Move(tmp);
    match (slot, kind) {
        (CopySlot::Val, StatementKind::SetIndex(_, _, val, _))
        | (CopySlot::Val, StatementKind::SetAttr(_, _, val))
        | (CopySlot::Val, StatementKind::PtrStore(_, val)) => *val = target,
        (CopySlot::Agg(pos), StatementKind::Assign(_, Rvalue::Aggregate(_, ops))) => {
            ops[pos] = target
        }
        (CopySlot::Arg(pos), StatementKind::Assign(_, Rvalue::Call { args, .. })) => {
            args[pos] = target
        }
        (CopySlot::UseVal, StatementKind::Assign(_, Rvalue::Use(val))) => *val = target,
        _ => {}
    }
}
