//! Whole-program summaries the per-function ownership pass consumes: which
//! params escape into longer-lived storage, and which functions may return a
//! borrow instead of a fresh value.
//!
//! Limitation: `collect_assigns` and `params_escaping` scan only
//! `Constant::Function` callees. Indirect calls through function-typed values
//! are invisible to the escape analysis, so escapes through those edges are
//! undetected here and left to the gencheck runtime backstop (E0707).

use super::block_preds;
use crate::mir::*;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Runtime calls that take ownership of an argument by storing it in the
/// receiver, keyed to the argument's position in the lowered call.
const RUNTIME_ESCAPES: &[(&str, usize)] = &[
    ("__olive_list_append", 1),
    ("__olive_list_insert", 2),
    ("__olive_obj_set", 2),
    ("__olive_set_add", 1),
    ("__olive_chan_send", 1),
    ("__olive_mutex_new", 0),
    ("__olive_mutex_unlock", 1),
    ("__olive_pool_run", 1),
    ("__olive_pool_run_sync", 1),
];

pub(crate) fn runtime_escape(name: &str, pos: usize) -> bool {
    RUNTIME_ESCAPES.iter().any(|&(n, p)| n == name && p == pos)
}

/// Fixpoint over the whole program: param `i` of a function escapes when the
/// body stores it (or an alias of it) beyond the frame -- into a field, an
/// element, a global, an aggregate, or an escaping position of another call.
/// A caller must then stop owning what it passed there.
pub fn compute_param_escapes(functions: &[MirFunction]) -> HashMap<String, Vec<bool>> {
    let mut escapes: HashMap<String, Vec<bool>> = functions
        .iter()
        .map(|f| (f.name.clone(), vec![false; f.arg_count]))
        .collect();
    loop {
        let mut changed = false;
        for func in functions {
            let updated = params_escaping(func, &escapes);
            let entry = escapes.get_mut(&func.name).unwrap();
            for (i, e) in updated.iter().enumerate() {
                if *e && !entry[i] {
                    entry[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return escapes;
        }
    }
}

fn params_escaping(func: &MirFunction, escapes: &HashMap<String, Vec<bool>>) -> Vec<bool> {
    let n = func.locals.len();
    // taint[l] = params local l may alias, as a bitmask. Params past 63 share
    // the top bit, so they conservatively co-escape rather than being missed.
    let mut taint = vec![0u64; n];
    for (i, t) in taint
        .iter_mut()
        .enumerate()
        .take(func.arg_count + 1)
        .skip(1)
    {
        *t = 1 << (i - 1).min(63);
    }
    loop {
        let mut changed = false;
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                let StatementKind::Assign(dst, rval) = &stmt.kind else {
                    continue;
                };
                if dst.0 >= n {
                    continue;
                }
                if let Rvalue::Use(Operand::Copy(src)) | Rvalue::Use(Operand::Move(src)) = rval
                    && src.0 < n
                {
                    let merged = taint[dst.0] | taint[src.0];
                    if merged != taint[dst.0] {
                        taint[dst.0] = merged;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut mask = 0u64;
    let mark = |op: &Operand, taint: &[u64], mask: &mut u64| {
        if let Operand::Copy(l) | Operand::Move(l) = op
            && l.0 < taint.len()
        {
            *mask |= taint[l.0];
        }
    };
    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            match &stmt.kind {
                StatementKind::SetAttr(_, _, val) => mark(val, &taint, &mut mask),
                StatementKind::SetIndex(_, _, val, _) => mark(val, &taint, &mut mask),
                StatementKind::PtrStore(_, val) => mark(val, &taint, &mut mask),
                StatementKind::Assign(_, rval) => match rval {
                    Rvalue::Aggregate(_, ops) => {
                        for op in ops {
                            mark(op, &taint, &mut mask);
                        }
                    }
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(callee)),
                        args,
                    } => {
                        for (pos, op) in args.iter().enumerate() {
                            let callee_escape = escapes
                                .get(callee)
                                .is_some_and(|v| v.get(pos) == Some(&true));
                            if runtime_escape(callee, pos) || callee_escape {
                                mark(op, &taint, &mut mask);
                            }
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
    (0..func.arg_count)
        .map(|i| mask & (1 << i.min(63)) != 0)
        .collect()
}

/// Fixpoint over the whole program: a function returns a borrow if `_return`
/// is ever reachable from a param, a field/element read, or a call to a
/// function already in the set.
pub fn compute_borrowed_returns(functions: &[MirFunction]) -> HashSet<String> {
    let mut borrowed: HashSet<String> = HashSet::default();
    loop {
        let mut changed = false;
        for func in functions {
            if borrowed.contains(&func.name) {
                continue;
            }
            if returns_borrow(func, &borrowed) {
                borrowed.insert(func.name.clone());
                changed = true;
            }
        }
        if !changed {
            return borrowed;
        }
    }
}

fn returns_borrow(func: &MirFunction, borrowed: &HashSet<String>) -> bool {
    if !func.locals.first().is_some_and(|d| d.ty.is_move_type()) {
        return false;
    }
    let n = func.locals.len();
    let nb = func.basic_blocks.len();
    let preds = block_preds(func);
    // Per-block entry taint state. Seeded with params as tainted everywhere.
    let param_bound = func.arg_count.min(n - 1);
    let mut entry: Vec<Vec<bool>> = vec![vec![false; n]; nb];
    for e in entry.iter_mut() {
        for s in e.iter_mut().take(param_bound + 1).skip(1) {
            *s = true;
        }
    }
    let mut out: Vec<Vec<bool>> = entry.clone();
    // Forward may-analysis: union at joins, ALWAYS compute taint for dst
    // (not just set to true) so a fresh assignment to _return clears the
    // taint even if a prior assignment in another block set it.
    loop {
        let mut changed = false;
        for bb in 0..nb {
            let mut state = vec![false; n];
            for &p in &preds[bb] {
                let mut p_out = out[p.0].clone();
                for stmt in &func.basic_blocks[p.0].statements {
                    if let StatementKind::Assign(dst, rval) = &stmt.kind
                        && dst.0 < n
                    {
                        p_out[dst.0] = match rval {
                            Rvalue::Use(Operand::Copy(src)) | Rvalue::Use(Operand::Move(src)) => {
                                src.0 < n && p_out[src.0]
                            }
                            Rvalue::GetIndex(_, _, _)
                            | Rvalue::GetAttr(_, _)
                            | Rvalue::PtrLoad(_)
                            | Rvalue::FatPtrData(_)
                            | Rvalue::VTableLoad { .. } => true,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(name)),
                                ..
                            } => borrowed.contains(name.as_str()),
                            _ => false,
                        };
                    }
                }
                for (i, &p_val) in p_out.iter().enumerate() {
                    if p_val {
                        state[i] = true;
                    }
                }
            }
            if bb == 0 {
                for s in state.iter_mut().take(param_bound + 1).skip(1) {
                    *s = true;
                }
            }
            if state != entry[bb] {
                entry[bb] = state.clone();
                changed = true;
            }
            for stmt in &func.basic_blocks[bb].statements {
                if let StatementKind::Assign(dst, rval) = &stmt.kind
                    && dst.0 < n
                {
                    state[dst.0] = match rval {
                        Rvalue::Use(Operand::Copy(src)) | Rvalue::Use(Operand::Move(src)) => {
                            src.0 < n && state[src.0]
                        }
                        Rvalue::GetIndex(_, _, _)
                        | Rvalue::GetAttr(_, _)
                        | Rvalue::PtrLoad(_)
                        | Rvalue::FatPtrData(_)
                        | Rvalue::VTableLoad { .. } => true,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(name)),
                            ..
                        } => borrowed.contains(name.as_str()),
                        _ => false,
                    };
                }
            }
            if state != out[bb] {
                out[bb] = state;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // Only mark function returning borrow if a reachable RETURN block has
    // tainted _return. Intermediate _return taint in non-return blocks
    // (from a prior assignment later overwritten) does not count.
    for (bb, block) in func.basic_blocks.iter().enumerate() {
        if !matches!(&block.terminator, Some(t) if matches!(&t.kind, TerminatorKind::Return)) {
            continue;
        }
        if out[bb][0] {
            return true;
        }
    }
    false
}
