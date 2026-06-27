use crate::mir::loop_utils;
use crate::mir::optimizations::Transform;
use crate::mir::*;
use crate::parser::BinOp;
use rustc_hash::{FxHashMap, FxHashSet};

/// Three proofs: const-index, redundant same-block, induction bounded by len.
pub struct BoundsCheckElim;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum IdxKey {
    Const(i64),
    Local(usize),
}

/// Known length: compile-time constant or immutable local from list_new(n).
#[derive(Clone, Copy, PartialEq, Eq)]
enum LenKey {
    Const(usize),
    Sym(usize),
}

impl Transform for BoundsCheckElim {
    fn run(&self, func: &mut MirFunction) -> bool {
        let single = single_defs(func);
        let counts = assign_counts(func);
        let lengths = list_lengths(func, &single, &counts);

        let mut marks: FxHashSet<(usize, usize)> = FxHashSet::default();
        self.collect_local(func, &single, &lengths, &mut marks);
        self.collect_loops(func, &counts, &mut marks);

        if marks.is_empty() {
            return false;
        }
        for &(bb, idx) in &marks {
            match &mut func.basic_blocks[bb].statements[idx].kind {
                StatementKind::Assign(_, Rvalue::GetIndex(_, _, u))
                | StatementKind::SetIndex(_, _, _, u) => *u = true,
                _ => {}
            }
        }
        true
    }
}

impl BoundsCheckElim {
    /// Proof cases 1+2: const index into fixed-length aggregate; redundant same-block re-check.
    fn collect_local(
        &self,
        func: &MirFunction,
        single: &FxHashMap<usize, Rvalue>,
        lengths: &FxHashMap<usize, LenKey>,
        marks: &mut FxHashSet<(usize, usize)>,
    ) {
        for (bb_idx, bb) in func.basic_blocks.iter().enumerate() {
            let mut checked: FxHashSet<(usize, IdxKey)> = FxHashSet::default();
            for (s_idx, stmt) in bb.statements.iter().enumerate() {
                if let Some((obj, idx, already)) = index_access(&stmt.kind) {
                    let origin = operand_local(obj).map(|l| resolve(single, l));
                    let key = idx_key(single, idx);
                    if let (Some(o), Some(k)) = (origin, key) {
                        if !already {
                            let in_fixed = match k {
                                IdxKey::Const(c) => matches!(
                                    lengths.get(&o),
                                    Some(LenKey::Const(n)) if c >= 0 && (c as usize) < *n
                                ),
                                IdxKey::Local(_) => false,
                            };
                            if in_fixed || checked.contains(&(o, k)) {
                                marks.insert((bb_idx, s_idx));
                            }
                        }
                        checked.insert((o, k));
                    }
                    continue;
                }
                invalidate(&stmt.kind, &mut checked);
            }
        }
    }

    /// Proof case 3: induction bounded by len(obj); state from loop entry to avoid slot reuse.
    fn collect_loops(
        &self,
        func: &MirFunction,
        counts: &FxHashMap<usize, usize>,
        marks: &mut FxHashSet<(usize, usize)>,
    ) {
        let doms = loop_utils::dominators(func);
        for lp in loop_utils::find_loops(func) {
            let Some((i, limit_op)) = header_guard(func, lp.header) else {
                continue;
            };
            let entry = entry_state(func, &doms, lp.header);
            if !nonneg_induction(func, &lp, &entry, i) {
                continue;
            }

            let mut bounded: FxHashMap<usize, bool> = FxHashMap::default();

            for &bb_id in &lp.body {
                for (s_idx, stmt) in func.basic_blocks[bb_id.0].statements.iter().enumerate() {
                    let Some((obj, idx, already)) = index_access(&stmt.kind) else {
                        continue;
                    };
                    // The index must be the induction variable read directly:
                    // it is reassigned each iteration, so it has no loop-entry
                    // value to resolve through.
                    if already || operand_local(idx) != Some(i) {
                        continue;
                    }
                    let Some(o) = operand_local(obj) else {
                        continue;
                    };
                    let ok = *bounded.entry(o).or_insert_with(|| {
                        !mutated_in_loop(func, &lp, o)
                            && limit_bounds_object(&entry, counts, &limit_op, o)
                    });
                    if ok {
                        marks.insert((bb_id.0, s_idx));
                    }
                }
            }
        }
    }
}

/// Last write to each local across dominators of the loop header, in dominance order.
fn entry_state(
    func: &MirFunction,
    doms: &[FxHashSet<BasicBlockId>],
    header: BasicBlockId,
) -> FxHashMap<usize, Rvalue> {
    let mut prelude: Vec<BasicBlockId> = doms[header.0].iter().copied().collect();
    prelude.sort_by_key(|b| doms[b.0].len());

    let mut state: FxHashMap<usize, Rvalue> = FxHashMap::default();
    for b in prelude {
        for stmt in &func.basic_blocks[b.0].statements {
            if let StatementKind::Assign(dst, rval) = &stmt.kind {
                state.insert(dst.0, rval.clone());
            }
        }
    }
    state
}

/// Follows `x = Use(Copy/Move(y))` chains within a loop-entry state.
fn resolve_entry(entry: &FxHashMap<usize, Rvalue>, local: usize) -> usize {
    let mut cur = local;
    for _ in 0..=entry.len() {
        match entry.get(&cur) {
            Some(Rvalue::Use(Operand::Copy(src) | Operand::Move(src))) if src.0 != cur => {
                cur = src.0;
            }
            _ => break,
        }
    }
    cur
}

/// Length of obj at loop entry: from a literal aggregate or list_new(constant/immutable-local).
fn length_at_entry(entry: &FxHashMap<usize, Rvalue>, obj: usize) -> Option<LenKey> {
    match entry.get(&resolve_entry(entry, obj))? {
        Rvalue::Aggregate(AggregateKind::List | AggregateKind::Tuple, ops) => {
            Some(LenKey::Const(ops.len()))
        }
        Rvalue::Call { func, args } if is_list_new(func) && args.len() == 1 => match &args[0] {
            Operand::Constant(Constant::Int(c)) if *c >= 0 => Some(LenKey::Const(*c as usize)),
            Operand::Copy(l) | Operand::Move(l) => Some(LenKey::Sym(resolve_entry(entry, l.0))),
            _ => None,
        },
        _ => None,
    }
}

/// True if i < limit guarantees i < len(obj): limit is len(obj), list_new param, or const bound.
fn limit_bounds_object(
    entry: &FxHashMap<usize, Rvalue>,
    counts: &FxHashMap<usize, usize>,
    limit: &Operand,
    obj: usize,
) -> bool {
    if let Some(l) = operand_local(limit)
        && len_source_at_entry(entry, l) == Some(resolve_entry(entry, obj))
    {
        return true;
    }
    match length_at_entry(entry, obj) {
        Some(LenKey::Const(n)) => {
            matches!(limit, Operand::Constant(Constant::Int(m)) if *m >= 0 && (*m as usize) <= n)
        }
        // The symbolic length holds only if that local is genuinely immutable
        // (never assigned), so its value at the guard matches its value at
        // construction.
        Some(LenKey::Sym(s)) => {
            counts.get(&s).copied().unwrap_or(0) == 0
                && operand_local(limit).map(|l| resolve_entry(entry, l)) == Some(s)
        }
        None => false,
    }
}

/// If `limit` is defined at loop entry as `len(obj)`, the resolved object local.
fn len_source_at_entry(entry: &FxHashMap<usize, Rvalue>, limit: usize) -> Option<usize> {
    match entry.get(&resolve_entry(entry, limit))? {
        Rvalue::Call { func, args } if args.len() == 1 && is_len_call(func) => {
            operand_local(&args[0]).map(|l| resolve_entry(entry, l))
        }
        _ => None,
    }
}

/// Single-assignment loop-body locals mapped to their rvalue; used to trace i+1 temporaries.
fn body_defs(func: &MirFunction, lp: &loop_utils::Loop) -> FxHashMap<usize, Rvalue> {
    let mut count: FxHashMap<usize, usize> = FxHashMap::default();
    let mut def: FxHashMap<usize, Rvalue> = FxHashMap::default();
    for &bb_id in &lp.body {
        for stmt in &func.basic_blocks[bb_id.0].statements {
            if let StatementKind::Assign(dst, rval) = &stmt.kind {
                *count.entry(dst.0).or_insert(0) += 1;
                def.insert(dst.0, rval.clone());
            }
        }
    }
    def.retain(|l, _| count.get(l) == Some(&1));
    def
}

/// True if rval is i + non-negative constant, tracing single-assignment temps.
fn is_nonneg_step(body: &FxHashMap<usize, Rvalue>, i: usize, rval: &Rvalue) -> bool {
    let mut cur = rval;
    for _ in 0..=body.len() {
        match cur {
            Rvalue::BinaryOp(BinOp::Add, Operand::Copy(s), Operand::Constant(Constant::Int(k))) => {
                return s.0 == i && *k >= 0;
            }
            Rvalue::Use(Operand::Copy(t) | Operand::Move(t)) => match body.get(&t.0) {
                Some(next) => cur = next,
                None => return false,
            },
            _ => return false,
        }
    }
    false
}

/// True if i only increments by positive steps and starts non-negative (stays in [0, limit)).
fn nonneg_induction(
    func: &MirFunction,
    lp: &loop_utils::Loop,
    entry: &FxHashMap<usize, Rvalue>,
    i: usize,
) -> bool {
    let body = body_defs(func, lp);
    for &bb_id in &lp.body {
        for stmt in &func.basic_blocks[bb_id.0].statements {
            if let StatementKind::Assign(dst, rval) = &stmt.kind
                && dst.0 == i
                && !is_nonneg_step(&body, i, rval)
            {
                return false;
            }
        }
    }
    matches!(
        entry.get(&resolve_entry(entry, i)),
        Some(Rvalue::Use(Operand::Constant(Constant::Int(c)))) if *c >= 0
    )
}

/// True if obj is reassigned, resized, or aliased anywhere in the loop; SetIndex excluded.
fn mutated_in_loop(func: &MirFunction, lp: &loop_utils::Loop, obj: usize) -> bool {
    for &bb_id in &lp.body {
        for stmt in &func.basic_blocks[bb_id.0].statements {
            match &stmt.kind {
                StatementKind::Assign(dst, rval) => {
                    if dst.0 == obj {
                        return true;
                    }
                    match rval {
                        // The length accessor only reads the header.
                        Rvalue::Call { func, args } if !is_len_call(func) => {
                            if args.iter().any(|a| operand_local(a) == Some(obj)) {
                                return true;
                            }
                        }
                        Rvalue::Ref(l) | Rvalue::MutRef(l) if l.0 == obj => return true,
                        _ => {}
                    }
                }
                StatementKind::SetAttr(o, _, _) => {
                    if operand_local(o) == Some(obj) {
                        return true;
                    }
                }
                _ => {}
            }
        }
    }
    false
}

/// The `(object, index, already_unchecked)` of an indexed read or write.
fn index_access(kind: &StatementKind) -> Option<(&Operand, &Operand, bool)> {
    match kind {
        StatementKind::Assign(_, Rvalue::GetIndex(obj, idx, u)) => Some((obj, idx, *u)),
        StatementKind::SetIndex(obj, idx, _, u) => Some((obj, idx, *u)),
        _ => None,
    }
}

fn operand_local(op: &Operand) -> Option<usize> {
    match op {
        Operand::Copy(l) | Operand::Move(l) => Some(l.0),
        Operand::Constant(_) => None,
    }
}

/// Single-assignment locals mapped to their rvalue; multi-assigned locals omitted.
fn single_defs(func: &MirFunction) -> FxHashMap<usize, Rvalue> {
    let mut count: FxHashMap<usize, usize> = FxHashMap::default();
    let mut def: FxHashMap<usize, Rvalue> = FxHashMap::default();
    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            if let StatementKind::Assign(dst, rval) = &stmt.kind {
                *count.entry(dst.0).or_insert(0) += 1;
                def.insert(dst.0, rval.clone());
            }
        }
    }
    def.retain(|l, _| count.get(l) == Some(&1));
    def
}

/// Follows `x = Use(Copy(y))` chains to the underlying value's local.
fn resolve(single: &FxHashMap<usize, Rvalue>, local: usize) -> usize {
    let mut cur = local;
    for _ in 0..=single.len() {
        match single.get(&cur) {
            Some(Rvalue::Use(Operand::Copy(src) | Operand::Move(src))) if src.0 != cur => {
                cur = src.0;
            }
            _ => break,
        }
    }
    cur
}

fn idx_key(single: &FxHashMap<usize, Rvalue>, idx: &Operand) -> Option<IdxKey> {
    match idx {
        Operand::Constant(Constant::Int(c)) => Some(IdxKey::Const(*c)),
        Operand::Copy(l) | Operand::Move(l) => Some(IdxKey::Local(resolve(single, l.0))),
        Operand::Constant(_) => None,
    }
}

/// Number of assignments to each local.
fn assign_counts(func: &MirFunction) -> FxHashMap<usize, usize> {
    let mut count: FxHashMap<usize, usize> = FxHashMap::default();
    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            if let StatementKind::Assign(dst, _) = &stmt.kind {
                *count.entry(dst.0).or_insert(0) += 1;
            }
        }
    }
    count
}

/// Known lengths of single-assignment list/tuple locals; revoked on resize, call, or borrow.
fn list_lengths(
    func: &MirFunction,
    single: &FxHashMap<usize, Rvalue>,
    counts: &FxHashMap<usize, usize>,
) -> FxHashMap<usize, LenKey> {
    let mut len_of: FxHashMap<usize, LenKey> = FxHashMap::default();
    let mut disqualified: FxHashSet<usize> = FxHashSet::default();

    let disq = |op: &Operand, set: &mut FxHashSet<usize>| {
        if let Some(l) = operand_local(op) {
            set.insert(resolve(single, l));
        }
    };
    let once = |l: usize| counts.get(&l) == Some(&1);

    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            match &stmt.kind {
                StatementKind::Assign(dst, rval) => match rval {
                    Rvalue::Aggregate(AggregateKind::List | AggregateKind::Tuple, ops)
                        if once(dst.0) =>
                    {
                        len_of.insert(dst.0, LenKey::Const(ops.len()));
                    }
                    Rvalue::Call { func, args }
                        if is_list_new(func) && args.len() == 1 && once(dst.0) =>
                    {
                        match &args[0] {
                            Operand::Constant(Constant::Int(c)) if *c >= 0 => {
                                len_of.insert(dst.0, LenKey::Const(*c as usize));
                            }
                            Operand::Copy(l) | Operand::Move(l) => {
                                let s = resolve(single, l.0);
                                if counts.get(&s).copied().unwrap_or(0) == 0 {
                                    len_of.insert(dst.0, LenKey::Sym(s));
                                }
                            }
                            _ => {}
                        }
                    }
                    // Reading the length or allocating never resizes the object.
                    Rvalue::Call { func, .. } if is_len_call(func) || is_list_new(func) => {}
                    // Any other call may resize or realloc the collection
                    // (append, pop, extend), and a borrow may be resized
                    // through, so either forfeits the length guarantee.
                    Rvalue::Call { args, .. } => {
                        for a in args {
                            disq(a, &mut disqualified);
                        }
                    }
                    Rvalue::Ref(l) | Rvalue::MutRef(l) => {
                        disqualified.insert(resolve(single, l.0));
                    }
                    _ => {}
                },
                StatementKind::SetAttr(obj, _, val) => {
                    disq(obj, &mut disqualified);
                    disq(val, &mut disqualified);
                }
                _ => {}
            }
        }
    }

    len_of.retain(|l, _| !disqualified.contains(l));
    len_of
}

fn is_list_new(func: &Operand) -> bool {
    matches!(func, Operand::Constant(Constant::Function(name)) if name == "__olive_list_new")
}

/// Evicts (object, index) pairs a statement may invalidate.
fn invalidate(kind: &StatementKind, checked: &mut FxHashSet<(usize, IdxKey)>) {
    match kind {
        StatementKind::Assign(dst, rval) => {
            if matches!(
                rval,
                Rvalue::Call { .. } | Rvalue::Ref(_) | Rvalue::MutRef(_)
            ) {
                checked.clear();
                return;
            }
            checked.retain(|(o, k)| *o != dst.0 && *k != IdxKey::Local(dst.0));
        }
        StatementKind::SetAttr(..) | StatementKind::PtrStore(..) => checked.clear(),
        _ => {}
    }
}

/// Guard `i < L` at the loop header; None if the comparison isn't there.
fn header_guard(func: &MirFunction, header: BasicBlockId) -> Option<(usize, Operand)> {
    let block = &func.basic_blocks[header.0];
    if !matches!(
        block.terminator.as_ref().map(|t| &t.kind),
        Some(TerminatorKind::SwitchInt { .. })
    ) {
        return None;
    }
    for stmt in block.statements.iter().rev() {
        if let StatementKind::Assign(_, Rvalue::BinaryOp(BinOp::Lt, Operand::Copy(i), lim)) =
            &stmt.kind
        {
            return Some((i.0, lim.clone()));
        }
    }
    None
}

fn is_len_call(func: &Operand) -> bool {
    matches!(func, Operand::Constant(Constant::Function(name)) if name == "__olive_list_len")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::types::Type;
    use crate::span::Span;

    fn sp() -> Span {
        Span {
            file_id: 0,
            line: 0,
            col: 0,
            start: 0,
            end: 0,
        }
    }

    fn assign(l: usize, rv: Rvalue) -> Statement {
        Statement {
            kind: StatementKind::Assign(Local(l), rv),
            span: sp(),
        }
    }

    fn stmt(k: StatementKind) -> Statement {
        Statement {
            kind: k,
            span: sp(),
        }
    }

    fn decl(ty: Type) -> LocalDecl {
        LocalDecl {
            ty,
            name: None,
            span: sp(),
            is_mut: true,
            is_owning: true,
        }
    }

    fn func(locals: Vec<LocalDecl>, blocks: Vec<BasicBlock>) -> MirFunction {
        MirFunction {
            name: "f".into(),
            locals,
            basic_blocks: blocks,
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    fn bb(stmts: Vec<Statement>, term: TerminatorKind) -> BasicBlock {
        BasicBlock {
            statements: stmts,
            terminator: Some(Terminator {
                kind: term,
                span: sp(),
            }),
        }
    }

    fn int(c: i64) -> Operand {
        Operand::Constant(Constant::Int(c))
    }

    fn list_ty() -> Type {
        Type::List(Box::new(Type::Int))
    }

    /// Pulls the `unchecked` flag of the indexed access at a position.
    fn unchecked_at(f: &MirFunction, b: usize, s: usize) -> bool {
        match &f.basic_blocks[b].statements[s].kind {
            StatementKind::Assign(_, Rvalue::GetIndex(_, _, u)) => *u,
            StatementKind::SetIndex(_, _, _, u) => *u,
            other => panic!("not an index access: {other:?}"),
        }
    }

    #[test]
    fn const_index_into_fixed_list_is_unchecked() {
        let mut f = func(
            vec![decl(list_ty()), decl(Type::Int)],
            vec![bb(
                vec![
                    assign(
                        0,
                        Rvalue::Aggregate(AggregateKind::List, vec![int(7), int(8)]),
                    ),
                    assign(1, Rvalue::GetIndex(Operand::Copy(Local(0)), int(1), false)),
                ],
                TerminatorKind::Return,
            )],
        );
        assert!(BoundsCheckElim.run(&mut f));
        assert!(unchecked_at(&f, 0, 1));
    }

    #[test]
    fn const_index_out_of_range_stays_checked() {
        let mut f = func(
            vec![decl(list_ty()), decl(Type::Int)],
            vec![bb(
                vec![
                    assign(
                        0,
                        Rvalue::Aggregate(AggregateKind::List, vec![int(7), int(8)]),
                    ),
                    assign(1, Rvalue::GetIndex(Operand::Copy(Local(0)), int(5), false)),
                ],
                TerminatorKind::Return,
            )],
        );
        assert!(!BoundsCheckElim.run(&mut f));
        assert!(!unchecked_at(&f, 0, 1));
    }

    #[test]
    fn list_grown_by_call_stays_checked() {
        // A call takes the list, so its length is no longer statically known.
        let mut f = func(
            vec![decl(list_ty()), decl(Type::Int), decl(Type::Int)],
            vec![bb(
                vec![
                    assign(0, Rvalue::Aggregate(AggregateKind::List, vec![int(7)])),
                    assign(
                        2,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function("__olive_append".into())),
                            args: vec![Operand::Copy(Local(0)), int(9)],
                        },
                    ),
                    assign(1, Rvalue::GetIndex(Operand::Copy(Local(0)), int(0), false)),
                ],
                TerminatorKind::Return,
            )],
        );
        assert!(!BoundsCheckElim.run(&mut f));
        assert!(!unchecked_at(&f, 0, 2));
    }

    #[test]
    fn redundant_same_block_access_is_unchecked() {
        // Two reads of the same dynamic index; the second rides the first check.
        let mut f = func(
            vec![
                decl(list_ty()),
                decl(Type::Int),
                decl(Type::Int),
                decl(Type::Int),
            ],
            vec![bb(
                vec![
                    assign(3, Rvalue::Use(int(0))),
                    assign(
                        1,
                        Rvalue::GetIndex(Operand::Copy(Local(0)), Operand::Copy(Local(3)), false),
                    ),
                    assign(
                        2,
                        Rvalue::GetIndex(Operand::Copy(Local(0)), Operand::Copy(Local(3)), false),
                    ),
                ],
                TerminatorKind::Return,
            )],
        );
        assert!(BoundsCheckElim.run(&mut f));
        assert!(!unchecked_at(&f, 0, 1), "first access keeps its check");
        assert!(unchecked_at(&f, 0, 2), "second access is redundant");
    }

    #[test]
    fn redundant_access_checked_again_after_call() {
        let mut f = func(
            vec![
                decl(list_ty()),
                decl(Type::Int),
                decl(Type::Int),
                decl(Type::Int),
            ],
            vec![bb(
                vec![
                    assign(3, Rvalue::Use(int(0))),
                    assign(
                        1,
                        Rvalue::GetIndex(Operand::Copy(Local(0)), Operand::Copy(Local(3)), false),
                    ),
                    assign(
                        1,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function("__olive_append".into())),
                            args: vec![Operand::Copy(Local(0)), int(9)],
                        },
                    ),
                    assign(
                        2,
                        Rvalue::GetIndex(Operand::Copy(Local(0)), Operand::Copy(Local(3)), false),
                    ),
                ],
                TerminatorKind::Return,
            )],
        );
        assert!(!BoundsCheckElim.run(&mut f));
        assert!(!unchecked_at(&f, 0, 3), "call invalidated the prior check");
    }

    /// MIR for `i=0; while i<len(xs): xs[i]; i+=1`. Blocks: 0=preheader, 1=header, 2=body, 3=exit.
    fn len_bounded_loop(index_idx: Operand) -> MirFunction {
        let locals = vec![
            decl(list_ty()),  // 0: xs
            decl(Type::Int),  // 1: i
            decl(Type::Int),  // 2: len
            decl(Type::Bool), // 3: cond
            decl(Type::Int),  // 4: loaded
        ];
        let preheader = bb(
            vec![
                assign(
                    0,
                    Rvalue::Aggregate(AggregateKind::List, vec![int(1), int(2), int(3)]),
                ),
                assign(1, Rvalue::Use(int(0))),
            ],
            TerminatorKind::Goto {
                target: BasicBlockId(1),
            },
        );
        let header = bb(
            vec![
                assign(
                    2,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__olive_list_len".into())),
                        args: vec![Operand::Copy(Local(0))],
                    },
                ),
                assign(
                    3,
                    Rvalue::BinaryOp(BinOp::Lt, Operand::Copy(Local(1)), Operand::Copy(Local(2))),
                ),
            ],
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(Local(3)),
                targets: vec![(0, BasicBlockId(3))],
                otherwise: BasicBlockId(2),
            },
        );
        let body = bb(
            vec![
                assign(
                    4,
                    Rvalue::GetIndex(Operand::Copy(Local(0)), index_idx, false),
                ),
                assign(
                    1,
                    Rvalue::BinaryOp(BinOp::Add, Operand::Copy(Local(1)), int(1)),
                ),
            ],
            TerminatorKind::Goto {
                target: BasicBlockId(1),
            },
        );
        let exit = bb(vec![], TerminatorKind::Return);
        func(locals, vec![preheader, header, body, exit])
    }

    #[test]
    fn induction_bounded_by_len_is_unchecked() {
        let mut f = len_bounded_loop(Operand::Copy(Local(1)));
        assert!(BoundsCheckElim.run(&mut f));
        assert!(unchecked_at(&f, 2, 0));
    }

    #[test]
    fn induction_indexing_other_list_stays_checked() {
        // The bound is len(xs) but the access reads a different, unrelated list.
        let mut f = len_bounded_loop(Operand::Copy(Local(1)));
        f.locals.push(decl(list_ty()));
        f.basic_blocks[0].statements.push(assign(
            5,
            Rvalue::Aggregate(AggregateKind::List, vec![int(9)]),
        ));
        f.basic_blocks[2].statements[0] = assign(
            4,
            Rvalue::GetIndex(Operand::Copy(Local(5)), Operand::Copy(Local(1)), false),
        );
        assert!(!BoundsCheckElim.run(&mut f));
        assert!(!unchecked_at(&f, 2, 0));
    }

    #[test]
    fn loop_object_mutated_stays_checked() {
        // Appending to xs inside the loop breaks the len(xs) bound invariant.
        let mut f = len_bounded_loop(Operand::Copy(Local(1)));
        f.locals.push(decl(Type::Int));
        f.basic_blocks[2].statements.insert(
            1,
            assign(
                5,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_append".into())),
                    args: vec![Operand::Copy(Local(0)), int(0)],
                },
            ),
        );
        assert!(!BoundsCheckElim.run(&mut f));
        assert!(!unchecked_at(&f, 2, 0));
    }

    #[test]
    fn induction_bounded_by_list_new_param_is_unchecked() {
        // `fn(n): xs = list_new(n); while i < n: xs[i]`. The list length equals
        // the immutable parameter `n`, so the `i < n` guard bounds the index
        // even though the limit is not syntactically `len(xs)`.
        let locals = vec![
            decl(Type::Int),  // 0: return
            decl(Type::Int),  // 1: n (parameter, never assigned)
            decl(list_ty()),  // 2: xs
            decl(Type::Int),  // 3: i
            decl(Type::Bool), // 4: cond
            decl(Type::Int),  // 5: loaded
        ];
        let preheader = bb(
            vec![
                assign(
                    2,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__olive_list_new".into())),
                        args: vec![Operand::Copy(Local(1))],
                    },
                ),
                assign(3, Rvalue::Use(int(0))),
            ],
            TerminatorKind::Goto {
                target: BasicBlockId(1),
            },
        );
        let header = bb(
            vec![assign(
                4,
                Rvalue::BinaryOp(BinOp::Lt, Operand::Copy(Local(3)), Operand::Copy(Local(1))),
            )],
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(Local(4)),
                targets: vec![(0, BasicBlockId(3))],
                otherwise: BasicBlockId(2),
            },
        );
        let body = bb(
            vec![
                assign(
                    5,
                    Rvalue::GetIndex(Operand::Copy(Local(2)), Operand::Copy(Local(3)), false),
                ),
                assign(
                    3,
                    Rvalue::BinaryOp(BinOp::Add, Operand::Copy(Local(3)), int(1)),
                ),
            ],
            TerminatorKind::Goto {
                target: BasicBlockId(1),
            },
        );
        let exit = bb(vec![], TerminatorKind::Return);
        let mut f = func(locals, vec![preheader, header, body, exit]);
        f.arg_count = 1;
        assert!(BoundsCheckElim.run(&mut f));
        assert!(unchecked_at(&f, 2, 0));
    }

    #[test]
    fn induction_bounded_by_reassigned_bound_stays_checked() {
        // If the limit local is reassigned, it is no longer a stable length, so
        // the bound cannot be trusted.
        let mut f = induction_reassigned_bound();
        assert!(!BoundsCheckElim.run(&mut f));
        assert!(!unchecked_at(&f, 2, 0));
    }

    /// Same as len_bounded_loop but n is mutated in the loop body (breaks the bound invariant).
    fn induction_reassigned_bound() -> MirFunction {
        let locals = vec![
            decl(Type::Int),
            decl(Type::Int),
            decl(list_ty()),
            decl(Type::Int),
            decl(Type::Bool),
            decl(Type::Int),
        ];
        let preheader = bb(
            vec![
                assign(
                    2,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__olive_list_new".into())),
                        args: vec![Operand::Copy(Local(1))],
                    },
                ),
                assign(3, Rvalue::Use(int(0))),
            ],
            TerminatorKind::Goto {
                target: BasicBlockId(1),
            },
        );
        let header = bb(
            vec![assign(
                4,
                Rvalue::BinaryOp(BinOp::Lt, Operand::Copy(Local(3)), Operand::Copy(Local(1))),
            )],
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(Local(4)),
                targets: vec![(0, BasicBlockId(3))],
                otherwise: BasicBlockId(2),
            },
        );
        let body = bb(
            vec![
                assign(
                    5,
                    Rvalue::GetIndex(Operand::Copy(Local(2)), Operand::Copy(Local(3)), false),
                ),
                assign(
                    1,
                    Rvalue::BinaryOp(BinOp::Add, Operand::Copy(Local(1)), int(1)),
                ),
                assign(
                    3,
                    Rvalue::BinaryOp(BinOp::Add, Operand::Copy(Local(3)), int(1)),
                ),
            ],
            TerminatorKind::Goto {
                target: BasicBlockId(1),
            },
        );
        let exit = bb(vec![], TerminatorKind::Return);
        let mut f = func(locals, vec![preheader, header, body, exit]);
        f.arg_count = 1;
        f
    }

    #[test]
    fn setindex_in_len_bounded_loop_is_unchecked() {
        let mut f = len_bounded_loop(Operand::Copy(Local(1)));
        f.basic_blocks[2].statements[0] = stmt(StatementKind::SetIndex(
            Operand::Copy(Local(0)),
            Operand::Copy(Local(1)),
            int(42),
            false,
        ));
        assert!(BoundsCheckElim.run(&mut f));
        assert!(unchecked_at(&f, 2, 0));
    }
}
