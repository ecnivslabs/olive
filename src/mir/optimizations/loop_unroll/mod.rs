mod rewrite;

use crate::mir::loop_utils;
use crate::mir::optimizations::Transform;
use crate::mir::*;
use crate::parser::BinOp;
use crate::semantic::types::Type as OliveType;
use crate::span::Span;
use rewrite::{is_storage_of, rvalue_refs_local, step_from_add, stmt_reads_local, subst_local};
use rustc_hash::FxHashSet;

/// Full unroll for constant trip counts; 4x partial with epilogue otherwise. Straight-line only.
pub struct LoopUnroll;

const FULL_TRIP_LIMIT: i64 = 32;
const PARTIAL_FACTOR: i64 = 4;
const MAX_EXPANSION: usize = 1024;

impl Transform for LoopUnroll {
    fn run(&self, func: &mut MirFunction) -> bool {
        let loops = loop_utils::find_loops(func);
        let mut touched: FxHashSet<BasicBlockId> = FxHashSet::default();
        let mut changed = false;
        for lp in loops {
            // Skip loops a prior transform consumed (nested, overlapping, clones).
            if lp.body.iter().any(|b| touched.contains(b)) {
                continue;
            }
            if let Some(plan) = analyze(func, &lp)
                && transform(func, &lp, &plan)
            {
                touched.extend(lp.body.iter().copied());
                changed = true;
            }
        }
        changed
    }
}

struct UnrollPlan {
    induction: Local,
    step: i64,
    limit: Operand,
    cond_local: Local,
    body_entry: BasicBlockId,
    exit: BasicBlockId,
    /// Iteration work, induction update removed.
    work: Vec<Statement>,
    const_trip: Option<i64>,
}

fn analyze(func: &MirFunction, lp: &loop_utils::Loop) -> Option<UnrollPlan> {
    if lp.latches.len() != 1 || lp.exits.len() != 1 {
        return None;
    }
    let header_id = lp.header;
    let latch_id = lp.latches[0];

    let (cond_local, body_entry, exit) = header_switch(func, lp)?;
    if exit != lp.exits[0] {
        return None;
    }
    let (induction, cmp, limit) = header_test(func, header_id, cond_local)?;

    // Header must hold only the guard; other work there is not replicated.
    for stmt in &func.basic_blocks[header_id.0].statements {
        match &stmt.kind {
            StatementKind::Assign(l, _) if *l == cond_local => {}
            StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {}
            _ => return None,
        }
    }

    let chain = linear_chain(func, lp, body_entry, latch_id)?;
    let mut work = Vec::new();
    for &bb_id in &chain {
        work.extend(func.basic_blocks[bb_id.0].statements.iter().cloned());
    }

    let step = extract_step(&mut work, induction)?;
    if step <= 0 {
        return None;
    }
    if work.is_empty() {
        return None;
    }
    if !body_is_unrollable(&work, induction) {
        return None;
    }

    let const_trip = const_trip_count(func, lp, induction, &cmp, &limit, step);

    Some(UnrollPlan {
        induction,
        step,
        limit,
        cond_local,
        body_entry,
        exit,
        work,
        const_trip,
    })
}

/// Header `SwitchInt` split into `(cond, in_loop_target, exit_target)`.
fn header_switch(
    func: &MirFunction,
    lp: &loop_utils::Loop,
) -> Option<(Local, BasicBlockId, BasicBlockId)> {
    let term = func.basic_blocks[lp.header.0].terminator.as_ref()?;
    let TerminatorKind::SwitchInt {
        discr,
        targets,
        otherwise,
    } = &term.kind
    else {
        return None;
    };
    let cond = match discr {
        Operand::Copy(l) | Operand::Move(l) => *l,
        Operand::Constant(_) => return None,
    };

    let mut in_loop = None;
    let mut out = None;
    for (_, t) in targets {
        if lp.body.contains(t) {
            if in_loop.is_some_and(|b| b != *t) {
                return None;
            }
            in_loop = Some(*t);
        } else {
            if out.is_some_and(|b| b != *t) {
                return None;
            }
            out = Some(*t);
        }
    }
    if lp.body.contains(otherwise) {
        if in_loop.is_some_and(|b| b != *otherwise) {
            return None;
        }
        in_loop = Some(*otherwise);
    } else {
        if out.is_some_and(|b| b != *otherwise) {
            return None;
        }
        out = Some(*otherwise);
    }

    Some((cond, in_loop?, out?))
}

/// Finds the `cond = i <cmp> limit` definition that feeds the header switch.
fn header_test(
    func: &MirFunction,
    header: BasicBlockId,
    cond_local: Local,
) -> Option<(Local, BinOp, Operand)> {
    for stmt in &func.basic_blocks[header.0].statements {
        if let StatementKind::Assign(l, Rvalue::BinaryOp(op, Operand::Copy(idx), rhs)) = &stmt.kind
            && *l == cond_local
            && matches!(op, BinOp::Lt | BinOp::LtEq)
        {
            return Some((*idx, op.clone(), rhs.clone()));
        }
    }
    None
}

/// Single-successor chain `entry` to `latch` covering the body sans header.
fn linear_chain(
    func: &MirFunction,
    lp: &loop_utils::Loop,
    entry: BasicBlockId,
    latch: BasicBlockId,
) -> Option<Vec<BasicBlockId>> {
    let mut chain = Vec::new();
    let mut seen = FxHashSet::default();
    let mut cur = entry;
    loop {
        if cur == lp.header || !lp.body.contains(&cur) || !seen.insert(cur) {
            return None;
        }
        chain.push(cur);
        let term = func.basic_blocks[cur.0].terminator.as_ref()?;
        let TerminatorKind::Goto { target } = &term.kind else {
            return None;
        };
        if cur == latch {
            if *target != lp.header {
                return None;
            }
            break;
        }
        cur = *target;
    }
    if chain.len() != lp.body.len() - 1 {
        return None;
    }
    Some(chain)
}

/// Strips the induction update; handles both direct i=i+c and tmp-indirection forms.
fn extract_step(work: &mut Vec<Statement>, induction: Local) -> Option<i64> {
    let mut writes = Vec::new();
    for (idx, stmt) in work.iter().enumerate() {
        if let StatementKind::Assign(dest, _) = &stmt.kind
            && *dest == induction
        {
            writes.push(idx);
        }
    }
    if writes.len() != 1 {
        return None;
    }
    let update = writes[0];

    // No read may observe the induction after its update.
    if work[update + 1..]
        .iter()
        .any(|s| stmt_reads_local(s, induction))
    {
        return None;
    }

    let mut remove = vec![update];
    let step = match &work[update].kind {
        StatementKind::Assign(_, Rvalue::BinaryOp(BinOp::Add, a, b)) => {
            step_from_add(a, b, induction)?
        }
        StatementKind::Assign(_, Rvalue::Use(Operand::Copy(tmp))) => {
            let tmp = *tmp;
            // tmp must be private to this update.
            if work
                .iter()
                .enumerate()
                .any(|(i, s)| i != update && stmt_reads_local(s, tmp))
            {
                return None;
            }
            let mut def = None;
            for (i, s) in work.iter().enumerate() {
                if let StatementKind::Assign(d, _) = &s.kind
                    && *d == tmp
                {
                    if def.is_some() {
                        return None;
                    }
                    def = Some(i);
                }
            }
            let def = def?;
            let step = match &work[def].kind {
                StatementKind::Assign(_, Rvalue::BinaryOp(BinOp::Add, a, b)) => {
                    step_from_add(a, b, induction)?
                }
                _ => return None,
            };
            remove.push(def);
            step
        }
        _ => return None,
    };

    remove.sort_unstable();
    for &idx in remove.iter().rev() {
        work.remove(idx);
    }
    Some(step)
}

/// Rejects bodies that write, alias, drop, or vectorize the induction.
fn body_is_unrollable(work: &[Statement], induction: Local) -> bool {
    for stmt in work {
        match &stmt.kind {
            StatementKind::Assign(dest, rval) => {
                if *dest == induction {
                    return false;
                }
                if rvalue_refs_local(rval, induction) {
                    return false;
                }
                if matches!(
                    rval,
                    Rvalue::VectorLoad(..)
                        | Rvalue::VectorSplat(..)
                        | Rvalue::VectorFMA(..)
                        | Rvalue::VectorReduce(..)
                ) {
                    return false;
                }
            }
            // Induction storage markers are dropped at copy time; a real Drop
            // means it owns a value that cannot be duplicated.
            StatementKind::Drop(l) if *l == induction => return false,
            StatementKind::StorageLive(_)
            | StatementKind::StorageDead(_)
            | StatementKind::Drop(_) => {}
            StatementKind::VectorStore(..) => return false,
            _ => {}
        }
    }
    true
}

/// Computes the trip count when start, limit, and step are all constant.
fn const_trip_count(
    func: &MirFunction,
    lp: &loop_utils::Loop,
    induction: Local,
    cmp: &BinOp,
    limit: &Operand,
    step: i64,
) -> Option<i64> {
    let start = const_init(func, lp, induction)?;
    let end = const_operand(func, limit)?;
    let span = match cmp {
        BinOp::Lt => end - start,
        BinOp::LtEq => end - start + 1,
        _ => return None,
    };
    if span <= 0 {
        return Some(0);
    }
    Some((span + step - 1) / step)
}

/// Constant the induction holds on loop entry.
fn const_init(func: &MirFunction, lp: &loop_utils::Loop, induction: Local) -> Option<i64> {
    let mut preds = Vec::new();
    for (i, bb) in func.basic_blocks.iter().enumerate() {
        let id = BasicBlockId(i);
        if lp.body.contains(&id) {
            continue;
        }
        if let Some(term) = &bb.terminator
            && successors(&term.kind).contains(&lp.header)
        {
            preds.push(id);
        }
    }
    if preds.len() != 1 {
        return None;
    }
    let mut value = None;
    for stmt in &func.basic_blocks[preds[0].0].statements {
        if let StatementKind::Assign(dest, Rvalue::Use(Operand::Constant(Constant::Int(v)))) =
            &stmt.kind
            && *dest == induction
        {
            value = Some(*v);
        }
    }
    value
}

/// Operand as a constant int, tracing single-assignment locals.
fn const_operand(func: &MirFunction, op: &Operand) -> Option<i64> {
    match op {
        Operand::Constant(Constant::Int(v)) => Some(*v),
        Operand::Copy(l) | Operand::Move(l) => {
            let mut value = None;
            for bb in &func.basic_blocks {
                for stmt in &bb.statements {
                    if let StatementKind::Assign(dest, rval) = &stmt.kind
                        && *dest == *l
                    {
                        match rval {
                            Rvalue::Use(Operand::Constant(Constant::Int(v))) if value.is_none() => {
                                value = Some(*v);
                            }
                            _ => return None,
                        }
                    }
                }
            }
            value
        }
        _ => None,
    }
}

fn successors(kind: &TerminatorKind) -> Vec<BasicBlockId> {
    match kind {
        TerminatorKind::Goto { target } => vec![*target],
        TerminatorKind::SwitchInt {
            targets, otherwise, ..
        } => {
            let mut s: Vec<_> = targets.iter().map(|(_, t)| *t).collect();
            s.push(*otherwise);
            s
        }
        _ => vec![],
    }
}

fn transform(func: &mut MirFunction, lp: &loop_utils::Loop, plan: &UnrollPlan) -> bool {
    match plan.const_trip {
        Some(n) if (1..=FULL_TRIP_LIMIT).contains(&n) => {
            if plan.work.len().saturating_mul(n as usize) > MAX_EXPANSION {
                return false;
            }
            full_unroll(func, lp, plan, n)
        }
        _ => {
            let factor = PARTIAL_FACTOR;
            if plan.work.len().saturating_mul(factor as usize) > MAX_EXPANSION {
                return false;
            }
            partial_unroll(func, lp, plan, factor)
        }
    }
}

fn full_unroll(
    func: &mut MirFunction,
    lp: &loop_utils::Loop,
    plan: &UnrollPlan,
    trip: i64,
) -> bool {
    let start = match const_init(func, lp, plan.induction) {
        Some(v) => v,
        None => return false,
    };

    let mut stmts = Vec::with_capacity(plan.work.len() * trip as usize);
    for j in 0..trip {
        let value = start + j * plan.step;
        for stmt in &plan.work {
            // Induction and guard locals are gone; drop their storage markers.
            if is_storage_of(stmt, plan.induction) || is_storage_of(stmt, plan.cond_local) {
                continue;
            }
            let mut s = stmt.clone();
            subst_local(
                &mut s,
                plan.induction,
                &Operand::Constant(Constant::Int(value)),
            );
            stmts.push(s);
        }
    }

    // Code after the loop may still read the induction local; give it the
    // value the final guard evaluation would have observed.
    stmts.push(Statement {
        kind: StatementKind::Assign(
            plan.induction,
            Rvalue::Use(Operand::Constant(Constant::Int(start + trip * plan.step))),
        ),
        span: Span::default(),
    });

    let unrolled = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: stmts,
        terminator: Some(Terminator {
            kind: TerminatorKind::Goto { target: plan.exit },
            span: Span::default(),
        }),
    });

    redirect_entries(func, lp, unrolled);
    true
}

fn partial_unroll(
    func: &mut MirFunction,
    lp: &loop_utils::Loop,
    plan: &UnrollPlan,
    factor: i64,
) -> bool {
    // Clone before mutating; the clone becomes the remainder epilogue.
    let epilogue_map = loop_utils::clone_blocks(func, &lp.body);
    let epilogue_header = match epilogue_map.get(&lp.header) {
        Some(&h) => h,
        None => return false,
    };

    // Pre-header: adj = limit - (factor - 1) * step.
    let adj = new_int_local(func, "unroll_limit");
    let pre_header = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: vec![Statement {
            kind: StatementKind::Assign(
                adj,
                Rvalue::BinaryOp(
                    BinOp::Sub,
                    plan.limit.clone(),
                    Operand::Constant(Constant::Int((factor - 1) * plan.step)),
                ),
            ),
            span: Span::default(),
        }],
        terminator: Some(Terminator {
            kind: TerminatorKind::Goto { target: lp.header },
            span: Span::default(),
        }),
    });

    // factor copies at offset j*step, then i += factor*step.
    let mut body_stmts = Vec::with_capacity(plan.work.len() * factor as usize);
    for j in 0..factor {
        let idx_op = if j == 0 {
            Operand::Copy(plan.induction)
        } else {
            let off = new_int_local(func, "unroll_idx");
            body_stmts.push(Statement {
                kind: StatementKind::Assign(
                    off,
                    Rvalue::BinaryOp(
                        BinOp::Add,
                        Operand::Copy(plan.induction),
                        Operand::Constant(Constant::Int(j * plan.step)),
                    ),
                ),
                span: Span::default(),
            });
            Operand::Copy(off)
        };
        for stmt in &plan.work {
            // Induction stays live across copies; drop per-copy storage markers.
            if is_storage_of(stmt, plan.induction) {
                continue;
            }
            let mut s = stmt.clone();
            subst_local(&mut s, plan.induction, &idx_op);
            body_stmts.push(s);
        }
    }
    body_stmts.push(Statement {
        kind: StatementKind::Assign(
            plan.induction,
            Rvalue::BinaryOp(
                BinOp::Add,
                Operand::Copy(plan.induction),
                Operand::Constant(Constant::Int(factor * plan.step)),
            ),
        ),
        span: Span::default(),
    });
    let unrolled_body = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: body_stmts,
        terminator: Some(Terminator {
            kind: TerminatorKind::Goto { target: lp.header },
            span: Span::default(),
        }),
    });

    // Header guards against adj, branches to the unrolled body or the epilogue.
    let header = &mut func.basic_blocks[lp.header.0];
    for stmt in &mut header.statements {
        if let StatementKind::Assign(l, Rvalue::BinaryOp(_, lhs, rhs)) = &mut stmt.kind
            && *l == plan.cond_local
        {
            *lhs = Operand::Copy(plan.induction);
            *rhs = Operand::Copy(adj);
        }
    }
    if let Some(term) = &mut header.terminator
        && let TerminatorKind::SwitchInt {
            targets, otherwise, ..
        } = &mut term.kind
    {
        for (_, t) in targets.iter_mut() {
            if *t == plan.body_entry {
                *t = unrolled_body;
            } else if *t == plan.exit {
                *t = epilogue_header;
            }
        }
        if *otherwise == plan.body_entry {
            *otherwise = unrolled_body;
        } else if *otherwise == plan.exit {
            *otherwise = epilogue_header;
        }
    }

    redirect_external(func, lp, &epilogue_map, pre_header);
    true
}

/// Repoints external edges into the header at `target`.
fn redirect_entries(func: &mut MirFunction, lp: &loop_utils::Loop, target: BasicBlockId) {
    for (idx, bb) in func.basic_blocks.iter_mut().enumerate() {
        if lp.body.contains(&BasicBlockId(idx)) {
            continue;
        }
        retarget(bb, lp.header, target);
    }
}

/// Like [`redirect_entries`], also skipping the cloned epilogue.
fn redirect_external(
    func: &mut MirFunction,
    lp: &loop_utils::Loop,
    epilogue_map: &rustc_hash::FxHashMap<BasicBlockId, BasicBlockId>,
    target: BasicBlockId,
) {
    let limit = func.basic_blocks.len();
    for idx in 0..limit {
        let id = BasicBlockId(idx);
        if lp.body.contains(&id) || epilogue_map.values().any(|&v| v == id) || id == target {
            continue;
        }
        retarget(&mut func.basic_blocks[idx], lp.header, target);
    }
}

fn retarget(bb: &mut BasicBlock, from: BasicBlockId, to: BasicBlockId) {
    if let Some(term) = &mut bb.terminator {
        match &mut term.kind {
            TerminatorKind::Goto { target } if *target == from => *target = to,
            TerminatorKind::SwitchInt {
                targets, otherwise, ..
            } => {
                for (_, t) in targets.iter_mut() {
                    if *t == from {
                        *t = to;
                    }
                }
                if *otherwise == from {
                    *otherwise = to;
                }
            }
            _ => {}
        }
    }
}

fn new_int_local(func: &mut MirFunction, name: &str) -> Local {
    let id = Local(func.locals.len());
    func.locals.push(LocalDecl {
        ty: OliveType::Int,
        name: Some(name.into()),
        span: Span::default(),
        is_mut: false,
        is_owning: true,
    });
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp() -> Span {
        Span::default()
    }

    fn int_local() -> LocalDecl {
        LocalDecl {
            ty: OliveType::Int,
            name: None,
            span: sp(),
            is_mut: true,
            is_owning: true,
        }
    }

    fn bool_local() -> LocalDecl {
        LocalDecl {
            ty: OliveType::Bool,
            name: None,
            span: sp(),
            is_mut: true,
            is_owning: true,
        }
    }

    fn assign(l: usize, rv: Rvalue) -> Statement {
        Statement {
            kind: StatementKind::Assign(Local(l), rv),
            span: sp(),
        }
    }

    fn term(kind: TerminatorKind) -> Option<Terminator> {
        Some(Terminator { kind, span: sp() })
    }

    fn func(locals: Vec<LocalDecl>, blocks: Vec<BasicBlock>, arg_count: usize) -> MirFunction {
        MirFunction {
            name: "f".into(),
            locals,
            basic_blocks: blocks,
            arg_count,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    /// Builds `for i in 0..limit { acc += i }`. limit const or copy of local 0.
    fn counted_loop(limit: Operand, arg_count: usize, indirect: bool) -> MirFunction {
        // locals: 0=param/spare, 1=i, 2=cond, 3=acc, 4=tmp
        let locals = vec![
            int_local(),
            int_local(),
            bool_local(),
            int_local(),
            int_local(),
        ];

        let entry = BasicBlock {
            statements: vec![
                assign(1, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
                assign(3, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            ],
            terminator: term(TerminatorKind::Goto {
                target: BasicBlockId(1),
            }),
        };
        let header = BasicBlock {
            statements: vec![assign(
                2,
                Rvalue::BinaryOp(BinOp::Lt, Operand::Copy(Local(1)), limit),
            )],
            terminator: term(TerminatorKind::SwitchInt {
                discr: Operand::Copy(Local(2)),
                targets: vec![(1, BasicBlockId(2))],
                otherwise: BasicBlockId(3),
            }),
        };
        let mut body = vec![assign(
            3,
            Rvalue::BinaryOp(BinOp::Add, Operand::Copy(Local(3)), Operand::Copy(Local(1))),
        )];
        if indirect {
            body.push(assign(
                4,
                Rvalue::BinaryOp(
                    BinOp::Add,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(1)),
                ),
            ));
            body.push(assign(1, Rvalue::Use(Operand::Copy(Local(4)))));
        } else {
            body.push(assign(
                1,
                Rvalue::BinaryOp(
                    BinOp::Add,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(1)),
                ),
            ));
        }
        let body = BasicBlock {
            statements: body,
            terminator: term(TerminatorKind::Goto {
                target: BasicBlockId(1),
            }),
        };
        let exit = BasicBlock {
            statements: vec![],
            terminator: term(TerminatorKind::Return),
        };
        func(locals, vec![entry, header, body, exit], arg_count)
    }

    fn reaches_loop(func: &MirFunction) -> bool {
        !loop_utils::find_loops(func).is_empty()
    }

    #[test]
    fn full_unroll_constant_trip() {
        let mut f = counted_loop(Operand::Constant(Constant::Int(4)), 0, false);
        assert!(LoopUnroll.run(&mut f));
        // One `acc += <const>` per iteration, no induction comparison left.
        let unrolled = f
            .basic_blocks
            .iter()
            .find(|bb| {
                matches!(
                    bb.terminator.as_ref().map(|t| &t.kind),
                    Some(TerminatorKind::Goto {
                        target: BasicBlockId(3)
                    })
                ) && bb
                    .statements
                    .iter()
                    .filter(|s| matches!(s.kind, StatementKind::Assign(Local(3), _)))
                    .count()
                    == 4
            })
            .expect("unrolled body with four copies");
        for s in &unrolled.statements {
            if let StatementKind::Assign(Local(3), Rvalue::BinaryOp(_, _, b)) = &s.kind {
                assert!(matches!(b, Operand::Constant(Constant::Int(_))));
            }
        }
    }

    #[test]
    fn full_unroll_indirect_increment() {
        let mut f = counted_loop(Operand::Constant(Constant::Int(3)), 0, true);
        assert!(LoopUnroll.run(&mut f));
    }

    #[test]
    fn partial_unroll_runtime_bound() {
        let mut f = counted_loop(Operand::Copy(Local(0)), 1, false);
        assert!(LoopUnroll.run(&mut f));
        let names: Vec<&str> = f.locals.iter().filter_map(|l| l.name.as_deref()).collect();
        assert!(names.contains(&"unroll_limit"));
        // Offset temps for copies 1, 2, 3.
        assert_eq!(names.iter().filter(|n| **n == "unroll_idx").count(), 3);
        assert!(reaches_loop(&f), "partial unroll keeps a residual loop");
    }

    #[test]
    fn no_unroll_without_induction() {
        let mut f = counted_loop(Operand::Constant(Constant::Int(4)), 0, false);
        // Drop the increment.
        f.basic_blocks[2].statements.pop();
        assert!(!LoopUnroll.run(&mut f));
    }

    #[test]
    fn no_unroll_nonlinear_body() {
        let mut f = counted_loop(Operand::Constant(Constant::Int(4)), 0, false);
        // Give the body an internal branch.
        f.basic_blocks[2].terminator = term(TerminatorKind::SwitchInt {
            discr: Operand::Copy(Local(3)),
            targets: vec![(1, BasicBlockId(1))],
            otherwise: BasicBlockId(3),
        });
        assert!(!LoopUnroll.run(&mut f));
    }

    #[test]
    fn no_unroll_when_no_loops() {
        let mut f = func(
            vec![int_local()],
            vec![BasicBlock {
                statements: vec![],
                terminator: term(TerminatorKind::Return),
            }],
            0,
        );
        assert!(!LoopUnroll.run(&mut f));
    }
}
