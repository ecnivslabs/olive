use crate::mir::loop_utils;
use crate::mir::optimizations::Transform;
use crate::mir::*;
use crate::span::Span;
use rustc_hash::FxHashSet as HashSet;

/// Callees taking a `PyObject` as their first argument that only ever read
/// it (`__len__`, `__getitem__` on the builtin protocol path) -- never
/// resize, replace, or otherwise mutate what the argument points at.
/// `__olive_py_getitem`'s second argument is a key/index `PyObject`, never
/// the container itself, so `target` appearing there would be a distinct
/// (nonsensical) case already excluded by the arg-position check at the
/// call site.
const PURE_READ_CALLS: &[&str] = &[
    "__olive_py_len",
    "__olive_py_getitem_int",
    "__olive_py_getitem",
];

pub struct Licm;

impl Transform for Licm {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        let loops = loop_utils::find_loops(func);
        let mut processed_headers = HashSet::default();

        for lp in loops {
            if lp.header.0 == 0 || processed_headers.contains(&lp.header) {
                continue;
            }
            processed_headers.insert(lp.header);

            if self.optimize_loop(func, &lp) {
                changed = true;
                break;
            }
        }

        changed
    }
}

impl Licm {
    fn optimize_loop(&self, func: &mut MirFunction, lp: &loop_utils::Loop) -> bool {
        let mut assign_counts = rustc_hash::FxHashMap::default();
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(local, _) = &stmt.kind {
                    *assign_counts.entry(*local).or_insert(0) += 1;
                } else if let StatementKind::SetAttr(obj, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        *assign_counts.entry(*l).or_insert(0) += 1;
                    }
                } else if let StatementKind::SetIndex(obj, _, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        *assign_counts.entry(*l).or_insert(0) += 1;
                    }
                } else if let StatementKind::VectorStore(obj, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        *assign_counts.entry(*l).or_insert(0) += 1;
                    }
                } else if let StatementKind::PtrStore(ptr, _) = &stmt.kind
                    && let Operand::Copy(l) | Operand::Move(l) = ptr
                {
                    *assign_counts.entry(*l).or_insert(0) += 1;
                }
            }
        }

        let mut defined_in_loop = HashSet::default();
        for &bb_id in &lp.body {
            let bb = &func.basic_blocks[bb_id.0];
            for stmt in &bb.statements {
                if let StatementKind::Assign(local, _) = &stmt.kind {
                    defined_in_loop.insert(*local);
                } else if let StatementKind::SetAttr(obj, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        defined_in_loop.insert(*l);
                    }
                } else if let StatementKind::SetIndex(obj, _, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        defined_in_loop.insert(*l);
                    }
                } else if let StatementKind::VectorStore(obj, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        defined_in_loop.insert(*l);
                    }
                } else if let StatementKind::PtrStore(ptr, _) = &stmt.kind
                    && let Operand::Copy(l) | Operand::Move(l) = ptr
                {
                    defined_in_loop.insert(*l);
                }
            }
        }

        let mut sorted_body: Vec<BasicBlockId> = lp.body.iter().copied().collect();
        sorted_body.sort_by_key(|id| id.0);

        let mut invariant_stmts = Vec::new();
        let mut invariant_locals = HashSet::default();
        let mut loop_changed = true;

        while loop_changed {
            loop_changed = false;
            for &bb_id in &sorted_body {
                let bb = &func.basic_blocks[bb_id.0];
                for (i, stmt) in bb.statements.iter().enumerate() {
                    if let StatementKind::Assign(local, rval) = &stmt.kind
                        && func.locals[local.0].name.is_none()
                        && assign_counts.get(local) == Some(&1)
                        && !invariant_locals.contains(local)
                        // Hoisting a move-type value past its Drop would free it early.
                        && !func.locals[local.0].ty.is_move_type()
                        && (self.is_invariant(rval, &defined_in_loop, &invariant_locals)
                            && self.is_safe_to_hoist(rval)
                            || self.is_hoistable_py_len_call(rval, func, &lp.body, &defined_in_loop, &invariant_locals))
                    {
                        invariant_locals.insert(*local);
                        invariant_stmts.push((bb_id, i));
                        loop_changed = true;
                    }
                }
            }
        }

        if !invariant_stmts.is_empty() {
            self.hoist_invariants(func, lp.header, &lp.body, invariant_stmts)
        } else {
            false
        }
    }

    fn is_invariant(
        &self,
        rval: &Rvalue,
        defined_in_loop: &HashSet<Local>,
        invariant_locals: &HashSet<Local>,
    ) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => {
                self.is_op_invariant(op, defined_in_loop, invariant_locals)
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r, _) => {
                self.is_op_invariant(l, defined_in_loop, invariant_locals)
                    && self.is_op_invariant(r, defined_in_loop, invariant_locals)
            }
            _ => false,
        }
    }

    fn is_op_invariant(
        &self,
        op: &Operand,
        defined_in_loop: &HashSet<Local>,
        invariant_locals: &HashSet<Local>,
    ) -> bool {
        match op {
            Operand::Constant(_) => true,
            Operand::Copy(l) | Operand::Move(l) => {
                !defined_in_loop.contains(l) || invariant_locals.contains(l)
            }
        }
    }

    fn is_safe_to_hoist(&self, rval: &Rvalue) -> bool {
        matches!(
            rval,
            Rvalue::Use(_)
                | Rvalue::UnaryOp(_, _)
                | Rvalue::BinaryOp(_, _, _)
                | Rvalue::GetIndex(_, _, _)
        )
    }

    /// A `__olive_py_len(x)` call is loop-invariant, and safe to hoist past
    /// any GIL/exception cost it carries, exactly when `x` is never mutated
    /// or handed to anything that could mutate it inside the loop --
    /// `PyObject_Length` on a live Python object can only change value if
    /// the object itself changes. Deliberately narrow: this recognizes
    /// exactly one callee name and requires every other appearance of `x`
    /// in the loop body to be a plain `GetIndex` read (`x[i]`) or the
    /// len-call operand itself. Anything else -- `x` copied into another
    /// local, passed to any other call, targeted by `SetAttr`/`SetIndex`,
    /// dropped -- disqualifies it: any of those could plausibly resize or
    /// replace what `x` points at.
    fn is_hoistable_py_len_call(
        &self,
        rval: &Rvalue,
        func: &MirFunction,
        body: &HashSet<BasicBlockId>,
        defined_in_loop: &HashSet<Local>,
        invariant_locals: &HashSet<Local>,
    ) -> bool {
        let Rvalue::Call { func: callee, args } = rval else {
            return false;
        };
        let Operand::Constant(Constant::Function(name)) = callee else {
            return false;
        };
        if name != "__olive_py_len" || args.len() != 1 {
            return false;
        }
        let Some(target) = Self::operand_local(&args[0]) else {
            return false;
        };
        if !self.is_op_invariant(&args[0], defined_in_loop, invariant_locals) {
            return false;
        }
        Self::pyobj_untouched_except_reads(func, body, target)
    }

    fn operand_local(op: &Operand) -> Option<Local> {
        match op {
            Operand::Copy(l) | Operand::Move(l) => Some(*l),
            Operand::Constant(_) => None,
        }
    }

    fn operand_is_local(op: &Operand, target: Local) -> bool {
        Self::operand_local(op) == Some(target)
    }

    /// Every place `target` could appear in an `Rvalue`, generically --
    /// used to prove a local is untouched without hardcoding which variants
    /// exist; a variant this misses is a compile error the day it's added
    /// to `Rvalue`, not a silent soundness hole here.
    fn rvalue_operands(rval: &Rvalue) -> Vec<&Operand> {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => vec![op],
            Rvalue::BinaryOp(_, l, r) => vec![l, r],
            Rvalue::Call { func, args } => {
                let mut v = vec![func];
                v.extend(args.iter());
                v
            }
            Rvalue::Aggregate(_, ops) => ops.iter().collect(),
            Rvalue::Cast(op, _) => vec![op],
            Rvalue::GetAttr(op, _) => vec![op],
            Rvalue::GetIndex(obj, idx, _) => vec![obj, idx],
            Rvalue::GetTag(op) | Rvalue::GetTypeId(op) => vec![op],
            Rvalue::VectorSplat(op, _) | Rvalue::VectorReduce(_, op, _) => vec![op],
            Rvalue::VectorLoad(a, b, _) => vec![a, b],
            Rvalue::VectorFMA(a, b, c) => vec![a, b, c],
            Rvalue::PtrLoad(op) | Rvalue::FatPtrData(op) => vec![op],
            Rvalue::VTableLoad { vtable, .. } => vec![vtable],
            Rvalue::GenOf(op) => vec![op],
            Rvalue::Ref(_) | Rvalue::MutRef(_) => vec![],
        }
    }

    fn pyobj_untouched_except_reads(
        func: &MirFunction,
        body: &HashSet<BasicBlockId>,
        target: Local,
    ) -> bool {
        for &bb_id in body {
            for stmt in &func.basic_blocks[bb_id.0].statements {
                match &stmt.kind {
                    StatementKind::Assign(_, Rvalue::GetIndex(obj, idx, _)) => {
                        if Self::operand_is_local(idx, target) {
                            return false;
                        }
                        let _ = obj; // reading target[idx] is the one allowed use.
                    }
                    StatementKind::Assign(_, rval @ Rvalue::Call { func: callee, args }) => {
                        let is_pure_read_of_target = matches!(
                            callee,
                            Operand::Constant(Constant::Function(n))
                                if PURE_READ_CALLS.contains(&n.as_str())
                        ) && args
                            .first()
                            .is_some_and(|a| Self::operand_is_local(a, target))
                            && args[1..].iter().all(|a| !Self::operand_is_local(a, target));
                        if !is_pure_read_of_target
                            && Self::rvalue_operands(rval)
                                .iter()
                                .any(|op| Self::operand_is_local(op, target))
                        {
                            return false;
                        }
                    }
                    StatementKind::Assign(_, rval) => {
                        if Self::rvalue_operands(rval)
                            .iter()
                            .any(|op| Self::operand_is_local(op, target))
                        {
                            return false;
                        }
                    }
                    StatementKind::SetAttr(obj, _, val) => {
                        if Self::operand_is_local(obj, target)
                            || Self::operand_is_local(val, target)
                        {
                            return false;
                        }
                    }
                    StatementKind::SetIndex(obj, idx, val, _) => {
                        if Self::operand_is_local(obj, target)
                            || Self::operand_is_local(idx, target)
                            || Self::operand_is_local(val, target)
                        {
                            return false;
                        }
                    }
                    StatementKind::VectorStore(obj, idx, val) => {
                        if Self::operand_is_local(obj, target)
                            || Self::operand_is_local(idx, target)
                            || Self::operand_is_local(val, target)
                        {
                            return false;
                        }
                    }
                    StatementKind::PtrStore(ptr, val) => {
                        if Self::operand_is_local(ptr, target)
                            || Self::operand_is_local(val, target)
                        {
                            return false;
                        }
                    }
                    StatementKind::Drop(l) if *l == target => return false,
                    _ => {}
                }
            }
        }
        true
    }

    fn hoist_invariants(
        &self,
        func: &mut MirFunction,
        header: BasicBlockId,
        body: &HashSet<BasicBlockId>,
        invariant_stmts: Vec<(BasicBlockId, usize)>,
    ) -> bool {
        let pre_header_id = BasicBlockId(func.basic_blocks.len());
        func.basic_blocks.push(BasicBlock {
            statements: Vec::new(),
            terminator: Some(Terminator {
                kind: TerminatorKind::Goto { target: header },
                span: Span::default(),
            }),
        });

        let mut changed = false;
        for i in 0..func.basic_blocks.len() - 1 {
            let bb_id = BasicBlockId(i);
            if body.contains(&bb_id) {
                continue;
            }

            let bb = &mut func.basic_blocks[i];
            if let Some(term) = &mut bb.terminator {
                match &mut term.kind {
                    TerminatorKind::Goto { target } if *target == header => {
                        *target = pre_header_id;
                        changed = true;
                    }
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        for (_, t) in targets {
                            if *t == header {
                                *t = pre_header_id;
                                changed = true;
                            }
                        }
                        if *otherwise == header {
                            *otherwise = pre_header_id;
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
        }

        if !changed {
            func.basic_blocks.pop();
            return false;
        }

        let mut stmts_to_move = Vec::new();
        for (bb_id, stmt_idx) in invariant_stmts {
            let stmt = func.basic_blocks[bb_id.0].statements[stmt_idx].clone();
            stmts_to_move.push((bb_id, stmt_idx, stmt));
        }

        let mut to_remove = stmts_to_move
            .iter()
            .map(|(bb, idx, _)| (*bb, *idx))
            .collect::<Vec<_>>();
        to_remove.sort_by(|a, b| {
            if a.0 != b.0 {
                a.0.0.cmp(&b.0.0)
            } else {
                b.1.cmp(&a.1)
            }
        });

        for (bb_id, idx) in to_remove {
            func.basic_blocks[bb_id.0].statements.remove(idx);
        }

        for (_, _, stmt) in stmts_to_move {
            func.basic_blocks[pre_header_id.0].statements.push(stmt);
        }

        true
    }
}

#[cfg(test)]
#[cfg_attr(test, allow(dead_code))]
mod tests {
    use super::*;

    fn sp() -> crate::span::Span {
        crate::span::Span {
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

    fn func(blocks: Vec<BasicBlock>) -> MirFunction {
        MirFunction {
            name: "f".into(),
            locals: vec![],
            basic_blocks: blocks,
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    fn bb(stmts: Vec<Statement>, kind: TerminatorKind) -> BasicBlock {
        BasicBlock {
            statements: stmts,
            terminator: Some(Terminator { kind, span: sp() }),
        }
    }

    fn local_decl() -> LocalDecl {
        LocalDecl {
            ty: crate::semantic::types::Type::Int,
            name: None,
            span: sp(),
            is_mut: false,
            is_owning: false,
        }
    }

    #[test]
    fn no_loops_no_change() {
        let f = func(vec![bb(vec![], TerminatorKind::Return)]);
        let mut f2 = f.clone();
        assert!(!Licm.run(&mut f2));
    }

    #[test]
    fn preheader_not_inserted_when_no_loop() {
        let mut f = func(vec![
            bb(
                vec![],
                TerminatorKind::Goto {
                    target: BasicBlockId(1),
                },
            ),
            bb(vec![], TerminatorKind::Return),
        ]);
        assert!(!Licm.run(&mut f));
    }

    fn pyobj_decl() -> LocalDecl {
        LocalDecl {
            ty: crate::semantic::types::Type::PyObject,
            name: None,
            span: sp(),
            is_mut: true,
            is_owning: true,
        }
    }

    fn bool_decl() -> LocalDecl {
        LocalDecl {
            ty: crate::semantic::types::Type::Bool,
            name: None,
            span: sp(),
            is_mut: true,
            is_owning: false,
        }
    }

    fn len_call(dst: usize, xs: usize) -> Statement {
        assign(
            dst,
            Rvalue::Call {
                func: Operand::Constant(Constant::Function("__olive_py_len".to_string())),
                args: vec![Operand::Copy(Local(xs))],
            },
        )
    }

    /// `xs`(0)`, `len`(1), `cond`(2), `i`(3): `block0 -> block1(header,
    /// len+cmp) -> block2(body, getitem+i++, latch) -> block1`, exit block3.
    fn py_len_loop_func(body_extra: Vec<Statement>) -> MirFunction {
        let locals = vec![
            pyobj_decl(), // 0: xs
            local_decl(), // 1: len result (Int)
            bool_decl(),  // 2: cond
            local_decl(), // 3: i (Int)
            pyobj_decl(), // 4: xs[i] item
            local_decl(), // 5: i+1 temp
        ];
        let header_stmts = vec![
            len_call(1, 0),
            assign(
                2,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Lt,
                    Operand::Copy(Local(3)),
                    Operand::Copy(Local(1)),
                ),
            ),
        ];
        let mut latch_stmts = vec![
            assign(
                4,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(
                        "__olive_py_getitem_int".to_string(),
                    )),
                    args: vec![Operand::Copy(Local(0)), Operand::Copy(Local(3))],
                },
            ),
            assign(
                5,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(Local(3)),
                    Operand::Constant(Constant::Int(1)),
                ),
            ),
            assign(3, Rvalue::Use(Operand::Copy(Local(5)))),
        ];
        latch_stmts.extend(body_extra);
        MirFunction {
            name: "test_fn".to_string(),
            locals,
            basic_blocks: vec![
                bb(
                    vec![],
                    TerminatorKind::Goto {
                        target: BasicBlockId(1),
                    },
                ),
                bb(
                    header_stmts,
                    TerminatorKind::SwitchInt {
                        discr: Operand::Copy(Local(2)),
                        targets: vec![(1, BasicBlockId(2))],
                        otherwise: BasicBlockId(3),
                    },
                ),
                bb(
                    latch_stmts,
                    TerminatorKind::Goto {
                        target: BasicBlockId(1),
                    },
                ),
                bb(vec![], TerminatorKind::Return),
            ],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    fn count_py_len_calls(func: &MirFunction) -> usize {
        func.basic_blocks
            .iter()
            .flat_map(|b| &b.statements)
            .filter(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, Rvalue::Call { func: f, .. })
                        if matches!(f, Operand::Constant(Constant::Function(n)) if n == "__olive_py_len")
                )
            })
            .count()
    }

    fn len_call_is_in_loop_body(func: &MirFunction) -> bool {
        let loops = loop_utils::find_loops(func);
        let Some(lp) = loops.into_iter().find(|l| l.header == BasicBlockId(1)) else {
            return false;
        };
        lp.body.iter().any(|&bb_id| {
            func.basic_blocks[bb_id.0].statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, Rvalue::Call { func: f, .. })
                        if matches!(f, Operand::Constant(Constant::Function(n)) if n == "__olive_py_len")
                )
            })
        })
    }

    #[test]
    fn hoists_py_len_call_when_container_is_untouched() {
        let mut f = py_len_loop_func(vec![]);
        let changed = Licm.run(&mut f);
        assert!(changed);
        assert_eq!(
            count_py_len_calls(&f),
            1,
            "hoisting must not duplicate the call"
        );
        assert!(
            !len_call_is_in_loop_body(&f),
            "the len call must have moved out of the loop"
        );
    }

    #[test]
    fn does_not_hoist_py_len_when_container_is_set_indexed() {
        let mut f = py_len_loop_func(vec![stmt(StatementKind::SetIndex(
            Operand::Copy(Local(0)),
            Operand::Copy(Local(3)),
            Operand::Copy(Local(4)),
            false,
        ))]);
        let changed = Licm.run(&mut f);
        assert!(!changed || len_call_is_in_loop_body(&f));
    }

    #[test]
    fn does_not_hoist_py_len_when_container_escapes_to_another_call() {
        let mut f = py_len_loop_func(vec![assign(
            6,
            Rvalue::Call {
                func: Operand::Constant(Constant::Function("__olive_some_other_call".to_string())),
                args: vec![Operand::Copy(Local(0))],
            },
        )]);
        f.locals.push(pyobj_decl());
        let changed = Licm.run(&mut f);
        assert!(!changed || len_call_is_in_loop_body(&f));
    }

    #[test]
    fn does_not_hoist_py_len_when_container_is_reassigned_in_loop() {
        let mut f = py_len_loop_func(vec![assign(0, Rvalue::Use(Operand::Copy(Local(4))))]);
        let changed = Licm.run(&mut f);
        assert!(!changed || len_call_is_in_loop_body(&f));
    }
}
