//! Resolves an indirect closure call to a direct one when the thunk pointer
//! is a compile-time constant traceable to one build site. Unlike trait
//! object devirtualization, this never needs escape analysis on the record:
//! captures are still read from it at runtime by the callee, so the record
//! stays exactly as live as before. The only thing that has to be constant
//! is which function the call lands on, and `closures.rs::build_closure_value`
//! writes that as a literal `Constant::Function` exactly once per record.

use super::Transform;
use crate::mir::*;
use crate::semantic::types::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub struct DevirtualizeClosures;

fn operand_local(op: &Operand) -> Option<Local> {
    match op {
        Operand::Copy(l) | Operand::Move(l) => Some(*l),
        Operand::Constant(_) => None,
    }
}

fn statement_reads(kind: &StatementKind, out: &mut Vec<Local>) {
    match kind {
        StatementKind::Assign(_, rval) => super::rvalue_operand_locals(rval, out),
        StatementKind::SetAttr(o, _, v) | StatementKind::PtrStore(o, v) => {
            out.extend(operand_local(o));
            out.extend(operand_local(v));
        }
        StatementKind::SetIndex(o, i, v, _) => {
            out.extend(operand_local(o));
            out.extend(operand_local(i));
            out.extend(operand_local(v));
        }
        StatementKind::VectorStore(o, i, v) => {
            out.extend(operand_local(o));
            out.extend(operand_local(i));
            out.extend(operand_local(v));
        }
        StatementKind::Drop(l) => out.push(*l),
        StatementKind::GenCheck { value, generation } => {
            out.push(*value);
            out.push(*generation);
        }
        StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {}
    }
}

/// A local's def counts toward "single-def" only if some later read (in the
/// same block, in program order) could actually observe it. Inlining splices
/// a callee's own prologue in whole, including its `_return` local's zero
/// init immediately overwritten by the real value before any use -- a
/// legitimate second static `Assign` that reaching-definitions would never
/// let flow anywhere. Filtering those out here keeps single-def strict
/// (still false on any def that might reach a use) while not choking on an
/// artifact every inlined-call-with-a-return-value produces.
fn effective_def_counts(func: &MirFunction) -> HashMap<Local, usize> {
    let mut dead: HashSet<(usize, usize)> = HashSet::default();
    for (bb_idx, bb) in func.basic_blocks.iter().enumerate() {
        let mut pending: HashMap<Local, usize> = HashMap::default();
        for (stmt_idx, stmt) in bb.statements.iter().enumerate() {
            let mut reads = Vec::new();
            statement_reads(&stmt.kind, &mut reads);
            for r in reads {
                pending.remove(&r);
            }
            if let StatementKind::Assign(dest, _) = &stmt.kind {
                if let Some(&prev_idx) = pending.get(dest) {
                    dead.insert((bb_idx, prev_idx));
                }
                pending.insert(*dest, stmt_idx);
            }
        }
    }

    let mut counts: HashMap<Local, usize> = HashMap::default();
    for (bb_idx, bb) in func.basic_blocks.iter().enumerate() {
        for (stmt_idx, stmt) in bb.statements.iter().enumerate() {
            if let StatementKind::Assign(dest, _) = &stmt.kind
                && !dead.contains(&(bb_idx, stmt_idx))
            {
                *counts.entry(*dest).or_insert(0) += 1;
            }
        }
    }
    counts
}

impl Transform for DevirtualizeClosures {
    fn run(&self, func: &mut MirFunction) -> bool {
        let def_counts = effective_def_counts(func);

        // Records built by `__olive_struct_alloc`, single-def past the fn args.
        let mut alloc_records: HashMap<Local, ()> = HashMap::default();
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, Rvalue::Call { func: f, .. }) = &stmt.kind
                    && matches!(f, Operand::Constant(Constant::Function(n)) if n == "__olive_struct_alloc")
                    && def_counts.get(dest) == Some(&1)
                    && dest.0 > func.arg_count
                {
                    alloc_records.insert(*dest, ());
                }
            }
        }
        if alloc_records.is_empty() {
            return false;
        }

        // A record's `__thunk` field set exactly once, to a function constant.
        let mut thunk_of: HashMap<Local, String> = HashMap::default();
        let mut thunk_set_count: HashMap<Local, usize> = HashMap::default();
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::SetAttr(target, attr, val) = &stmt.kind
                    && attr == "__thunk"
                    && let Some(rec) = operand_local(target)
                {
                    *thunk_set_count.entry(rec).or_insert(0) += 1;
                    match val {
                        Operand::Constant(Constant::Function(name)) => {
                            thunk_of.insert(rec, name.clone());
                        }
                        _ => {
                            thunk_of.remove(&rec);
                        }
                    }
                }
            }
        }
        thunk_of.retain(|rec, _| {
            alloc_records.contains_key(rec) && thunk_set_count.get(rec) == Some(&1)
        });
        if thunk_of.is_empty() {
            return false;
        }

        // `fn_view = Cast(record, Fn)` locals: single-def, root of a callable.
        let mut roots: HashMap<Local, (String, Local)> = HashMap::default();
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, Rvalue::Cast(op, ty)) = &stmt.kind
                    && matches!(ty, Type::Fn(..))
                    && def_counts.get(dest) == Some(&1)
                    && dest.0 > func.arg_count
                    && let Some(rec) = operand_local(op)
                    && let Some(thunk) = thunk_of.get(&rec)
                {
                    roots.insert(*dest, (thunk.clone(), rec));
                }
            }
        }
        if roots.is_empty() {
            return false;
        }

        // Alias chain: single-def locals holding a plain copy of a root.
        // A local with more than one *live* def (reassigned to something
        // else later, e.g. a different closure or a loop-carried rebind)
        // never enters this map, so a stale binding can never devirtualize.
        let mut group: HashMap<Local, Local> = HashMap::default();
        for &r in roots.keys() {
            group.insert(r, r);
        }
        let mut changed_alias = true;
        while changed_alias {
            changed_alias = false;
            for bb in &func.basic_blocks {
                for stmt in &bb.statements {
                    if let StatementKind::Assign(dest, Rvalue::Use(op)) = &stmt.kind
                        && let Some(src) = operand_local(op)
                        && let Some(&root) = group.get(&src)
                        && def_counts.get(dest) == Some(&1)
                        && dest.0 > func.arg_count
                        && !group.contains_key(dest)
                    {
                        group.insert(*dest, root);
                        changed_alias = true;
                    }
                }
            }
        }

        // Calls through a resolved local become direct, with the record
        // appended as the thunk's trailing hidden env argument, matching
        // the ABI `build_closure_thunk` and `translate_call.rs` already use.
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, Rvalue::Call { func: f, args }) = &mut stmt.kind
                    && let Some(l) = operand_local(f)
                    && let Some(&root) = group.get(&l)
                {
                    let (thunk, record) = &roots[&root];
                    *f = Operand::Constant(Constant::Function(thunk.clone()));
                    args.push(Operand::Copy(*record));
                    changed = true;
                }
            }
        }

        changed
    }
}
