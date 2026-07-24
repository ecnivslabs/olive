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
    /// The left operand of a `BinaryOp`.
    BinLhs,
}

/// The `args_list` aggregate holds one slot per positional argument, so its
/// packed tag word indexes 1:1 with the aggregate's element positions. The
/// `kwargs_list` aggregate instead alternates a constant name string and a
/// value per keyword argument, so its tag word (one nibble per *keyword*,
/// per `pack_tags` on the compiler side) indexes the value at ops
/// position `2*n + 1`, not position `n`.
pub(super) enum PyCallTagSource {
    Args(i64),
    Kwargs(i64),
    /// R15's `kwvals_list`: values only, no interleaved name constant, so
    /// tag index `n` is aggregate position `n` directly.
    KwargsFlat(i64),
}

/// Operand positions of one `__olive_py_call*` entry point's `args_list` and
/// packed collection-tag constant, plus (for a kwargs-taking variant) the
/// same pair for `kwargs_list`. `kw_flat` is true when that aggregate holds
/// values only (R15's `kwvals_list`) rather than interleaved name/value
/// pairs (every shape before it).
struct PyCallShape {
    args_list_pos: usize,
    coll_tags_pos: usize,
    kwargs: Option<(usize, usize)>,
    kw_flat: bool,
}

/// Every `__olive_py_call*` runtime entry point this file must recognize,
/// with its exact operand positions. R5 added a tagged-fast-path shape
/// (`__olive_py_call_t*`) with an extra `arg_tags` word ahead of
/// `kwargs_list`, shifting its positions relative to the legacy shape -- an
/// exact per-name lookup, not a positional heuristic, is what keeps a
/// future signature change from silently desyncing this analysis from the
/// real call.
///
/// `args`/`kwargs` `[Any]` aggregates these functions consume are handed
/// back to the caller's own allocation, never retained: a collection
/// argument tagged for copy-out (a non-zero nibble in the packed tag word)
/// must reach the runtime as the caller's own pointer, not a defensive
/// copy, or `sync_back` mutates a throwaway clone instead of the value the
/// caller keeps using. See `python_writeback.rs`'s tag vocabulary.
fn py_call_shape(name: &str) -> Option<PyCallShape> {
    match name {
        "__olive_py_call"
        | "__olive_py_call_safe"
        | "__olive_py_call_t"
        | "__olive_py_call_t_safe" => Some(PyCallShape {
            args_list_pos: 1,
            coll_tags_pos: 2,
            kwargs: None,
            kw_flat: false,
        }),
        "__olive_py_call_kw" | "__olive_py_call_kw_safe" => Some(PyCallShape {
            args_list_pos: 1,
            coll_tags_pos: 2,
            kwargs: Some((3, 4)),
            kw_flat: false,
        }),
        // R15: `kwvals_list` holds values only (names are a compile-time
        // packed constant, not a runtime aggregate), so its tag word
        // indexes 1:1 with its own positions, unlike `_kw`'s interleaved
        // `2*n + 1` stride.
        "__olive_py_call_kw_v" | "__olive_py_call_kw_v_safe" => Some(PyCallShape {
            args_list_pos: 1,
            coll_tags_pos: 2,
            kwargs: Some((5, 6)),
            kw_flat: true,
        }),
        "__olive_py_call_method_kw_v" | "__olive_py_call_method_kw_v_safe" => Some(PyCallShape {
            args_list_pos: 2,
            coll_tags_pos: 3,
            kwargs: Some((6, 7)),
            kw_flat: true,
        }),
        _ => None,
    }
}

/// If `stmts[idx]` builds the `[Any]` aggregate `dst` that a later
/// `__olive_py_call*` statement in the same basic block consumes as its
/// `args_list` or `kwargs_list`, returns that call's matching packed
/// collection-tag word. `dst` is a fresh temp created solely for this one
/// call (never a user-named binding reused elsewhere), so scanning the rest
/// of the straight-line block for the first call that references it is
/// exact, not a heuristic guess -- the callee-name/attr-getattr bookkeeping
/// lowering emits ahead of the call (`__olive_py_getattr`, `__olive_py_set_loc`,
/// their own temps) means the real gap is not a fixed statement count.
pub(super) fn py_call_coll_tags(
    stmts: &[Statement],
    idx: usize,
    dst: Local,
) -> Option<PyCallTagSource> {
    let is_dst = |op: &Operand| matches!(op, Operand::Copy(l) | Operand::Move(l) if *l == dst);
    for stmt in &stmts[idx + 1..] {
        let StatementKind::Assign(
            _,
            Rvalue::Call {
                func: Operand::Constant(Constant::Function(name)),
                args,
            },
        ) = &stmt.kind
        else {
            continue;
        };
        let Some(shape) = py_call_shape(name) else {
            continue;
        };
        if args.len() > shape.coll_tags_pos
            && is_dst(&args[shape.args_list_pos])
            && let Operand::Constant(Constant::Int(tags)) = &args[shape.coll_tags_pos]
        {
            return Some(PyCallTagSource::Args(*tags));
        }
        if let Some((kw_list_pos, kw_coll_pos)) = shape.kwargs
            && args.len() > kw_coll_pos
            && is_dst(&args[kw_list_pos])
            && let Operand::Constant(Constant::Int(tags)) = &args[kw_coll_pos]
        {
            return Some(if shape.kw_flat {
                PyCallTagSource::KwargsFlat(*tags)
            } else {
                PyCallTagSource::Kwargs(*tags)
            });
        }
    }
    None
}

/// Reads argument `i`'s 4-bit tag from a packed collection-tag word. Mirrors
/// `python_writeback::tag_at` (a different crate, no shared dependency).
fn tag_at(tags: i64, i: usize) -> i64 {
    if i >= 16 {
        return 0;
    }
    (tags >> (i * 4)) & 0xF
}

/// The effective copy-out tag for aggregate element `pos`, translating a
/// `kwargs_list`'s interleaved name/value layout back to a keyword index.
pub(super) fn py_call_tag_for_pos(src: &PyCallTagSource, pos: usize) -> i64 {
    match src {
        PyCallTagSource::Args(tags) | PyCallTagSource::KwargsFlat(tags) => tag_at(*tags, pos),
        PyCallTagSource::Kwargs(tags) if pos % 2 == 1 => tag_at(*tags, pos / 2),
        PyCallTagSource::Kwargs(_) => 0,
    }
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

    // `Any` sits outside the heap/ownership classification (`is_move_type`
    // is false: its payload may be a raw scalar), yet an `Any` word read out
    // of a container is still a borrow of that container's storage. When such
    // a view outlives its owner's Drop -- returned to the caller, stored into
    // another container, or passed to an escaping call -- the word must be
    // deep-copied first or it dangles. `__olive_copy_typed` with the `Any`
    // descriptor dispatches on the runtime kind, so a raw-scalar payload is
    // returned unchanged and only heap-backed payloads allocate.
    let any_views = collect_any_views(func);
    let is_any_view = |l: Local| -> bool { l.0 != 0 && any_views.contains(&l) };

    let mut hits: Vec<(usize, usize, CopySlot, Local, &'static str)> = Vec::new();
    for (bb_idx, bb) in func.basic_blocks.iter().enumerate() {
        for (idx, stmt) in bb.statements.iter().enumerate() {
            match &stmt.kind {
                StatementKind::SetIndex(_, _, Operand::Copy(l), _)
                | StatementKind::SetAttr(_, _, Operand::Copy(l))
                | StatementKind::PtrStore(_, Operand::Copy(l))
                    if needs_copy(*l) || is_any_view(*l) =>
                {
                    hits.push((bb_idx, idx, CopySlot::Val, *l, "__olive_copy_typed"));
                }
                // Returning an `Any` view hands the caller a pointer into a
                // local container that the epilogue is about to drop.
                StatementKind::Assign(dst, Rvalue::Use(Operand::Copy(l)))
                    if dst.0 == 0 && is_any_view(*l) =>
                {
                    hits.push((bb_idx, idx, CopySlot::UseVal, *l, "__olive_copy_typed"));
                }
                StatementKind::Assign(dst, Rvalue::Aggregate(kind, ops))
                    if *kind != AggregateKind::FatPtr =>
                {
                    let py_tags = py_call_coll_tags(&bb.statements, idx, *dst);
                    for (pos, op) in ops.iter().enumerate() {
                        if let Some(src) = &py_tags
                            && py_call_tag_for_pos(src, pos) != 0
                        {
                            continue;
                        }
                        if let Operand::Copy(l) = op
                            && (needs_copy(*l) || is_any_view(*l))
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
                // `str_concat_inplace` grows or frees its left operand's own
                // storage: a `+` chain that ends this local's life (proven by
                // the ownership pass promoting that use to `Move`) may run it
                // in place, but any `Copy` reaching this point means the
                // local is still needed afterward -- the runtime has no way
                // to tell "sole owner" from "shared" apart, so a surviving
                // `Copy` here must hand it an independent buffer instead.
                StatementKind::Assign(_, Rvalue::BinaryOp(op, Operand::Copy(l), _))
                    if *op == crate::parser::BinOp::Add
                        && l.0 != 0
                        && l.0 < heap.len()
                        && heap[l.0]
                        && func.locals[l.0].ty == crate::semantic::types::Type::Str =>
                {
                    hits.push((bb_idx, idx, CopySlot::BinLhs, *l, "__olive_copy_typed"));
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
                            && (needs_copy(*l) || is_any_view(*l))
                        {
                            // A value crossing a real task boundary
                            // (`chan_send`/`mutex_new`/`mutex_unlock`) needs
                            // the copy to land in the shared escape arena,
                            // not the sending function's own arena.
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

/// `Any`-typed locals holding a borrowed view of a container's storage: any
/// local assigned (at least once) from an indexed or attribute read, plus
/// everything a plain `Use` copies such a view into. A local that is ever
/// view-assigned is treated as a view at every use -- copying an owned `Any`
/// is only a cost, never a correctness problem, so the over-approximation on
/// mixed reassignment is safe.
fn collect_any_views(func: &MirFunction) -> HashSet<Local> {
    let is_any = |l: Local| {
        func.locals
            .get(l.0)
            .is_some_and(|d| d.ty == crate::semantic::types::Type::Any)
    };
    let mut views: HashSet<Local> = HashSet::default();
    loop {
        let before = views.len();
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                let StatementKind::Assign(dst, rv) = &stmt.kind else {
                    continue;
                };
                if !is_any(*dst) {
                    continue;
                }
                match rv {
                    Rvalue::GetIndex(_, _, _) | Rvalue::GetAttr(_, _) => {
                        views.insert(*dst);
                    }
                    Rvalue::Use(Operand::Copy(src)) | Rvalue::Use(Operand::Move(src))
                        if views.contains(src) =>
                    {
                        views.insert(*dst);
                    }
                    _ => {}
                }
            }
        }
        if views.len() == before {
            return views;
        }
    }
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
        (CopySlot::BinLhs, StatementKind::Assign(_, Rvalue::BinaryOp(_, lhs, _))) => *lhs = target,
        _ => {}
    }
}
