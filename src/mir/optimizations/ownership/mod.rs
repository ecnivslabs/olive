use super::Transform;
use crate::mir::liveness::Liveness;
use crate::mir::*;
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

mod escape_copies;
mod guards;
mod reassign;
pub mod summaries;
#[cfg(test)]
mod tests;

use escape_copies::insert_escape_copies;
use guards::{apply_drop_guards, insert_flags_and_marks, process_return_sites};
use reassign::{insert_reassign_drops, reassign_free_locals};
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
            }
        }

        let (classes, transfers) = classify(func, &records, &heap, &builder_owning);

        let mut changed = !moved_from.is_empty();

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
        let reassign = reassign_free_locals(func, &classes, &heap, &records, &borrow_edges);
        for l in &reassign {
            mixed_locals.insert(*l);
        }

        let (did_insert, flag_of) =
            insert_flags_and_marks(func, &classes, &mixed_locals, &records, &transfers);
        changed |= did_insert;

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
            let stmts = &func.basic_blocks[bb].statements;
            for (j, stmt) in stmts.iter().enumerate().skip(idx + 1) {
                match &stmt.kind {
                    StatementKind::Drop(l) if *l == src => {
                        drop_removals.push((bb, j));
                        break;
                    }
                    StatementKind::Assign(d, _) if *d == src => break,
                    _ => {}
                }
            }
        }
        drop_removals.sort_unstable_by(|a, b| b.cmp(a));
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
                Rvalue::GetIndex(base, _, _)
                | Rvalue::GetAttr(base, _)
                | Rvalue::PtrLoad(base)
                | Rvalue::FatPtrData(base) => RvClass::Borrow(borrow_base(base, heap)),
                Rvalue::Ref(l) | Rvalue::MutRef(l) => {
                    RvClass::Borrow((l.0 < heap.len() && heap[l.0]).then_some(*l))
                }
                Rvalue::VTableLoad { .. } => RvClass::Borrow(None),
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(name)),
                    ..
                } if borrowed_returns.contains(name) => RvClass::Borrow(None),
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
