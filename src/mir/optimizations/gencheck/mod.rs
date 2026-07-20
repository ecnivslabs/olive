//! Generation-check insertion. Runs after every other pass, so checks aren't
//! moved or duplicated. Suspects (views, element/field reads, escaped
//! owners, borrowed-return calls) capture the slab generation at borrow
//! time and re-validate before each use that a free could precede. Failed
//! check aborts with E0707. Checks elided by forward must-analysis; dead
//! at drops of potential owners and non-runtime calls. Globals never
//! checked. Element borrows validate at creation when an alias mark exists.

use super::Transform;
use crate::compile::errors::Diagnostic;
use crate::mir::*;
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cell::RefCell;

mod must_stale;
#[cfg(test)]
mod tests;

pub struct GenCheckInsertion {
    pub borrowed_returns: HashSet<String>,
    pub param_escapes: HashMap<String, Vec<bool>>,
    /// E0708 reports collected across every function this pass has run on.
    /// `Transform::run` takes `&self`, so this is the only way to hand
    /// compile errors back to the caller without changing that signature.
    pub diagnostics: RefCell<Vec<Diagnostic>>,
}

/// Whether values of this type live in generational slab slots, so the word
/// at `ptr - 8` is a valid generation. Types excluded: `Any` (boxed scalars
/// use BOXED_SLAB but the Any local is never checked), Python objects,
/// FFI-backed structs, and `Future` (box-allocated, no generation word).
/// Strings are slab-backed too but tagged and literal-tolerant, so they
/// check through a runtime helper (`str_backed`). `Bytes` is slab-backed via
/// BYTES_SLAB. `Fn` (a closure record, E5.2) is slab-backed through the same
/// pool ordinary structs use, but flows through `struct_backed` below like a
/// struct does, not through this predicate.
pub(crate) fn slab_backed(ty: &Type) -> bool {
    match ty {
        Type::List(_)
        | Type::Tuple(_)
        | Type::Set(_)
        | Type::Dict(_, _)
        | Type::Enum(_, _)
        | Type::Bytes => true,
        Type::Union(members) => {
            let non_null: Vec<_> = members
                .iter()
                .filter(|m| !matches!(m, Type::Null))
                .collect();
            matches!(non_null.as_slice(), [m] if slab_backed(m))
        }
        _ => false,
    }
}

/// Whether values of this type are heap strings whose borrows validate through
/// the literal-tolerant runtime helper instead of a raw `ptr - 8` read. A
/// string pointer is tagged and may address a `.rodata` literal with no
/// generation word, so codegen dispatches these to `olive_str_gen_*`.
pub(crate) fn str_backed(ty: &Type) -> bool {
    match ty {
        Type::Str => true,
        Type::Union(members) => {
            let non_null: Vec<_> = members
                .iter()
                .filter(|m| !matches!(m, Type::Null))
                .collect();
            matches!(non_null.as_slice(), [m] if str_backed(m))
        }
        _ => false,
    }
}

pub(crate) fn struct_backed(ty: &Type) -> bool {
    match ty {
        Type::Struct(_, _, is_ffi) => !is_ffi,
        // A closure record (`build_closure_value`) is allocated via the same
        // `__olive_struct_alloc` slab pool as an ordinary struct and carries
        // the same generation word at `ptr - 8`, so it checks through the
        // identical `__olive_struct_gen_*` runtime helpers.
        Type::Fn(..) => true,
        Type::Union(members) => {
            let non_null: Vec<_> = members
                .iter()
                .filter(|m| !matches!(m, Type::Null))
                .collect();
            matches!(non_null.as_slice(), [m] if struct_backed(m))
        }
        _ => false,
    }
}

/// A type whose borrows are generation-checked, by either slab path.
pub(crate) fn checkable(ty: &Type) -> bool {
    slab_backed(ty) || str_backed(ty) || struct_backed(ty)
}

/// Runtime helpers never free an object header; only user code can, through
/// its own drops. Async entry points switch tasks, which runs foreign drops.
fn is_safe_call(callee: &Operand) -> bool {
    match callee {
        Operand::Constant(Constant::Function(name)) => {
            name.starts_with("__olive_") && !name.starts_with("__olive_aio")
        }
        _ => false,
    }
}

#[derive(Default, Clone)]
struct SuspectInfo {
    /// This local is a borrow or an escaped owner somewhere.
    suspect: bool,
    /// Element/field/global-read definitions (stale-at-birth candidates).
    elem_def: bool,
    /// Owner unknown: a borrowed call result, or an owner that escaped
    /// through a call; any drop or unsafe call may free the value.
    unknown: bool,
    /// Locals whose drop may free this value (aliased sources, containers
    /// read from or escaped into).
    roots: HashSet<Local>,
    /// Where this local was first marked a suspect; the E0708 caret's
    /// "borrowed here" anchor.
    borrow_span: Option<Span>,
}

impl Transform for GenCheckInsertion {
    fn run(&self, func: &mut MirFunction) -> bool {
        if func.basic_blocks.is_empty() {
            return false;
        }
        let n = func.locals.len();
        let checkable: Vec<bool> = func.locals.iter().map(|d| checkable(&d.ty)).collect();

        let (mut info, aliases) = self.collect_suspects(func);
        merge_aliases(&mut info, &aliases);
        close_roots(&mut info);
        let suspects: Vec<Local> = (0..n)
            .filter(|&i| i != 0 && info[i].suspect && checkable[i])
            .map(Local)
            .collect();
        if suspects.is_empty() {
            return false;
        }
        let index_of: HashMap<Local, usize> =
            suspects.iter().enumerate().map(|(i, &l)| (l, i)).collect();

        let must_hits = must_stale::find_must_stale(func, &info, &checkable);
        let must_sites: HashSet<(usize, usize, Local)> =
            must_hits.iter().map(|h| (h.bb, h.idx, h.local)).collect();
        for hit in &must_hits {
            self.diagnostics
                .borrow_mut()
                .push(e0708_for(func, &info, hit));
        }

        let (mut check_sites, mut def_checks) = self.solve(func, &info, &suspects, &index_of);
        // A use proven stale on every path is rejected above, not checked.
        check_sites.retain(|s| !must_sites.contains(s));
        def_checks.retain(|s| !must_sites.contains(s));

        if check_sites.is_empty() && def_checks.is_empty() {
            return !must_hits.is_empty();
        }

        self.insert(func, &info, check_sites, def_checks);
        true
    }
}

/// The three-way caret for a definite-staleness rejection: where the value
/// was borrowed, where it was given away, and where it was used after.
fn e0708_for(
    func: &MirFunction,
    info: &[SuspectInfo],
    hit: &must_stale::MustStaleUse,
) -> Diagnostic {
    let use_span = if hit.idx == usize::MAX {
        func.basic_blocks[hit.bb]
            .terminator
            .as_ref()
            .map(|t| t.span)
            .unwrap_or_default()
    } else {
        func.basic_blocks[hit.bb].statements[hit.idx].span
    };
    let borrow_span = info[hit.local.0].borrow_span.unwrap_or_default();
    Diagnostic::error("E0708", "use of a value after it was given away", use_span)
        .label("used here after it was definitely given away")
        .secondary(borrow_span, "borrowed here")
        .secondary(hit.free_span, "freed or escaped here")
        .note(format!("in function `{}`", func.name))
}

/// A statement position where a check goes in front, keyed by original
/// indices before any insertion.
type Site = (usize, usize, Local);

/// Sinks the solver emits into: use-site checks and validate-at-birth defs.
type Emit<'a> = (&'a mut Vec<Site>, &'a mut Vec<Site>);

impl GenCheckInsertion {
    fn collect_suspects(&self, func: &MirFunction) -> (Vec<SuspectInfo>, Vec<(Local, Local)>) {
        let n = func.locals.len();
        let mut info: Vec<SuspectInfo> = vec![SuspectInfo::default(); n];
        let mut aliases: Vec<(Local, Local)> = Vec::new();
        // Chain nodes include `Any` locals: a checkable value borrowed or
        // stored through an untyped hop is still the same object.
        let tracked: Vec<bool> = func
            .locals
            .iter()
            .map(|d| d.ty.is_move_type() || matches!(d.ty, Type::Any))
            .collect();

        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                // Escape positions: the stored local gains the container as a
                // root; through a call the owner becomes untrackable.
                let escape = |op: &Operand, owner: Option<Local>, info: &mut Vec<SuspectInfo>| {
                    if let Operand::Copy(l) | Operand::Move(l) = op
                        && l.0 != 0
                        && tracked.get(l.0).copied().unwrap_or(false)
                    {
                        info[l.0].suspect = true;
                        info[l.0].borrow_span.get_or_insert(stmt.span);
                        match owner {
                            Some(c) => {
                                info[l.0].roots.insert(c);
                            }
                            None => info[l.0].unknown = true,
                        }
                    }
                };
                match &stmt.kind {
                    StatementKind::SetIndex(obj, _, val, _)
                    | StatementKind::SetAttr(obj, _, val) => {
                        let owner = match obj {
                            Operand::Copy(c) | Operand::Move(c) => Some(*c),
                            _ => None,
                        };
                        escape(val, owner, &mut info);
                    }
                    // Global storage is never freed mid-run: the value gains
                    // an owner that cannot die, so no root and no unknown.
                    StatementKind::PtrStore(_, _) => {}
                    StatementKind::Assign(dst, rval) => match rval {
                        Rvalue::Aggregate(kind, ops) if *kind != AggregateKind::FatPtr => {
                            for op in ops {
                                escape(op, Some(*dst), &mut info);
                            }
                        }
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(callee)),
                            args,
                        } => {
                            for (pos, op) in args.iter().enumerate() {
                                let callee_escape = self
                                    .param_escapes
                                    .get(callee)
                                    .is_some_and(|v| v.get(pos) == Some(&true));
                                if super::ownership::summaries::runtime_escape(callee, pos)
                                    || callee_escape
                                {
                                    escape(op, None, &mut info);
                                }
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }

                // Borrow- and transfer-producing definitions.
                let StatementKind::Assign(dst, rval) = &stmt.kind else {
                    continue;
                };
                if dst.0 == 0 || !tracked.get(dst.0).copied().unwrap_or(false) {
                    continue;
                }
                match rval {
                    Rvalue::Use(Operand::Copy(src))
                        if tracked.get(src.0).copied().unwrap_or(false) && src != dst =>
                    {
                        info[dst.0].suspect = true;
                        info[dst.0].borrow_span.get_or_insert(stmt.span);
                        info[dst.0].roots.insert(*src);
                        // Both names see one object: an escape or drop that
                        // frees it through either must taint the other too.
                        aliases.push((*dst, *src));
                    }
                    // A move hands the object to `dst`: anything still
                    // watching it through `src` dies at `dst`'s drop.
                    Rvalue::Use(Operand::Move(src))
                        if tracked.get(src.0).copied().unwrap_or(false) && src != dst =>
                    {
                        info[src.0].roots.insert(*dst);
                        aliases.push((*dst, *src));
                    }
                    Rvalue::GetIndex(base, _, _)
                    | Rvalue::GetAttr(base, _)
                    | Rvalue::FatPtrData(base)
                    | Rvalue::VTableLoad { vtable: base, .. } => {
                        info[dst.0].suspect = true;
                        info[dst.0].borrow_span.get_or_insert(stmt.span);
                        info[dst.0].elem_def = true;
                        if let Operand::Copy(b) | Operand::Move(b) = base {
                            info[dst.0].roots.insert(*b);
                        }
                    }
                    // No alias marks exist in the program (all escapes are
                    // deep-copied), so element borrows are never stale at
                    // birth and need no special treatment.
                    Rvalue::PtrLoad(_) => {}
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(name)),
                        ..
                    } if self.borrowed_returns.contains(name) => {
                        info[dst.0].suspect = true;
                        info[dst.0].borrow_span.get_or_insert(stmt.span);
                        info[dst.0].unknown = true;
                    }
                    _ => {}
                }
            }
        }
        (info, aliases)
    }

    /// Forward must-analysis; returns the check sites (before a statement,
    /// `usize::MAX` for the terminator) and the defs that validate at birth.
    fn solve(
        &self,
        func: &MirFunction,
        info: &[SuspectInfo],
        suspects: &[Local],
        index_of: &HashMap<Local, usize>,
    ) -> (Vec<Site>, Vec<Site>) {
        let k = suspects.len();
        let nb = func.basic_blocks.len();
        let preds = predecessors(func);

        // Proven-sets; top = all proven.
        let mut out: Vec<Vec<bool>> = vec![vec![true; k]; nb];
        let mut changed = true;
        while changed {
            changed = false;
            for bb in 0..nb {
                let mut state = if preds[bb].is_empty() {
                    vec![true; k]
                } else {
                    let mut s = out[preds[bb][0].0].clone();
                    for p in preds[bb].iter().skip(1) {
                        for (a, b) in s.iter_mut().zip(&out[p.0]) {
                            *a &= *b;
                        }
                    }
                    s
                };
                self.transfer_block(func, bb, info, suspects, index_of, &mut state, None);
                if state != out[bb] {
                    out[bb] = state;
                    changed = true;
                }
            }
        }

        let mut sites = Vec::new();
        let mut def_checks = Vec::new();
        for bb in 0..nb {
            let mut state = if preds[bb].is_empty() {
                vec![true; k]
            } else {
                let mut s = out[preds[bb][0].0].clone();
                for p in preds[bb].iter().skip(1) {
                    for (a, b) in s.iter_mut().zip(&out[p.0]) {
                        *a &= *b;
                    }
                }
                s
            };
            self.transfer_block(
                func,
                bb,
                info,
                suspects,
                index_of,
                &mut state,
                Some((&mut sites, &mut def_checks)),
            );
        }
        (sites, def_checks)
    }

    /// One pass over a block. With `emit` set, records check sites; the
    /// transfer itself models each use as proven afterwards, mirroring the
    /// check that insertion will place there.
    #[allow(clippy::too_many_arguments)]
    fn transfer_block(
        &self,
        func: &MirFunction,
        bb: usize,
        info: &[SuspectInfo],
        suspects: &[Local],
        index_of: &HashMap<Local, usize>,
        state: &mut [bool],
        mut emit: Option<Emit<'_>>,
    ) {
        let block = &func.basic_blocks[bb];
        for (idx, stmt) in block.statements.iter().enumerate() {
            // Uses first: the operand is read before the statement acts.
            for l in stmt_uses(stmt) {
                if let Some(&s) = index_of.get(&l)
                    && !state[s]
                {
                    if let Some((sites, _)) = emit.as_mut() {
                        sites.push((bb, idx, l));
                    }
                    state[s] = true;
                }
            }

            match &stmt.kind {
                StatementKind::Assign(dst, rval) => {
                    // The call runs before its result exists, so its kill
                    // must not erase the definition it produces.
                    if let Rvalue::Call { func: callee, .. } = rval
                        && !is_safe_call(callee)
                    {
                        for (s, &l) in suspects.iter().enumerate() {
                            if info[l.0].unknown {
                                state[s] = false;
                            }
                        }
                    }
                    if let Some(&s) = index_of.get(dst) {
                        // Every definition is fresh or verified; no alias
                        // marks exist, so element-birth checks are never
                        // needed.
                        state[s] = true;
                    }
                }
                StatementKind::Drop(dropped) => {
                    for (s, &l) in suspects.iter().enumerate() {
                        if info[l.0].unknown || info[l.0].roots.contains(dropped) {
                            state[s] = false;
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some(Terminator {
            kind: TerminatorKind::SwitchInt { discr, .. },
            ..
        }) = &block.terminator
            && let Operand::Copy(l) | Operand::Move(l) = discr
            && let Some(&s) = index_of.get(l)
            && !state[s]
        {
            if let Some((sites, _)) = emit.as_mut() {
                sites.push((bb, usize::MAX, *l));
            }
            state[s] = true;
        }
    }

    /// Materialises the analysis: a generation local per checked suspect,
    /// `GenOf` captures after each definition (at entry for params), and the
    /// checks themselves.
    fn insert(
        &self,
        func: &mut MirFunction,
        info: &[SuspectInfo],
        sites: Vec<Site>,
        def_checks: Vec<Site>,
    ) {
        let mut checked: Vec<Local> = sites
            .iter()
            .map(|&(_, _, l)| l)
            .chain(def_checks.iter().map(|&(_, _, l)| l))
            .collect();
        checked.sort_unstable_by_key(|l| l.0);
        checked.dedup();

        let mut gen_of: HashMap<Local, Local> = HashMap::default();
        for &l in &checked {
            let g = Local(func.locals.len());
            func.locals.push(LocalDecl {
                ty: Type::Int,
                name: None,
                span: Span::default(),
                is_mut: true,
                is_owning: true,
            });
            gen_of.insert(l, g);
        }

        let mut before: HashMap<(usize, usize), Vec<Statement>> = HashMap::default();
        let mut after: HashMap<(usize, usize), Vec<Statement>> = HashMap::default();
        let mut at_term: HashMap<usize, Vec<Statement>> = HashMap::default();

        let check_stmt = |l: Local, g: Local, span: Span| Statement {
            kind: StatementKind::GenCheck {
                value: l,
                generation: g,
            },
            span,
        };
        let capture_stmt = |l: Local, g: Local, span: Span| Statement {
            kind: StatementKind::Assign(g, Rvalue::GenOf(Operand::Copy(l))),
            span,
        };

        // Re-capture after every definition of a checked local.
        for (bb, block) in func.basic_blocks.iter().enumerate() {
            for (idx, stmt) in block.statements.iter().enumerate() {
                if let StatementKind::Assign(dst, _) = &stmt.kind
                    && let Some(&g) = gen_of.get(dst)
                {
                    after
                        .entry((bb, idx))
                        .or_default()
                        .push(capture_stmt(*dst, g, stmt.span));
                }
            }
        }

        for (bb, idx, l) in sites {
            let g = gen_of[&l];
            if idx == usize::MAX {
                let span = func.basic_blocks[bb]
                    .terminator
                    .as_ref()
                    .map(|t| t.span)
                    .unwrap_or_default();
                at_term.entry(bb).or_default().push(check_stmt(l, g, span));
            } else {
                let span = func.basic_blocks[bb].statements[idx].span;
                before
                    .entry((bb, idx))
                    .or_default()
                    .push(check_stmt(l, g, span));
            }
        }
        for (bb, idx, l) in def_checks {
            let span = func.basic_blocks[bb].statements[idx].span;
            after
                .entry((bb, idx))
                .or_default()
                .push(check_stmt(l, gen_of[&l], span));
        }

        for (bb, block) in func.basic_blocks.iter_mut().enumerate() {
            let old = std::mem::take(&mut block.statements);
            let mut rebuilt = Vec::with_capacity(old.len() + 8);
            if bb == 0 {
                // Zero captures so no path reads an undefined generation; a
                // null value skips its check anyway. Params capture at entry.
                for &l in &checked {
                    rebuilt.push(Statement {
                        kind: StatementKind::Assign(
                            gen_of[&l],
                            Rvalue::Use(Operand::Constant(Constant::Int(0))),
                        ),
                        span: Span::default(),
                    });
                }
                for &l in &checked {
                    if l.0 >= 1 && l.0 <= func.arg_count && info[l.0].suspect {
                        rebuilt.push(capture_stmt(l, gen_of[&l], Span::default()));
                    }
                }
            }
            for (idx, stmt) in old.into_iter().enumerate() {
                if let Some(list) = before.remove(&(bb, idx)) {
                    rebuilt.extend(list);
                }
                rebuilt.push(stmt);
                if let Some(list) = after.remove(&(bb, idx)) {
                    // Captures come before any birth check of the same def.
                    rebuilt.extend(list);
                }
            }
            if let Some(list) = at_term.remove(&bb) {
                rebuilt.extend(list);
            }
            block.statements = rebuilt;
        }
    }
}

/// An alias pair shares one object, so escape roots, untracked-owner taint,
/// and suspicion flow across the pair in both directions. Kept separate from
/// directed root edges: the pair itself is not a kill, only what it carries.
fn merge_aliases(info: &mut [SuspectInfo], aliases: &[(Local, Local)]) {
    loop {
        let mut changed = false;
        for &(a, b) in aliases {
            for (x, y) in [(a, b), (b, a)] {
                if info[y.0].suspect && !info[x.0].suspect {
                    info[x.0].suspect = true;
                    changed = true;
                }
                if info[x.0].borrow_span.is_none() && info[y.0].borrow_span.is_some() {
                    info[x.0].borrow_span = info[y.0].borrow_span;
                    changed = true;
                }
                if info[y.0].unknown && !info[x.0].unknown {
                    info[x.0].unknown = true;
                    changed = true;
                }
                let extra: Vec<Local> = info[y.0]
                    .roots
                    .iter()
                    .copied()
                    .filter(|r| *r != x && !info[x.0].roots.contains(r))
                    .collect();
                if !extra.is_empty() {
                    info[x.0].roots.extend(extra);
                    changed = true;
                }
            }
        }
        if !changed {
            return;
        }
    }
}

/// Merges every root's own roots in, so a drop anywhere along a borrow chain
/// kills the proof; a chain through an untracked owner taints the whole set.
fn close_roots(info: &mut [SuspectInfo]) {
    loop {
        let mut changed = false;
        for i in 0..info.len() {
            let roots: Vec<Local> = info[i].roots.iter().copied().collect();
            for r in roots {
                if r.0 == i {
                    continue;
                }
                if info[r.0].unknown && !info[i].unknown {
                    info[i].unknown = true;
                    changed = true;
                }
                let extra: Vec<Local> = info[r.0]
                    .roots
                    .iter()
                    .copied()
                    .filter(|x| x.0 != i && !info[i].roots.contains(x))
                    .collect();
                if !extra.is_empty() {
                    info[i].roots.extend(extra);
                    changed = true;
                }
            }
        }
        if !changed {
            return;
        }
    }
}

/// Locals a statement reads as values. Drops and storage markers are not
/// uses: freeing a dead value is absorbed by the generation word itself.
fn stmt_uses(stmt: &Statement) -> Vec<Local> {
    let mut out = Vec::new();
    let op = |o: &Operand, out: &mut Vec<Local>| {
        if let Operand::Copy(l) | Operand::Move(l) = o {
            out.push(*l);
        }
    };
    match &stmt.kind {
        StatementKind::Assign(_, rval) => match rval {
            Rvalue::Use(o)
            | Rvalue::UnaryOp(_, o)
            | Rvalue::Cast(o, _)
            | Rvalue::GetAttr(o, _)
            | Rvalue::GetTag(o)
            | Rvalue::GetTypeId(o)
            | Rvalue::VectorSplat(o, _)
            | Rvalue::VectorReduce(_, o, _)
            | Rvalue::PtrLoad(o)
            | Rvalue::GenOf(o)
            | Rvalue::FatPtrData(o)
            | Rvalue::VTableLoad { vtable: o, .. } => op(o, &mut out),
            Rvalue::BinaryOp(_, a, b) | Rvalue::GetIndex(a, b, _) | Rvalue::VectorLoad(a, b, _) => {
                op(a, &mut out);
                op(b, &mut out);
            }
            Rvalue::VectorFMA(a, b, c) => {
                op(a, &mut out);
                op(b, &mut out);
                op(c, &mut out);
            }
            Rvalue::Call { func, args } => {
                op(func, &mut out);
                for a in args {
                    op(a, &mut out);
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for o in ops {
                    op(o, &mut out);
                }
            }
            Rvalue::Ref(l) | Rvalue::MutRef(l) => out.push(*l),
        },
        StatementKind::SetAttr(a, _, b) | StatementKind::PtrStore(a, b) => {
            op(a, &mut out);
            op(b, &mut out);
        }
        StatementKind::SetIndex(a, b, c, _) | StatementKind::VectorStore(a, b, c) => {
            op(a, &mut out);
            op(b, &mut out);
            op(c, &mut out);
        }
        StatementKind::Drop(_)
        | StatementKind::StorageLive(_)
        | StatementKind::StorageDead(_)
        | StatementKind::GenCheck { .. } => {}
    }
    out
}

fn predecessors(func: &MirFunction) -> Vec<Vec<BasicBlockId>> {
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
