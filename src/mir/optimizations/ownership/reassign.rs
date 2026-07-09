use super::{AssignRec, BorrowEdges, LocalClass, block_preds};
use crate::mir::*;
use crate::semantic::types::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use std::cell::RefCell;

thread_local! {
    pub(crate) static REASSIGN_LIVE_BORROWS: RefCell<HashMap<String, HashMap<Local, bool>>> = RefCell::new(HashMap::default());
}

pub(super) fn reassign_free_locals(
    func: &MirFunction,
    classes: &[LocalClass],
    heap: &[bool],
    records: &[AssignRec],
    borrow_edges: &BorrowEdges,
) -> HashSet<Local> {
    let mut view_roots: HashSet<Local> = HashSet::default();
    for srcs in borrow_edges.values() {
        view_roots.extend(srcs.iter().map(|(l, _)| *l));
    }
    let mut assigns = vec![0u32; classes.len()];
    for rec in records {
        assigns[rec.dst.0] += 1;
    }
    let in_loop_body: HashSet<Local> = crate::mir::loop_utils::find_loops(func)
        .iter()
        .flat_map(|lp| &lp.body)
        .flat_map(|&bb| &func.basic_blocks[bb.0].statements)
        .filter_map(|stmt| match &stmt.kind {
            StatementKind::Assign(dst, _) => Some(*dst),
            _ => None,
        })
        .collect();
    let mut result = HashSet::default();
    let mut borrow_map = HashMap::default();
    for i in 0..classes.len() {
        if i != 0
            && heap[i]
            && classes[i] == LocalClass::Owner
            && (assigns[i] >= 2 || in_loop_body.contains(&Local(i)))
        {
            let local = Local(i);
            let has_borrow = view_roots.contains(&local);
            // Only locals WITHOUT live borrows get an explicit MIR drop inserted
            // by insert_reassign_drops. Borrow-aliased locals are recorded in
            // REASSIGN_LIVE_BORROWS so the codegen can still attempt in-place
            // reuse (with a generation bump), but the MIR pass must not free
            // them since a live view may still alias the slot.
            borrow_map.insert(local, has_borrow);
            if !has_borrow {
                result.insert(local);
            }
        }
    }
    REASSIGN_LIVE_BORROWS.with(|map| {
        map.borrow_mut().insert(func.name.clone(), borrow_map);
    });
    result
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

/// Whether a reassign-tracked local may already hold an owned value at each
/// block's entry: forward may-dataflow (OR at joins) over the whole CFG. A
/// per-block "have we assigned this before" set alone misses a loop-body
/// reassignment, since the back edge revisits the same block without that
/// block-local set ever recording the prior iteration's def.
fn reassign_entry_state(func: &MirFunction, reassign: &HashSet<Local>) -> Vec<HashSet<Local>> {
    let nb = func.basic_blocks.len();
    let preds = block_preds(func);
    // `out` folds in each block's own defs (needed to propagate through a
    // loop's back edge); `entry` is predecessors-only and is what a caller
    // must seed with -- using `out[bb]` for block `bb` itself would count a
    // reassignment's own definition as already having happened before it.
    let mut out: Vec<HashSet<Local>> = vec![HashSet::default(); nb];
    let mut entry: Vec<HashSet<Local>> = vec![HashSet::default(); nb];
    let mut changed = true;
    while changed {
        changed = false;
        for bb in 0..nb {
            let mut state: HashSet<Local> = HashSet::default();
            for &p in &preds[bb] {
                state.extend(out[p.0].iter().copied());
            }
            if state != entry[bb] {
                entry[bb] = state.clone();
                changed = true;
            }
            for stmt in &func.basic_blocks[bb].statements {
                if let StatementKind::Assign(dst, _) = &stmt.kind
                    && reassign.contains(dst)
                {
                    state.insert(*dst);
                }
            }
            if state != out[bb] {
                out[bb] = state;
                changed = true;
            }
        }
    }
    entry
}

/// Frees reassigned owner before each reassignment. The unconditional drop
/// is later gated on the ownership flag so the first definition frees nothing.
pub(super) fn insert_reassign_drops(func: &mut MirFunction, reassign: &HashSet<Local>) -> bool {
    let mut any = false;
    let entry_state = reassign_entry_state(func, reassign);
    for (bb_idx, bb) in func.basic_blocks.iter_mut().enumerate() {
        let old = std::mem::take(&mut bb.statements);
        // For each local, the set of other locals that may alias its slot.
        // `str_concat_inplace` is the only runtime op that reuses an
        // operand's storage instead of allocating fresh; a generic
        // BinaryOp/Call reading `dst` (PyObject add, list index, ...) always
        // produces an independent value, so tracking those here would wrongly
        // treat a real reassignment as an alias and skip freeing the old one.
        let mut aliases_of: HashMap<Local, HashSet<Local>> = HashMap::default();
        for stmt in &old {
            if let StatementKind::Assign(dst, Rvalue::BinaryOp(crate::parser::BinOp::Add, a, b)) =
                &stmt.kind
                && func.locals[dst.0].ty == Type::Str
            {
                let each = |aliases: &mut HashMap<Local, HashSet<Local>>, op: &Operand| {
                    if let Operand::Copy(src) | Operand::Move(src) = op {
                        aliases.entry(*src).or_default().insert(*dst);
                    }
                };
                each(&mut aliases_of, a);
                each(&mut aliases_of, b);
            }
        }
        let mut rebuilt = Vec::with_capacity(old.len() + 2);
        // Seeded from the block's entry state, so a loop-body reassignment
        // sees "already initialized" on its very first (and only) static
        // occurrence, carried in from the def before the loop or a prior trip
        // around the back edge.
        let mut assigned_in_block: HashSet<Local> = entry_state[bb_idx].clone();
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
