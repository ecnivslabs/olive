use super::Transform;
use crate::mir::liveness::Liveness;
use crate::mir::*;
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub mod summaries;
#[cfg(test)]
mod tests;

use summaries::runtime_escape;
pub use summaries::{compute_borrowed_returns, compute_param_escapes};

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
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RvClass {
    /// `dst = src` alias; may become a transfer.
    UseCopy(Local),
    /// Produces a value the destination owns.
    Own,
    /// Produces a borrow of a value owned elsewhere.
    Borrow,
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

struct AssignRec {
    bb: usize,
    idx: usize,
    dst: Local,
    class: RvClass,
    /// For `UseCopy`: source is dead after this statement.
    src_dead: bool,
}

impl Transform for OwnershipInference {
    fn run(&self, func: &mut MirFunction) -> bool {
        if func.basic_blocks.is_empty() {
            return false;
        }

        let liveness = Liveness::compute(func);
        let heap: Vec<bool> = func.locals.iter().map(|d| d.ty.is_move_type()).collect();
        let builder_owning: Vec<bool> = func.locals.iter().map(|d| d.is_owning).collect();

        let (records, arg_moves) = collect_assigns(
            func,
            &liveness,
            &heap,
            &builder_owning,
            &self.borrowed_returns,
            &self.param_escapes,
        );

        // A lone escaping call arg whose pure-owner source dies at the call
        // hands the value over outright: the callee's container owns it and
        // the nulled variable keeps the scope-end drop inert.
        for (bb, idx, pos) in arg_moves {
            if let StatementKind::Assign(_, Rvalue::Call { args, .. }) =
                &mut func.basic_blocks[bb].statements[idx].kind
                && let Operand::Copy(l) = args[pos]
            {
                args[pos] = Operand::Move(l);
            }
        }
        let (classes, transfers) = classify(func, &records, &heap, &builder_owning);

        let mut changed = false;

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

        // Roots a view may alias, for return-site drop handling.
        let mut borrow_edges: HashMap<Local, HashSet<Local>> = HashMap::default();
        for (rec_idx, rec) in records.iter().enumerate() {
            if let RvClass::UseCopy(src) = rec.class
                && !transfers.contains(&rec_idx)
            {
                borrow_edges.entry(rec.dst).or_default().insert(src);
            }
        }

        if !view_locals.is_empty() {
            for bb in &mut func.basic_blocks {
                bb.statements.retain(
                    |s| !matches!(&s.kind, StatementKind::Drop(l) if view_locals.contains(l)),
                );
            }
        }

        // A reassigned sole owner leaks the value it overwrites. Freeing it
        // first recycles the slot, but only when nothing else can still read
        // it: the local owns every value it holds (Owner), and no live view
        // aliases it (never a borrow-edge root).
        let reassign = reassign_free_locals(&classes, &heap, &records, &borrow_edges);
        for l in &reassign {
            mixed_locals.insert(*l);
        }

        let (did_insert, flag_of) =
            insert_flags_and_marks(func, &classes, &mixed_locals, &records, &transfers);
        changed |= did_insert;

        if !reassign.is_empty() {
            changed |= insert_reassign_drops(func, &reassign);
        }

        changed |= process_return_sites(func, &classes, &borrow_edges, &builder_owning);

        changed |= apply_drop_guards(func, &mixed_locals, &flag_of);

        changed |=
            insert_escape_copies(func, &classes, &builder_owning, &heap, &self.param_escapes);

        changed
    }
}

/// (bb, idx) -> copies to prepend: which operand, its source, its owning temp.
type CopyPlan = HashMap<(usize, usize), Vec<(CopySlot, Local, Local)>>;

/// Which operand of a store statement a copy redirects.
#[derive(Clone, Copy)]
enum CopySlot {
    /// The value operand of a `SetIndex`, `SetAttr`, or `PtrStore`.
    Val,
    /// Element `pos` of an aggregate literal.
    Agg(usize),
    /// Argument `pos` of an escaping call.
    Arg(usize),
}

/// Deep-copies every non-owning value stored into a container so the container
/// owns an independent copy. No value is ever shared between two owners: an
/// owning path transfers by move, a non-owning path deep-copies. Eliminates
/// alias marks (no SHARED_BIT, no RC) and the quarantine leak with them.
fn insert_escape_copies(
    func: &mut MirFunction,
    classes: &[LocalClass],
    builder_owning: &[bool],
    heap: &[bool],
    param_escapes: &HashMap<String, Vec<bool>>,
) -> bool {
    let needs_copy = |l: Local| -> bool {
        l.0 != 0
            && l.0 < heap.len()
            && heap[l.0]
            && (classes.get(l.0) == Some(&LocalClass::View)
                || !builder_owning[l.0]
                || classes.get(l.0) == Some(&LocalClass::Mixed))
    };

    let mut hits: Vec<(usize, usize, CopySlot, Local)> = Vec::new();
    for (bb_idx, bb) in func.basic_blocks.iter().enumerate() {
        for (idx, stmt) in bb.statements.iter().enumerate() {
            match &stmt.kind {
                StatementKind::SetIndex(_, _, Operand::Copy(l), _)
                | StatementKind::SetAttr(_, _, Operand::Copy(l))
                | StatementKind::PtrStore(_, Operand::Copy(l))
                    if needs_copy(*l) =>
                {
                    hits.push((bb_idx, idx, CopySlot::Val, *l));
                }
                StatementKind::Assign(_, Rvalue::Aggregate(kind, ops))
                    if *kind != AggregateKind::FatPtr =>
                {
                    for (pos, op) in ops.iter().enumerate() {
                        if let Operand::Copy(l) = op
                            && needs_copy(*l)
                        {
                            hits.push((bb_idx, idx, CopySlot::Agg(pos), *l));
                        }
                    }
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
                            hits.push((bb_idx, idx, CopySlot::Arg(pos), *l));
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
    for (bb_idx, idx, slot, l) in hits {
        let tmp = push_local(func, func.locals[l.0].ty.clone());
        plan.entry((bb_idx, idx)).or_default().push((slot, l, tmp));
    }

    for (bb_idx, bb) in func.basic_blocks.iter_mut().enumerate() {
        let old = std::mem::take(&mut bb.statements);
        let mut rebuilt = Vec::with_capacity(old.len());
        for (idx, mut stmt) in old.into_iter().enumerate() {
            if let Some(list) = plan.get(&(bb_idx, idx)) {
                for &(slot, src, tmp) in list {
                    rebuilt.push(Statement {
                        kind: StatementKind::Assign(
                            tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_copy_typed".into(),
                                )),
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
        _ => {}
    }
}

enum SiteKind {
    /// Escaping argument of a call, at this lowered position.
    CallArg(usize),
    /// Element, field, global, or aggregate store.
    DirectStore,
}

struct EscapeSite {
    bb: usize,
    idx: usize,
    local: Local,
    /// Source is dead after this statement.
    dead: bool,
    kind: SiteKind,
}

fn collect_assigns(
    func: &MirFunction,
    liveness: &Liveness,
    heap: &[bool],
    builder_owning: &[bool],
    borrowed_returns: &HashSet<String>,
    param_escapes: &HashMap<String, Vec<bool>>,
) -> (Vec<AssignRec>, Vec<(usize, usize, usize)>) {
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
                StatementKind::SetIndex(_, _, val, _)
                | StatementKind::SetAttr(_, _, val)
                | StatementKind::PtrStore(_, val) => site(val, SiteKind::DirectStore, &mut sites),
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
                    for op in ops {
                        site(op, SiteKind::DirectStore, &mut sites);
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
                Rvalue::GetIndex(_, _, _)
                | Rvalue::GetAttr(_, _)
                | Rvalue::PtrLoad(_)
                | Rvalue::FatPtrData(_)
                | Rvalue::VTableLoad { .. }
                | Rvalue::Ref(_)
                | Rvalue::MutRef(_) => RvClass::Borrow,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(name)),
                    ..
                } if borrowed_returns.contains(name) => RvClass::Borrow,
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
    for (i, s) in sites.into_iter().enumerate() {
        if s.dead && !impure[i] {
            if let SiteKind::CallArg(pos) = s.kind {
                arg_moves.push((s.bb, s.idx, pos));
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
    (records, arg_moves)
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
        if matches!(rec.class, RvClass::Borrow | RvClass::UseCopy(_)) {
            borrowish[rec.dst.0] = true;
        }
    }
    let mut def_at: HashMap<(usize, usize), Vec<(Local, DefKind)>> = HashMap::default();
    for rec in records {
        let kind = match rec.class {
            RvClass::Own => DefKind::Owning,
            RvClass::Borrow => DefKind::Impure,
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
                RvClass::Borrow | RvClass::Escape => borrow_count[rec.dst.0] += 1,
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

/// Move-type owners that are reassigned and never aliased by a live view, so
/// the value each reassignment overwrites is provably unreachable afterward
/// and can be freed first. `Owner` already excludes escapes and mixed borrow
/// paths; the view-root check excludes locals a surviving `let b = x` reads.
fn reassign_free_locals(
    classes: &[LocalClass],
    heap: &[bool],
    records: &[AssignRec],
    borrow_edges: &HashMap<Local, HashSet<Local>>,
) -> HashSet<Local> {
    let mut view_roots: HashSet<Local> = HashSet::default();
    for srcs in borrow_edges.values() {
        view_roots.extend(srcs.iter().copied());
    }
    let mut assigns = vec![0u32; classes.len()];
    for rec in records {
        assigns[rec.dst.0] += 1;
    }
    (0..classes.len())
        .filter(|&i| {
            i != 0
                && heap[i]
                && classes[i] == LocalClass::Owner
                && assigns[i] >= 2
                && !view_roots.contains(&Local(i))
        })
        .map(Local)
        .collect()
}

/// Whether `x` appears as an operand of `rval`. A reassignment whose value
/// reads the old one (`x = f(x)`) must not free `x` before the read.
fn rvalue_reads(rval: &Rvalue, x: Local) -> bool {
    let hits = |op: &Operand| matches!(op, Operand::Copy(l) | Operand::Move(l) if *l == x);
    match rval {
        Rvalue::Use(op)
        | Rvalue::UnaryOp(_, op)
        | Rvalue::Cast(op, _)
        | Rvalue::GetAttr(op, _)
        | Rvalue::GetTag(op)
        | Rvalue::GetTypeId(op)
        | Rvalue::VectorSplat(op, _)
        | Rvalue::PtrLoad(op)
        | Rvalue::FatPtrData(op)
        | Rvalue::GenOf(op) => hits(op),
        Rvalue::BinaryOp(_, a, b) | Rvalue::GetIndex(a, b, _) | Rvalue::VectorLoad(a, b, _) => {
            hits(a) || hits(b)
        }
        Rvalue::VectorFMA(a, b, c) => hits(a) || hits(b) || hits(c),
        Rvalue::Call { func, args } => hits(func) || args.iter().any(hits),
        Rvalue::Aggregate(_, ops) => ops.iter().any(hits),
        Rvalue::Ref(l) | Rvalue::MutRef(l) => *l == x,
        Rvalue::VTableLoad { vtable, .. } => hits(vtable),
    }
}

/// Frees reassigned owner before each reassignment. The unconditional drop
/// is later gated on the ownership flag so the first definition frees nothing.
fn insert_reassign_drops(func: &mut MirFunction, reassign: &HashSet<Local>) -> bool {
    let mut any = false;
    for bb in &mut func.basic_blocks {
        let old = std::mem::take(&mut bb.statements);
        // For each local, the set of other locals that may alias its slot
        // (produced by an operation reading it, e.g. in-place concat).
        let mut aliases_of: HashMap<Local, HashSet<Local>> = HashMap::default();
        for stmt in &old {
            if let StatementKind::Assign(dst, rval) = &stmt.kind {
                let each = |aliases: &mut HashMap<Local, HashSet<Local>>, op: &Operand| {
                    if let Operand::Copy(src) | Operand::Move(src) = op {
                        aliases.entry(*src).or_default().insert(*dst);
                    }
                };
                match rval {
                    Rvalue::BinaryOp(_, a, b) | Rvalue::GetIndex(a, b, _) => {
                        each(&mut aliases_of, a);
                        each(&mut aliases_of, b);
                    }
                    Rvalue::Call { args, .. } => {
                        for arg in args {
                            each(&mut aliases_of, arg);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut rebuilt = Vec::with_capacity(old.len() + 2);
        let mut assigned_in_block: HashSet<Local> = HashSet::default();
        for stmt in old {
            if let StatementKind::Assign(dst, rval) = &stmt.kind
                && reassign.contains(dst)
                && !rvalue_reads(rval, *dst)
            {
                // Skip the drop if this is the first assignment to dst (no
                // prior value to free) or if the rval moves from a local
                // that may alias dst's slot (in-place concat result shares
                // the slot, so freeing would break the alias).
                let first = !assigned_in_block.contains(dst);
                let alias_move = matches!(rval, Rvalue::Use(Operand::Move(src))
                    if aliases_of.get(dst).is_some_and(|a| a.contains(src)));
                if !first && !alias_move {
                    rebuilt.push(Statement {
                        kind: StatementKind::Drop(*dst),
                        span: stmt.span,
                    });
                    any = true;
                }
            }
            if let StatementKind::Assign(dst, _) = &stmt.kind {
                assigned_in_block.insert(*dst);
            }
            rebuilt.push(stmt);
        }
        bb.statements = rebuilt;
    }
    any
}

/// Gives each dynamically-owned local a shadow bool (set after each owning
/// assignment, cleared after each borrowing one, tested at its drops).
/// Escapes are now handled by `insert_escape_copies` (deep copy), never by
/// alias marks, so no mark emission or flag update needed for Escape records.
fn insert_flags_and_marks(
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
            RvClass::Borrow | RvClass::Neutral => false,
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

/// Transitive owning roots a view might alias, following borrow edges.
fn owning_roots(
    start: Local,
    borrow_edges: &HashMap<Local, HashSet<Local>>,
    classes: &[LocalClass],
    builder_owning: &[bool],
) -> HashSet<Local> {
    let mut roots = HashSet::default();
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
            stack.extend(srcs.iter().copied());
        }
    }
    roots
}

/// At a return of a view, the root's drop in the same block would free the
/// value being returned. A single-root pure view provably aliases it: delete
/// the drop. Otherwise keep the drop but skip it at runtime when the pointers
/// are equal.
fn process_return_sites(
    func: &mut MirFunction,
    classes: &[LocalClass],
    borrow_edges: &HashMap<Local, HashSet<Local>>,
    builder_owning: &[bool],
) -> bool {
    let mut changed = false;
    let mut guards: Vec<(usize, usize)> = Vec::new();

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
        let roots = owning_roots(v, borrow_edges, classes, builder_owning);
        if roots.is_empty() {
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
/// `if l$owned { Drop(l) }`.
fn apply_drop_guards(
    func: &mut MirFunction,
    mixed: &HashSet<Local>,
    flag_of: &HashMap<Local, Local>,
) -> bool {
    if mixed.is_empty() {
        return false;
    }
    let mut changed = false;
    loop {
        let mut target: Option<(usize, usize, Local)> = None;
        'scan: for (bb_idx, bb) in func.basic_blocks.iter().enumerate() {
            for (idx, stmt) in bb.statements.iter().enumerate() {
                if let StatementKind::Drop(l) = &stmt.kind
                    && mixed.contains(l)
                    && !guarded_by_flag(func, bb_idx, flag_of[l])
                {
                    target = Some((bb_idx, idx, *l));
                    break 'scan;
                }
            }
        }
        let Some((bb_idx, idx, l)) = target else {
            return changed;
        };
        guard_drop_with_flag(func, bb_idx, idx, flag_of[&l]);
        changed = true;
    }
}

/// A drop block created by `guard_drop_with_flag` is entered through a
/// `SwitchInt` on the flag; entering it again from the rescan loop would
/// nest guards forever.
fn guarded_by_flag(func: &MirFunction, bb_idx: usize, flag: Local) -> bool {
    func.basic_blocks.iter().any(|bb| {
        matches!(
            &bb.terminator,
            Some(Terminator {
                kind: TerminatorKind::SwitchInt { discr: Operand::Copy(d), targets, .. },
                ..
            }) if *d == flag && targets.iter().any(|(_, t)| t.0 == bb_idx)
        )
    })
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

fn push_local(func: &mut MirFunction, ty: Type) -> Local {
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
