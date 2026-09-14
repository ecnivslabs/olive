use super::Transform;
use crate::mir::liveness::Liveness;
use crate::mir::*;
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cell::RefCell;

mod escape_copies;
mod guards;
mod reassign;
pub mod summaries;
#[cfg(test)]
mod tests;

use escape_copies::insert_escape_copies;
pub use escape_copies::{CopyReason, CopySite};
use escape_copies::{py_call_coll_tags, py_call_tag_for_pos};
use guards::{apply_drop_guards, insert_flags_and_marks, process_return_sites};
pub(crate) use reassign::REASSIGN_LIVE_BORROWS;
use reassign::{insert_reassign_drops, reassign_free_locals};
pub use summaries::{compute_borrowed_returns, compute_param_escapes};
use summaries::{runtime_borrowed_return, runtime_escape};

pub(crate) fn python_call_collection_tags(
    statements: &[Statement],
    index: usize,
    destination: Local,
    len: usize,
) -> Option<Vec<i64>> {
    py_call_coll_tags(statements, index, destination).map(|source| {
        (0..len)
            .map(|pos| py_call_tag_for_pos(&source, pos))
            .collect()
    })
}

/// Classifies heap locals as owner, view, or dynamic, then makes drops agree.
///
/// Builder lowers every use as Copy and emits Drop for every heap local. This
/// pass reclassifies each from its assignments. An alias whose source is an
/// owner and dead after is rewritten to Move. Stores into containers transfer
/// ownership; when the source may not own, deep copies instead of alias marks
/// (no SHARED_BIT, no RC, no quarantine leak). Returns that alias a value
/// guard the root's drop with a raw pointer compare. borrowed_returns marks
/// functions whose result may be a borrow.
pub struct OwnershipInference {
    pub borrowed_returns: HashSet<String>,
    pub param_escapes: HashMap<String, Vec<bool>>,
    /// Mangled names of every trait method reachable through a vtable
    /// (`__vtable_{trait}_{struct}` entries). Their params arrive as raw
    /// caller words via `call_indirect` -- the escape summaries only cover
    /// named calls, so no caller-side copy-in exists for them and their
    /// in-callee defensive copies must stay.
    pub vtable_methods: HashSet<String>,
    pub explain_copies: bool,
    pub copy_sites: RefCell<Vec<CopySite>>,
}

impl OwnershipInference {
    /// Resolve every tainted `_return` assignment (see `tainted_return_assigns`)
    /// into an unambiguous transfer. A maybe-borrow return strands its object
    /// when the callee built it fresh (the caller holds a view and drops the
    /// word on the floor, one object per call) and double-frees when it is
    /// genuinely borrowed (the caller would own what the owner keeps).
    ///
    /// A plain-value source rewrites to a copy plus a move, with a drop of the
    /// source only when it is builder-owning: the rest of the pass guards
    /// (Mixed), removes (View), or runs (Owner) that drop, while a borrow
    /// source (param, view temp) owns nothing and keeps no drop at all. (The
    /// builder excludes the returned operand from scope-end drops, which is
    /// why the owning case needs its drop spelled out here.) A read or
    /// runtime-borrow call instead splits into a temp holding the single
    /// evaluation, then the same copy plus move, so the container keeps its
    /// storage and the caller owns the copy. Calls to maybe-borrow MIR
    /// functions are left alone, since their own returns already went
    /// through this same split.
    ///
    /// Runs before the rest of the pass so drops, flags, and reassign see
    /// the final shape.
    fn own_tainted_returns(&self, func: &mut MirFunction) -> bool {
        if func.is_async || !self.borrowed_returns.contains(&func.name) {
            return false;
        }
        let sites = summaries::tainted_return_assigns(func, &self.borrowed_returns);
        if sites.is_empty() {
            return false;
        }
        // Descending per block so earlier indices stay valid while splicing.
        let mut ordered = sites;
        ordered.sort_unstable_by(|a, b| b.cmp(a));
        for (bb, idx) in ordered {
            let span = func.basic_blocks[bb].statements[idx].span;
            let StatementKind::Assign(dst, rval) =
                func.basic_blocks[bb].statements[idx].kind.clone()
            else {
                continue;
            };
            if dst != Local(0) {
                continue;
            }
            if let Rvalue::Call {
                func: Operand::Constant(Constant::Function(name)),
                ..
            } = &rval
                && self.borrowed_returns.contains(name.as_str())
            {
                continue;
            }
            let ret_ty = func.locals[0].ty.clone();
            let tmp = push_local(func, ret_ty.clone());
            let mut replacement: Vec<Statement> = Vec::new();
            match rval {
                Rvalue::Use(Operand::Copy(r)) | Rvalue::Use(Operand::Move(r)) if r != Local(0) => {
                    replacement.push(Statement {
                        kind: StatementKind::Assign(
                            tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_copy_typed".into(),
                                )),
                                args: vec![Operand::Copy(r)],
                            },
                        ),
                        span,
                    });
                    // Only a builder-owning source can hold a disposal: a
                    // borrow (param, view temp) owns nothing, so there is no
                    // drop to keep, and emitting one would free the owner's
                    // object out from under it (params classify External, not
                    // View, so no later machinery would neutralize it).
                    if func.locals[r.0].is_owning {
                        replacement.push(Statement {
                            kind: StatementKind::Drop(r),
                            span,
                        });
                    }
                }
                _ => {
                    let t = push_local(func, ret_ty.clone());
                    replacement.push(Statement {
                        kind: StatementKind::Assign(t, rval),
                        span,
                    });
                    replacement.push(Statement {
                        kind: StatementKind::Assign(
                            tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_copy_typed".into(),
                                )),
                                args: vec![Operand::Copy(t)],
                            },
                        ),
                        span,
                    });
                }
            }
            replacement.push(Statement {
                kind: StatementKind::Assign(Local(0), Rvalue::Use(Operand::Move(tmp))),
                span,
            });
            func.basic_blocks[bb]
                .statements
                .splice(idx..=idx, replacement);
            if self.explain_copies {
                self.copy_sites.borrow_mut().push(CopySite {
                    span,
                    copied_type: format!("{ret_ty}"),
                    reason: CopyReason::InteriorReturn,
                    function: func.name.clone(),
                });
            }
        }
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RvClass {
    /// `dst = src` alias; may become a transfer.
    UseCopy(Local),
    /// Produces a value the destination owns.
    Own,
    /// Borrow of a value owned elsewhere; carries the base local if named (element/field read).
    Borrow(Option<Local>),
    /// No heap value (constants, self-assign).
    Neutral,
    /// Not an assignment: the local's value was stored beyond this frame
    /// (into a callee, an element, a field, a global, or an aggregate), so
    /// ownership left here. Clears the drop flag like a borrowing assignment,
    /// but nothing is rewritten or root-tracked.
    Escape,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LocalClass {
    Owner,
    View,
    Mixed,
    /// Not a heap local or already non-owning from the builder.
    External,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum EdgeKind {
    /// The view aliases the whole value of the source.
    Alias,
    /// The view points into an element or field of the source.
    Interior,
}

/// view -> (source, how the view relates to it)
type BorrowEdges = HashMap<Local, HashSet<(Local, EdgeKind)>>;

struct AssignRec {
    bb: usize,
    idx: usize,
    dst: Local,
    class: RvClass,
    /// For `UseCopy`: source is dead after this statement.
    src_dead: bool,
}

/// Cached statement positions go stale whenever a pass step inserts or
/// removes statements ahead of them (view-drop cleanup, flag insertion):
/// a drop search starting from a stale index stops at the source's own
/// definition, mistakes it for a redefinition, and leaves a dead Drop
/// behind, which the generation checker then reads as a genuine
/// use-after-free (E0708). These translate a cached index across one such
/// step. Callers must apply them after every layout-changing step, before
/// any later consumer of the cached positions.
fn shift_after_removals(removed: &HashMap<usize, Vec<usize>>, bb: usize, idx: usize) -> usize {
    idx - removed
        .get(&bb)
        .map(|v| v.iter().filter(|&&p| p < idx).count())
        .unwrap_or(0)
}

fn shift_after_inserts(
    base: &HashMap<usize, usize>,
    after: &HashMap<(usize, usize), usize>,
    bb: usize,
    idx: usize,
) -> usize {
    let mut out = idx + base.get(&bb).copied().unwrap_or(0);
    for ((b, k), n) in after {
        if *b == bb && *k < idx {
            out += n;
        }
    }
    out
}

impl Transform for OwnershipInference {
    fn run(&self, func: &mut MirFunction) -> bool {
        if func.basic_blocks.is_empty() {
            return false;
        }

        let mut changed = self.own_tainted_returns(func);

        let liveness = Liveness::compute(func);
        let heap: Vec<bool> = func.locals.iter().map(|d| d.ty.needs_drop()).collect();
        let builder_owning: Vec<bool> = func.locals.iter().map(|d| d.is_owning).collect();

        let (mut records, arg_moves, direct_store_moves, agg_moves) =
            collect_assigns(func, &liveness, &heap, &builder_owning, &self.param_escapes);

        // Promoted Move hands src's value to dst; src's stale scope-end Drop must go too.
        let mut moved_from: Vec<(usize, usize, Local)> = Vec::new();

        // Escaping arg whose owner-source dies at the call transfers outright; its Drop goes stale too.
        for (bb, idx, pos) in arg_moves {
            if let StatementKind::Assign(_, Rvalue::Call { args, .. }) =
                &mut func.basic_blocks[bb].statements[idx].kind
                && let Operand::Copy(l) = args[pos]
            {
                args[pos] = Operand::Move(l);
                moved_from.push((bb, idx, l));
            }
        }

        // Same transfer for aggregate literal operands whose owner-source dies at creation.
        for (bb, idx, pos) in agg_moves {
            if let StatementKind::Assign(_, Rvalue::Aggregate(_, ops)) =
                &mut func.basic_blocks[bb].statements[idx].kind
                && let Operand::Copy(l) = ops[pos]
            {
                ops[pos] = Operand::Move(l);
                moved_from.push((bb, idx, l));
            }
        }

        // Same transfer for a direct store (`SetIndex`/`SetAttr`/`PtrStore`)
        // whose source solely owns its value there and dies with the
        // statement: the container becomes the sole owner, so the source's
        // own scope-end Drop must go stale too, exactly as for a call arg.
        // Without this a struct-in-union temp stored into a field (e.g.
        // `session.child = spawn()`) keeps its own unconditional Drop,
        // which then frees the resource out from under the field that now
        // holds the same handle, a use-after-free on first access.
        // `SetIndex` transfers its dict key the same way: the map keeps the
        // key word itself, so a dead-after heap key (a struct box in an
        // erased dict) must move or its scope-end Drop frees it while the
        // map still points at it.
        for (bb, idx, is_key) in direct_store_moves {
            if is_key {
                let l = match &func.basic_blocks[bb].statements[idx].kind {
                    StatementKind::SetIndex(_, Operand::Copy(l), _, _) => Some(*l),
                    _ => None,
                };
                let Some(l) = l else { continue };
                match &mut func.basic_blocks[bb].statements[idx].kind {
                    StatementKind::SetIndex(_, key, _, _) => *key = Operand::Move(l),
                    _ => unreachable!(),
                }
                moved_from.push((bb, idx, l));
                continue;
            }
            let l = match &func.basic_blocks[bb].statements[idx].kind {
                StatementKind::SetIndex(_, _, Operand::Copy(l), _) => Some(*l),
                StatementKind::SetAttr(_, _, Operand::Copy(l)) => Some(*l),
                StatementKind::PtrStore(_, Operand::Copy(l)) => Some(*l),
                _ => None,
            };
            let Some(l) = l else { continue };
            match &mut func.basic_blocks[bb].statements[idx].kind {
                StatementKind::SetIndex(_, _, val, _)
                | StatementKind::SetAttr(_, _, val)
                | StatementKind::PtrStore(_, val) => *val = Operand::Move(l),
                _ => unreachable!(),
            }
            moved_from.push((bb, idx, l));
        }
        // `str_concat_inplace` always consumes its left operand's storage; a
        // dead-after copy there is really a last use. Runs here, before any
        // block-mutating step below, because it indexes `bb.statements`
        // fresh against `liveness`'s original layout: reading it after a
        // pass has inserted or removed statements walks stale indices.
        for (bb_idx, bb) in func.basic_blocks.iter_mut().enumerate() {
            for (idx, stmt) in bb.statements.iter_mut().enumerate() {
                if let StatementKind::Assign(_, Rvalue::BinaryOp(op, l_op, _)) = &mut stmt.kind
                    && *op == crate::parser::BinOp::Add
                    && let Operand::Copy(l) = *l_op
                    && l.0 < heap.len()
                    && heap[l.0]
                    && builder_owning[l.0]
                    && func.locals[l.0].ty == Type::Str
                    && !liveness.live_after[bb_idx][idx + 1].contains(&l)
                {
                    *l_op = Operand::Move(l);
                    moved_from.push((bb_idx, idx, l));
                }
                // List concat whose operands both die here reuses the left
                // list's storage and steals the right's elements, skipping
                // the deep copy entirely. E0500 already rejects reassigning
                // a borrowed list, so no live view can observe the reuse.
                if let StatementKind::Assign(_, Rvalue::Call { func: f, args }) = &mut stmt.kind
                    && matches!(&*f, Operand::Constant(Constant::Function(n))
                        if n == "__olive_list_concat_typed" || n == "__olive_list_concat")
                    && let [Operand::Copy(l), Operand::Copy(r)] = args.as_slice()
                {
                    let (l, r) = (*l, *r);
                    if l.0 < heap.len()
                        && r.0 < heap.len()
                        && heap[l.0]
                        && builder_owning[l.0]
                        && builder_owning[r.0]
                        && matches!(func.locals[l.0].ty, Type::List(_))
                        && matches!(func.locals[r.0].ty, Type::List(_))
                        && !liveness.live_after[bb_idx][idx + 1].contains(&l)
                        && !liveness.live_after[bb_idx][idx + 1].contains(&r)
                    {
                        *f = Operand::Constant(Constant::Function(
                            "__olive_list_concat_move".to_string(),
                        ));
                        args[0] = Operand::Move(l);
                        moved_from.push((bb_idx, idx, l));
                    }
                }
            }
        }

        let (classes, transfers) = classify(func, &records, &heap, &builder_owning);

        changed |= !moved_from.is_empty();

        for (rec_idx, rec) in records.iter().enumerate() {
            if transfers.contains(&rec_idx)
                && let RvClass::UseCopy(src) = rec.class
                && let StatementKind::Assign(_, Rvalue::Use(op)) =
                    &mut func.basic_blocks[rec.bb].statements[rec.idx].kind
            {
                *op = Operand::Move(src);
                changed = true;
            }
        }

        let mut view_locals: HashSet<Local> = HashSet::default();
        let mut mixed_locals: HashSet<Local> = HashSet::default();
        for (i, class) in classes.iter().enumerate() {
            match class {
                LocalClass::View => {
                    func.locals[i].is_owning = false;
                    view_locals.insert(Local(i));
                    changed = true;
                }
                LocalClass::Mixed => {
                    mixed_locals.insert(Local(i));
                    changed = true;
                }
                _ => {}
            }
        }

        // Roots a view may alias, for return-site drop handling (Alias = whole value, Interior = element/field).
        let mut borrow_edges: BorrowEdges = HashMap::default();
        for (rec_idx, rec) in records.iter().enumerate() {
            match rec.class {
                RvClass::UseCopy(src) if !transfers.contains(&rec_idx) => {
                    borrow_edges
                        .entry(rec.dst)
                        .or_default()
                        .insert((src, EdgeKind::Alias));
                }
                RvClass::Borrow(Some(base)) => {
                    borrow_edges
                        .entry(rec.dst)
                        .or_default()
                        .insert((base, EdgeKind::Interior));
                }
                _ => {}
            }
        }

        if !view_locals.is_empty() {
            // View Drops carry no positions anyone caches (views never own),
            // but their removal still shifts every statement after them, so
            // cached record and move positions translate across it.
            let mut removed: HashMap<usize, Vec<usize>> = HashMap::default();
            for (bb_idx, bb) in func.basic_blocks.iter_mut().enumerate() {
                let mut kept = Vec::with_capacity(bb.statements.len());
                for (idx, s) in bb.statements.drain(..).enumerate() {
                    if matches!(&s.kind, StatementKind::Drop(l) if view_locals.contains(l)) {
                        removed.entry(bb_idx).or_default().push(idx);
                    } else {
                        kept.push(s);
                    }
                }
                bb.statements = kept;
            }
            if !removed.is_empty() {
                for rec in records.iter_mut() {
                    rec.idx = shift_after_removals(&removed, rec.bb, rec.idx);
                }
                for (bb, idx, _) in moved_from.iter_mut() {
                    *idx = shift_after_removals(&removed, *bb, *idx);
                }
            }
        }

        // A reassigned sole owner leaks the value it overwrites. Freeing it
        // first recycles the slot, but only when nothing else can still read
        // it: the local owns every value it holds (Owner), and no live view
        // aliases it (never a borrow-edge root).
        let reassign = reassign_free_locals(func, &classes, &heap, &records, &borrow_edges);
        for l in &reassign {
            mixed_locals.insert(*l);
        }

        let (did_insert, flag_of, shift) =
            insert_flags_and_marks(func, &classes, &mixed_locals, &records, &transfers);
        changed |= did_insert;
        if did_insert {
            // Flag initializations and updates shift every statement after
            // them; cached record and move positions translate across, or a
            // drop search starts mid-block and mistakes the source's own
            // definition for a redefinition.
            for rec in records.iter_mut() {
                rec.idx = shift_after_inserts(&shift.base, &shift.after, rec.bb, rec.idx);
            }
            for (bb, idx, _) in moved_from.iter_mut() {
                *idx = shift_after_inserts(&shift.base, &shift.after, *bb, *idx);
            }
        }

        // Same last-use promotion, for a plain `dst = src` rebind instead of a call arg.
        for (rec_idx, rec) in records.iter().enumerate() {
            if transfers.contains(&rec_idx)
                && let RvClass::UseCopy(src) = rec.class
            {
                moved_from.push((rec.bb, rec.idx, src));
            }
        }

        // Remove each now-stale Drop, descending per block so earlier indices stay valid.
        let mut drop_removals: Vec<(usize, usize)> = Vec::new();
        for (bb, idx, src) in moved_from {
            find_drop_to_remove(func, bb, idx, src, &mut drop_removals);
        }
        drop_removals.sort_unstable_by(|a, b| b.cmp(a));
        drop_removals.dedup();
        for (bb, idx) in drop_removals {
            func.basic_blocks[bb].statements.remove(idx);
            changed = true;
        }

        if !reassign.is_empty() {
            changed |= insert_reassign_drops(func, &reassign);
        }

        changed |= process_return_sites(func, &classes, &borrow_edges, &builder_owning);

        changed |= apply_drop_guards(func, &mixed_locals, &flag_of);

        changed |= insert_escape_copies(
            func,
            &classes,
            &builder_owning,
            &heap,
            &self.param_escapes,
            &reassign,
            &self.vtable_methods,
            self.explain_copies,
            &self.copy_sites,
        );

        changed
    }
}

fn borrow_base(op: &Operand, heap: &[bool]) -> Option<Local> {
    match op {
        Operand::Copy(l) | Operand::Move(l) if l.0 < heap.len() && heap[l.0] => Some(*l),
        _ => None,
    }
}

enum SiteKind {
    /// Escaping argument of a call, at this lowered position.
    CallArg(usize),
    /// Value stored into an element, field, or global.
    DirectStoreVal,
    /// Dict key stored by `SetIndex`: the container keeps the key word
    /// itself (a struct box included), so a dead-after heap key transfers
    /// exactly like the value does. List indices are ints and never reach
    /// here through the heap gate below.
    DirectStoreKey,
    /// Aggregate element, at this lowered operand position.
    AggElem(usize),
}

struct EscapeSite {
    bb: usize,
    idx: usize,
    local: Local,
    /// Source is dead after this statement.
    dead: bool,
    kind: SiteKind,
}

/// (bb, idx, arg position) for a call-arg escape ready for move promotion.
type ArgMoveSite = (usize, usize, usize);
/// (bb, idx, is_key) for a direct-store escape ready for move promotion:
/// `false` upgrades the stored value, `true` upgrades a `SetIndex` dict key.
type DirectStoreMoveSite = (usize, usize, bool);
/// (bb, idx, operand position) for an aggregate-element escape ready for move promotion.
type AggMoveSite = (usize, usize, usize);

fn boxed_scalar_getter_returns_owned(name: &str, args: &[Operand], func: &MirFunction) -> bool {
    if !matches!(
        name,
        "__olive_obj_get_boxed"
            | "__olive_obj_get_default_boxed"
            | "__olive_obj_get_default_boxed_typed"
    ) {
        return false;
    }
    let Some(Operand::Copy(local) | Operand::Move(local)) = args.first() else {
        return false;
    };
    let recv_ty = crate::semantic::type_descriptor::concrete_ty(&func.locals[local.0].ty);
    let Type::Dict(_, value) = recv_ty else {
        return false;
    };
    let value = crate::semantic::type_descriptor::concrete_ty(value);
    matches!(
        value,
        Type::Int
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize
            | Type::Float
            | Type::F32
            | Type::IntegerLiteral(_)
            | Type::FloatLiteral(_)
            | Type::Bool
            | Type::Null
    )
}

fn collect_assigns(
    func: &MirFunction,
    liveness: &Liveness,
    heap: &[bool],
    builder_owning: &[bool],
    param_escapes: &HashMap<String, Vec<bool>>,
) -> (
    Vec<AssignRec>,
    Vec<ArgMoveSite>,
    Vec<DirectStoreMoveSite>,
    Vec<AggMoveSite>,
) {
    let mut records = Vec::new();
    // Escapes are deferred: a lone last-use escape of a pure owner transfers
    // outright, so it must not demote its source to dynamic ownership.
    let mut sites: Vec<EscapeSite> = Vec::new();

    for (bb_idx, bb) in func.basic_blocks.iter().enumerate() {
        for (idx, stmt) in bb.statements.iter().enumerate() {
            let site = |op: &Operand, kind: SiteKind, sites: &mut Vec<EscapeSite>| {
                if let Operand::Copy(l) = op
                    && l.0 != 0
                    && l.0 < heap.len()
                    && heap[l.0]
                    && builder_owning[l.0]
                {
                    let dead = !liveness.live_after[bb_idx][idx + 1].contains(l);
                    sites.push(EscapeSite {
                        bb: bb_idx,
                        idx,
                        local: *l,
                        dead,
                        kind,
                    });
                }
            };
            match &stmt.kind {
                StatementKind::SetIndex(_, idx, val, _) => {
                    site(idx, SiteKind::DirectStoreKey, &mut sites);
                    site(val, SiteKind::DirectStoreVal, &mut sites);
                }
                StatementKind::SetAttr(_, _, val) | StatementKind::PtrStore(_, val) => {
                    site(val, SiteKind::DirectStoreVal, &mut sites)
                }
                _ => {}
            }

            let StatementKind::Assign(dst, rval) = &stmt.kind else {
                continue;
            };

            // Escape scan runs regardless of the destination's type: a call
            // like obj_set assigns a scalar but still consumes a heap arg.
            match rval {
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(callee)),
                    args,
                } => {
                    for (pos, op) in args.iter().enumerate() {
                        let callee_escape = param_escapes
                            .get(callee)
                            .is_some_and(|v| v.get(pos) == Some(&true));
                        if runtime_escape(callee, pos) || callee_escape {
                            site(op, SiteKind::CallArg(pos), &mut sites);
                        }
                    }
                }
                // A fat pointer wraps a trait object without owning it.
                Rvalue::Aggregate(kind, ops) if *kind != AggregateKind::FatPtr => {
                    let py_tags = py_call_coll_tags(&bb.statements, idx, *dst);
                    for (pos, op) in ops.iter().enumerate() {
                        if let Some(src) = &py_tags
                            && py_call_tag_for_pos(src, pos) != 0
                        {
                            continue;
                        }
                        site(op, SiteKind::AggElem(pos), &mut sites);
                    }
                }
                _ => {}
            }

            if dst.0 >= heap.len() || !heap[dst.0] || dst.0 == 0 {
                continue;
            }
            let class = match rval {
                Rvalue::Use(Operand::Copy(src)) if src.0 < heap.len() && heap[src.0] => {
                    if src == dst {
                        RvClass::Neutral
                    } else {
                        RvClass::UseCopy(*src)
                    }
                }
                Rvalue::Use(Operand::Move(_)) => RvClass::Own,
                Rvalue::Use(Operand::Constant(_)) | Rvalue::Use(Operand::Copy(_)) => {
                    RvClass::Neutral
                }
                Rvalue::GetIndex(base, _, _)
                | Rvalue::GetAttr(base, _)
                | Rvalue::PtrLoad(base)
                | Rvalue::FatPtrData(base) => RvClass::Borrow(borrow_base(base, heap)),
                Rvalue::Ref(l) | Rvalue::MutRef(l) => {
                    RvClass::Borrow((l.0 < heap.len() && heap[l.0]).then_some(*l))
                }
                Rvalue::VTableLoad { .. } => RvClass::Borrow(None),
                // Only runtime calls still return true borrows. MIR-level
                // maybe-borrow functions deep-copy tainted returns in-callee
                // (`own_tainted_returns`), so their results are owned here.
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(name)),
                    args,
                } if runtime_borrowed_return(name)
                    && !boxed_scalar_getter_returns_owned(name, args, func) =>
                {
                    RvClass::Borrow(None)
                }
                _ => RvClass::Own,
            };
            let src_dead = match &class {
                RvClass::UseCopy(src) => {
                    !liveness.live_after[bb_idx][idx + 1].contains(src)
                        && builder_owning[src.0]
                        && builder_owning[dst.0]
                }
                _ => false,
            };
            records.push(AssignRec {
                bb: bb_idx,
                idx,
                dst: *dst,
                class,
                src_dead,
            });
        }
    }

    // An escape hands the value over cleanly when the source solely owns it
    // there and dies with the statement: a direct store transfers by move
    // elision, a call arg by an in-place move upgrade. Every other site
    // records a dynamic escape.
    let impure = solve_impurity(func, heap, builder_owning, &records, &sites);
    let mut arg_moves = Vec::new();
    let mut direct_store_moves = Vec::new();
    let mut agg_moves = Vec::new();
    for (i, s) in sites.into_iter().enumerate() {
        if s.dead && !impure[i] {
            match s.kind {
                SiteKind::CallArg(pos) => arg_moves.push((s.bb, s.idx, pos)),
                SiteKind::DirectStoreVal => direct_store_moves.push((s.bb, s.idx, false)),
                SiteKind::DirectStoreKey => direct_store_moves.push((s.bb, s.idx, true)),
                SiteKind::AggElem(pos) => agg_moves.push((s.bb, s.idx, pos)),
            }
            continue;
        }
        records.push(AssignRec {
            bb: s.bb,
            idx: s.idx,
            dst: s.local,
            class: RvClass::Escape,
            src_dead: false,
        });
    }
    (records, arg_moves, direct_store_moves, agg_moves)
}

/// For each escape site, whether the source may not solely own its value
/// there: it was defined by a borrow, or an earlier escape of the same
/// definition already stored it. Forward may-analysis, union at joins; an
/// owning definition resets the local to sole ownership.
fn solve_impurity(
    func: &MirFunction,
    heap: &[bool],
    builder_owning: &[bool],
    records: &[AssignRec],
    sites: &[EscapeSite],
) -> Vec<bool> {
    let n = func.locals.len();
    let nb = func.basic_blocks.len();

    // Events per block position: definition class changes and escapes. An
    // alias of a dying sole owner is itself a sole owner (the assignment
    // becomes a move); the source must have no borrow-shaped definitions,
    // or the later classification could demote that move.
    enum DefKind {
        Owning,
        Impure,
        Alias(Local),
    }
    let mut borrowish = vec![false; n];
    for rec in records {
        if matches!(rec.class, RvClass::Borrow(_) | RvClass::UseCopy(_)) {
            borrowish[rec.dst.0] = true;
        }
    }
    let mut def_at: HashMap<(usize, usize), Vec<(Local, DefKind)>> = HashMap::default();
    for rec in records {
        let kind = match rec.class {
            RvClass::Own => DefKind::Owning,
            RvClass::Borrow(_) => DefKind::Impure,
            RvClass::UseCopy(src) if rec.src_dead && !borrowish[src.0] => DefKind::Alias(src),
            RvClass::UseCopy(_) => DefKind::Impure,
            RvClass::Neutral | RvClass::Escape => continue,
        };
        def_at
            .entry((rec.bb, rec.idx))
            .or_default()
            .push((rec.dst, kind));
    }
    let mut escape_at: HashMap<(usize, usize), Vec<Local>> = HashMap::default();
    for s in sites {
        escape_at.entry((s.bb, s.idx)).or_default().push(s.local);
    }

    // Params and anything not builder-owned never solely own their value.
    let entry_state: Vec<bool> = (0..n).map(|i| !heap[i] || !builder_owning[i]).collect();

    let preds = block_preds(func);
    let mut out: Vec<Vec<bool>> = vec![entry_state.clone(); nb];
    let step = |bb: usize, state: &mut Vec<bool>, record: Option<&mut Vec<bool>>| {
        let mut record = record;
        for idx in 0..func.basic_blocks[bb].statements.len() {
            if let Some(list) = escape_at.get(&(bb, idx)) {
                for l in list {
                    if let Some(rec) = record.as_deref_mut() {
                        rec.push(state[l.0]);
                    }
                    state[l.0] = true;
                }
            }
            if let Some(list) = def_at.get(&(bb, idx)) {
                for (l, kind) in list {
                    state[l.0] = match kind {
                        DefKind::Owning => false,
                        DefKind::Impure => true,
                        DefKind::Alias(src) => state[src.0],
                    };
                }
            }
        }
    };

    let mut changed = true;
    while changed {
        changed = false;
        for bb in 0..nb {
            let mut state = if preds[bb].is_empty() {
                entry_state.clone()
            } else {
                let mut s = out[preds[bb][0].0].clone();
                for p in preds[bb].iter().skip(1) {
                    for (a, b) in s.iter_mut().zip(&out[p.0]) {
                        *a |= *b;
                    }
                }
                s
            };
            step(bb, &mut state, None);
            if state != out[bb] {
                out[bb] = state;
                changed = true;
            }
        }
    }

    // Escape order inside `sites` is block-major statement order, matching
    // the recording walk.
    let mut result = Vec::with_capacity(sites.len());
    let mut per_block: HashMap<usize, Vec<bool>> = HashMap::default();
    for bb in 0..nb {
        let mut state = if preds[bb].is_empty() {
            entry_state.clone()
        } else {
            let mut s = out[preds[bb][0].0].clone();
            for p in preds[bb].iter().skip(1) {
                for (a, b) in s.iter_mut().zip(&out[p.0]) {
                    *a |= *b;
                }
            }
            s
        };
        let mut rec = Vec::new();
        step(bb, &mut state, Some(&mut rec));
        per_block.insert(bb, rec);
    }
    let mut cursor: HashMap<usize, usize> = HashMap::default();
    for s in sites {
        let c = cursor.entry(s.bb).or_default();
        result.push(per_block[&s.bb][*c]);
        *c += 1;
    }
    result
}

fn block_preds(func: &MirFunction) -> Vec<Vec<BasicBlockId>> {
    let mut preds = vec![Vec::new(); func.basic_blocks.len()];
    for (i, bb) in func.basic_blocks.iter().enumerate() {
        if let Some(term) = &bb.terminator {
            match &term.kind {
                TerminatorKind::Goto { target } => preds[target.0].push(BasicBlockId(i)),
                TerminatorKind::SwitchInt {
                    targets, otherwise, ..
                } => {
                    for (_, t) in targets {
                        preds[t.0].push(BasicBlockId(i));
                    }
                    preds[otherwise.0].push(BasicBlockId(i));
                }
                _ => {}
            }
        }
    }
    preds
}

/// Fixpoint: a transfer is only valid while its source stays a pure owner;
/// demoting one turns the assignment into a borrow, which can reclassify the
/// destination and invalidate further transfers.
fn classify(
    func: &MirFunction,
    records: &[AssignRec],
    heap: &[bool],
    builder_owning: &[bool],
) -> (Vec<LocalClass>, HashSet<usize>) {
    let n = func.locals.len();
    let mut transfers: HashSet<usize> = records
        .iter()
        .enumerate()
        .filter(|(_, r)| r.src_dead)
        .map(|(i, _)| i)
        .collect();

    loop {
        let mut owning_count = vec![0u32; n];
        let mut borrow_count = vec![0u32; n];
        for i in 1..=func.arg_count {
            if builder_owning[i] {
                owning_count[i] = 1;
            }
        }
        for (i, rec) in records.iter().enumerate() {
            match rec.class {
                RvClass::UseCopy(_) => {
                    if transfers.contains(&i) {
                        owning_count[rec.dst.0] += 1;
                    } else {
                        borrow_count[rec.dst.0] += 1;
                    }
                }
                RvClass::Own => owning_count[rec.dst.0] += 1,
                RvClass::Borrow(_) | RvClass::Escape => borrow_count[rec.dst.0] += 1,
                RvClass::Neutral => {}
            }
        }

        let classes: Vec<LocalClass> = (0..n)
            .map(|i| {
                if !heap[i] || !builder_owning[i] || i == 0 {
                    LocalClass::External
                } else if borrow_count[i] > 0 && owning_count[i] == 0 {
                    LocalClass::View
                } else if borrow_count[i] > 0 {
                    LocalClass::Mixed
                } else {
                    LocalClass::Owner
                }
            })
            .collect();

        let before = transfers.len();
        transfers.retain(|&i| {
            let RvClass::UseCopy(src) = records[i].class else {
                return false;
            };
            classes[src.0] == LocalClass::Owner
        });
        if transfers.len() == before {
            return (classes, transfers);
        }
    }
}

pub fn push_local(func: &mut MirFunction, ty: Type) -> Local {
    let l = Local(func.locals.len());
    func.locals.push(LocalDecl {
        ty,
        name: None,
        span: Span::default(),
        is_mut: true,
        is_owning: true,
    });
    l
}

fn find_drop_to_remove(
    func: &MirFunction,
    start_bb: usize,
    start_idx: usize,
    src: Local,
    drop_removals: &mut Vec<(usize, usize)>,
) {
    let mut visited = HashSet::default();
    let mut queue = std::collections::VecDeque::new();

    let stmts = &func.basic_blocks[start_bb].statements;
    let mut found_in_start = false;
    for (j, stmt) in stmts.iter().enumerate().skip(start_idx + 1) {
        match &stmt.kind {
            StatementKind::Drop(l) if *l == src => {
                drop_removals.push((start_bb, j));
                found_in_start = true;
                break;
            }
            StatementKind::Assign(d, _) if *d == src => {
                found_in_start = true;
                break;
            }
            _ => {}
        }
    }

    if !found_in_start {
        visited.insert(start_bb);
        if let Some(term) = &func.basic_blocks[start_bb].terminator {
            for succ in term_successors(term) {
                if visited.insert(succ) {
                    queue.push_back(succ);
                }
            }
        }

        while let Some(bb) = queue.pop_front() {
            let mut stopped = false;
            for (j, stmt) in func.basic_blocks[bb].statements.iter().enumerate() {
                match &stmt.kind {
                    StatementKind::Drop(l) if *l == src => {
                        drop_removals.push((bb, j));
                        stopped = true;
                        break;
                    }
                    StatementKind::Assign(d, _) if *d == src => {
                        stopped = true;
                        break;
                    }
                    _ => {}
                }
            }
            if !stopped && let Some(term) = &func.basic_blocks[bb].terminator {
                for succ in term_successors(term) {
                    if visited.insert(succ) {
                        queue.push_back(succ);
                    }
                }
            }
        }
    }
}

fn term_successors(term: &Terminator) -> Vec<usize> {
    match &term.kind {
        TerminatorKind::Goto { target } => vec![target.0],
        TerminatorKind::SwitchInt {
            targets, otherwise, ..
        } => {
            let mut succs = Vec::with_capacity(targets.len() + 1);
            for (_, t) in targets {
                succs.push(t.0);
            }
            succs.push(otherwise.0);
            succs
        }
        TerminatorKind::Return | TerminatorKind::Unreachable => vec![],
    }
}
