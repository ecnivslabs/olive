//! GIL region fusion (R13). Wraps a basic-block-local run of two or more
//! Python-touching statements in one `__olive_py_gil_begin`/`_end` pair
//! instead of paying a `PyGILState_Ensure`/`Release` pair per statement.
//! Runs last, after every other pass has settled final statement order.
//!
//! A statement only counts as fusable when its codegen lowering is a call
//! this pass can name (`__olive_py_*`, or a `BinaryOp`/`UnaryOp`/`GetAttr`/
//! `GetIndex`/`Cast` on a `PyObject` operand, all of which route through
//! `with_gil` in the runtime). Anything else -- an unrecognized call, an
//! indirect call, a struct destructor, a thread spawn -- breaks the run
//! rather than risk holding the GIL across a call that could block or
//! re-enter Python. A fault inside a fused region aborts the process
//! (Olive faults never unwind), so an unmatched begin can never leak the
//! GIL into continued execution.
//!
//! Loops get the same treatment one level up: a natural loop whose every
//! statement is fusable holds the GIL across all its iterations
//! (`gil_begin` in a fresh preheader, `gil_end` on every exit edge) instead
//! of paying one ensure/release pair per iteration. Each latch gains a
//! `__olive_py_gil_checkpoint` so a long-held region still yields to other
//! GIL-waiting threads periodically, mirroring CPython's own switch
//! interval. Any statement the block-local classifier would call opaque
//! disqualifies the whole loop -- nothing that could block, spawn, or run
//! arbitrary native code for long stretches ever sits inside a held region.

use super::Transform;
use crate::mir::ir::*;
use crate::mir::loop_utils;
use crate::semantic::types::Type as OliveType;
use crate::span::Span;
use rustc_hash::FxHashSet;

const EXCLUDED_PY_CALLS: &[&str] = &[
    "__olive_py_gil_begin",
    "__olive_py_gil_end",
    "__olive_py_gil_checkpoint",
    "__olive_py_initialize",
    "__olive_py_finalize",
];

/// Compiler-emitted intrinsics with a name the MIR builder always writes
/// verbatim (never a user or FFI symbol), each a single thread-local write
/// or read with no possibility of blocking or touching Python. Safe to
/// bridge across the same as a `Neutral` statement.
const SAFE_NON_PY_CALLS: &[&str] = &["__olive_set_fault_loc"];

#[derive(PartialEq, Eq, Clone, Copy)]
enum StmtClass {
    /// Lowers to a call that already routes through the runtime's GIL
    /// depth counter -- safe to fuse across and worth wrapping.
    PyTouch,
    /// Never calls out (or calls something that provably can't touch the
    /// GIL or block) -- safe to bridge across without extending the run.
    Neutral,
    /// Anything else. Ends the current run.
    Opaque,
}

pub struct GilFusion;

impl GilFusion {
    fn operand_ty<'a>(func: &'a MirFunction, op: &Operand) -> Option<&'a OliveType> {
        match op {
            Operand::Copy(l) | Operand::Move(l) => Some(&func.locals[l.0].ty),
            _ => None,
        }
    }

    fn is_pyobj(func: &MirFunction, op: &Operand) -> bool {
        Self::operand_ty(func, op) == Some(&OliveType::PyObject)
    }

    fn classify_rvalue(func: &MirFunction, rvalue: &Rvalue) -> StmtClass {
        match rvalue {
            Rvalue::Use(_)
            | Rvalue::Ref(_)
            | Rvalue::MutRef(_)
            | Rvalue::GetTag(_)
            | Rvalue::GetTypeId(_)
            | Rvalue::VectorSplat(..)
            | Rvalue::VectorLoad(..)
            | Rvalue::VectorFMA(..)
            | Rvalue::PtrLoad(_)
            | Rvalue::FatPtrData(_)
            | Rvalue::VTableLoad { .. }
            | Rvalue::GenOf(_) => StmtClass::Neutral,
            Rvalue::Call { func: callee, .. } => match callee {
                Operand::Constant(Constant::Function(name))
                    if name.starts_with("__olive_py_")
                        && !EXCLUDED_PY_CALLS.contains(&name.as_str()) =>
                {
                    StmtClass::PyTouch
                }
                Operand::Constant(Constant::Function(name))
                    if SAFE_NON_PY_CALLS.contains(&name.as_str()) =>
                {
                    StmtClass::Neutral
                }
                _ => StmtClass::Opaque,
            },
            Rvalue::BinaryOp(_, l, r) => {
                if Self::is_pyobj(func, l) || Self::is_pyobj(func, r) {
                    StmtClass::PyTouch
                } else {
                    StmtClass::Neutral
                }
            }
            Rvalue::UnaryOp(_, o) => {
                if Self::is_pyobj(func, o) {
                    StmtClass::PyTouch
                } else {
                    StmtClass::Neutral
                }
            }
            Rvalue::GetAttr(o, _) | Rvalue::GetIndex(o, _, _) => {
                if Self::is_pyobj(func, o) {
                    StmtClass::PyTouch
                } else {
                    StmtClass::Neutral
                }
            }
            Rvalue::Cast(o, ty) => {
                if Self::is_pyobj(func, o) || *ty == OliveType::PyObject {
                    StmtClass::PyTouch
                } else {
                    StmtClass::Neutral
                }
            }
            Rvalue::Aggregate(_, ops) => {
                if ops.iter().any(|o| Self::is_pyobj(func, o)) {
                    StmtClass::Opaque
                } else {
                    StmtClass::Neutral
                }
            }
        }
    }

    fn classify(func: &MirFunction, stmt: &Statement) -> StmtClass {
        match &stmt.kind {
            StatementKind::Assign(_, rvalue) => Self::classify_rvalue(func, rvalue),
            StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => StmtClass::Neutral,
            StatementKind::Drop(local) => {
                let ty = &func.locals[local.0].ty;
                if *ty == OliveType::PyObject {
                    StmtClass::PyTouch
                } else if !ty.is_move_type() {
                    // No codegen is emitted at all for a non-move-type drop.
                    StmtClass::Neutral
                } else if matches!(ty, OliveType::Str | OliveType::Bytes) {
                    // Always routes to the runtime's own string/bytes free
                    // path (`__olive_free_str`/`__olive_buf_free`), never
                    // a struct's FFI destructor. A Python-backed bytes free
                    // decrefs under its own reentrant with_gil and exact
                    // PyBytes dealloc is C-only, so bridging stays safe.
                    StmtClass::Neutral
                } else {
                    // Every other move-type drop may hit a struct's custom
                    // FFI destructor, which is arbitrary foreign code.
                    StmtClass::Opaque
                }
            }
            StatementKind::SetAttr(obj, _, _) => {
                if Self::is_pyobj(func, obj) {
                    StmtClass::PyTouch
                } else {
                    StmtClass::Neutral
                }
            }
            StatementKind::SetIndex(obj, _, _, _) => {
                if Self::is_pyobj(func, obj) {
                    StmtClass::PyTouch
                } else {
                    StmtClass::Neutral
                }
            }
            StatementKind::VectorStore(..) | StatementKind::PtrStore(..) => StmtClass::Neutral,
            StatementKind::GenCheck { .. } => StmtClass::Neutral,
        }
    }

    /// Trimmed `(first, last)` index (inclusive) of every run in `bb` worth
    /// wrapping: a maximal `PyTouch`/`Neutral` stretch containing at least
    /// two `PyTouch` statements, narrowed to the span between its first and
    /// last `PyTouch` so no neutral padding sits inside the held region
    /// without needing to.
    fn find_regions(func: &MirFunction, classes: &[StmtClass]) -> Vec<(usize, usize)> {
        let mut regions = Vec::new();
        let mut i = 0;
        while i < classes.len() {
            if classes[i] == StmtClass::Opaque {
                i += 1;
                continue;
            }
            let seg_start = i;
            while i < classes.len() && classes[i] != StmtClass::Opaque {
                i += 1;
            }
            let py_positions: Vec<usize> = (seg_start..i)
                .filter(|&j| classes[j] == StmtClass::PyTouch)
                .collect();
            if py_positions.len() >= 2 {
                regions.push((py_positions[0], py_positions[py_positions.len() - 1]));
            }
        }
        let _ = func;
        regions
    }

    fn new_sink(func: &mut MirFunction) -> Local {
        let l = Local(func.locals.len());
        func.locals.push(LocalDecl {
            ty: crate::semantic::types::Type::Null,
            name: None,
            span: Span::default(),
            is_mut: false,
            is_owning: false,
        });
        l
    }

    fn gil_call(sink: Local, name: &str) -> Statement {
        Statement {
            kind: StatementKind::Assign(
                sink,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(name.to_string())),
                    args: vec![],
                },
            ),
            span: Span::default(),
        }
    }

    /// Retargets every occurrence of `from` in `bb`'s terminator to `to`.
    fn retarget(bb: &mut BasicBlock, from: BasicBlockId, to: BasicBlockId) {
        if let Some(term) = &mut bb.terminator {
            match &mut term.kind {
                TerminatorKind::Goto { target } => {
                    if *target == from {
                        *target = to;
                    }
                }
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

    fn block_successors(bb: &BasicBlock) -> Vec<BasicBlockId> {
        match &bb.terminator {
            Some(t) => match &t.kind {
                TerminatorKind::Goto { target } => vec![*target],
                TerminatorKind::SwitchInt {
                    targets, otherwise, ..
                } => {
                    let mut s: Vec<_> = targets.iter().map(|(_, b)| *b).collect();
                    s.push(*otherwise);
                    s
                }
                _ => vec![],
            },
            None => vec![],
        }
    }

    /// Every statement in the loop body is fusable and at least one really
    /// touches Python.
    fn loop_qualifies(func: &MirFunction, body: &FxHashSet<BasicBlockId>) -> bool {
        let mut has_py = false;
        for &bb in body {
            for stmt in &func.basic_blocks[bb.0].statements {
                match Self::classify(func, stmt) {
                    StmtClass::Opaque => return false,
                    StmtClass::PyTouch => has_py = true,
                    StmtClass::Neutral => {}
                }
            }
        }
        has_py
    }

    /// Wraps qualifying loops in one held GIL region each. Returns the union
    /// of wrapped bodies so block-local fusion skips them.
    fn fuse_loops(func: &mut MirFunction) -> FxHashSet<BasicBlockId> {
        let mut wrapped: FxHashSet<BasicBlockId> = FxHashSet::default();
        let loops = loop_utils::find_loops(func);
        if loops.is_empty() {
            return wrapped;
        }

        // One entry per header: `find_loops` reports one loop per back edge,
        // so `continue` statements produce several loops sharing a header.
        let mut merged: Vec<(BasicBlockId, FxHashSet<BasicBlockId>, Vec<BasicBlockId>)> =
            Vec::new();
        for l in loops {
            if let Some(entry) = merged.iter_mut().find(|(h, _, _)| *h == l.header) {
                entry.1.extend(l.body.iter().copied());
                entry.2.extend(l.latches.iter().copied());
            } else {
                merged.push((l.header, l.body.iter().copied().collect(), l.latches));
            }
        }

        // Outermost first; a loop overlapping an already-wrapped one (its
        // inner loops, in particular) stays unwrapped -- the outer region
        // already covers it.
        merged.sort_by_key(|(_, body, _)| std::cmp::Reverse(body.len()));

        for (header, body, latches) in merged {
            // A header at the function entry has no edge a preheader could
            // intercept.
            if header.0 == 0
                || body.iter().any(|b| wrapped.contains(b))
                || !Self::loop_qualifies(func, &body)
            {
                continue;
            }

            let span = Span::default();
            let pre = BasicBlockId(func.basic_blocks.len());
            let sink = Self::new_sink(func);
            func.basic_blocks.push(BasicBlock {
                statements: vec![Self::gil_call(sink, "__olive_py_gil_begin")],
                terminator: Some(Terminator {
                    kind: TerminatorKind::Goto { target: header },
                    span,
                }),
            });
            for i in 0..pre.0 {
                if !body.contains(&BasicBlockId(i)) {
                    Self::retarget(&mut func.basic_blocks[i], header, pre);
                }
            }

            for &b in &body {
                let outside: Vec<BasicBlockId> = Self::block_successors(&func.basic_blocks[b.0])
                    .into_iter()
                    .filter(|s| !body.contains(s))
                    .collect();
                for s in outside {
                    let tramp = BasicBlockId(func.basic_blocks.len());
                    let sink = Self::new_sink(func);
                    func.basic_blocks.push(BasicBlock {
                        statements: vec![Self::gil_call(sink, "__olive_py_gil_end")],
                        terminator: Some(Terminator {
                            kind: TerminatorKind::Goto { target: s },
                            span,
                        }),
                    });
                    Self::retarget(&mut func.basic_blocks[b.0], s, tramp);
                }
            }

            let latch_set: FxHashSet<BasicBlockId> = latches.into_iter().collect();
            for latch in latch_set {
                let sink = Self::new_sink(func);
                func.basic_blocks[latch.0]
                    .statements
                    .push(Self::gil_call(sink, "__olive_py_gil_checkpoint"));
            }

            wrapped.extend(body);
        }
        wrapped
    }

    fn fuse_block(func: &mut MirFunction, bb_idx: usize) -> bool {
        let classes: Vec<StmtClass> = func.basic_blocks[bb_idx]
            .statements
            .iter()
            .map(|s| Self::classify(func, s))
            .collect();

        let regions = Self::find_regions(func, &classes);
        if regions.is_empty() {
            return false;
        }

        let old_statements = std::mem::take(&mut func.basic_blocks[bb_idx].statements);
        let mut new_statements = Vec::with_capacity(old_statements.len() + regions.len() * 2);
        let mut regions = regions.into_iter().peekable();

        for (idx, stmt) in old_statements.into_iter().enumerate() {
            if let Some(&(start, _)) = regions.peek()
                && idx == start
            {
                let sink = Self::new_sink(func);
                new_statements.push(Self::gil_call(sink, "__olive_py_gil_begin"));
            }
            new_statements.push(stmt);
            if let Some(&(_, end)) = regions.peek()
                && idx == end
            {
                let sink = Self::new_sink(func);
                new_statements.push(Self::gil_call(sink, "__olive_py_gil_end"));
                regions.next();
            }
        }

        func.basic_blocks[bb_idx].statements = new_statements;
        true
    }
}

impl Transform for GilFusion {
    fn run(&self, func: &mut MirFunction) -> bool {
        let wrapped = Self::fuse_loops(func);
        let mut changed = !wrapped.is_empty();
        for bb_idx in 0..func.basic_blocks.len() {
            if wrapped.contains(&BasicBlockId(bb_idx)) {
                continue;
            }
            changed |= Self::fuse_block(func, bb_idx);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn stmt(kind: StatementKind) -> Statement {
        Statement {
            kind,
            span: Span::default(),
        }
    }

    fn py_call_stmt(dst: Local, name: &str) -> Statement {
        stmt(StatementKind::Assign(
            dst,
            Rvalue::Call {
                func: Operand::Constant(Constant::Function(name.to_string())),
                args: vec![],
            },
        ))
    }

    fn decl(ty: crate::semantic::types::Type) -> LocalDecl {
        LocalDecl {
            ty,
            name: None,
            span: Span::default(),
            is_mut: true,
            is_owning: false,
        }
    }

    fn count_gil_calls(func: &MirFunction, name: &str) -> usize {
        func.basic_blocks
            .iter()
            .flat_map(|bb| &bb.statements)
            .filter(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, Rvalue::Call { func: f, .. })
                        if matches!(f, Operand::Constant(Constant::Function(n)) if n == name)
                )
            })
            .count()
    }

    fn base_func(statements: Vec<Statement>) -> MirFunction {
        use crate::semantic::types::Type;
        MirFunction {
            name: "test_fn".to_string(),
            locals: vec![decl(Type::PyObject), decl(Type::PyObject)],
            basic_blocks: vec![BasicBlock {
                statements,
                terminator: Some(Terminator {
                    kind: TerminatorKind::Return,
                    span: Span::default(),
                }),
            }],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    #[test]
    fn fuses_two_consecutive_py_calls() {
        let mut func = base_func(vec![
            py_call_stmt(Local(0), "__olive_py_call0"),
            py_call_stmt(Local(1), "__olive_py_call0"),
        ]);
        let changed = GilFusion.run(&mut func);
        assert!(changed);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_begin"), 1);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_end"), 1);
        let stmts = &func.basic_blocks[0].statements;
        assert!(matches!(
            &stmts[0].kind,
            StatementKind::Assign(_, Rvalue::Call { func: f, .. })
                if matches!(f, Operand::Constant(Constant::Function(n)) if n == "__olive_py_gil_begin")
        ));
        assert!(matches!(
            &stmts[3].kind,
            StatementKind::Assign(_, Rvalue::Call { func: f, .. })
                if matches!(f, Operand::Constant(Constant::Function(n)) if n == "__olive_py_gil_end")
        ));
    }

    #[test]
    fn does_not_fuse_a_single_py_call() {
        let mut func = base_func(vec![py_call_stmt(Local(0), "__olive_py_call0")]);
        let changed = GilFusion.run(&mut func);
        assert!(!changed);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_begin"), 0);
    }

    #[test]
    fn bridges_neutral_statements_between_py_calls() {
        use crate::semantic::types::Type;
        let mut func = base_func(vec![
            py_call_stmt(Local(0), "__olive_py_call0"),
            stmt(StatementKind::StorageLive(Local(1))),
            py_call_stmt(Local(1), "__olive_py_call0"),
        ]);
        func.locals.push(decl(Type::PyObject));
        let changed = GilFusion.run(&mut func);
        assert!(changed);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_begin"), 1);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_end"), 1);
    }

    #[test]
    fn does_not_fuse_across_an_unrecognized_call() {
        let mut func = base_func(vec![
            py_call_stmt(Local(0), "__olive_py_call0"),
            py_call_stmt(Local(1), "__olive_thread_spawn"),
            py_call_stmt(Local(0), "__olive_py_call0"),
        ]);
        let changed = GilFusion.run(&mut func);
        assert!(!changed);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_begin"), 0);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_end"), 0);
    }

    #[test]
    fn never_spans_a_terminator() {
        let mut f1 = base_func(vec![py_call_stmt(Local(0), "__olive_py_call0")]);
        let mut f2 = base_func(vec![py_call_stmt(Local(0), "__olive_py_call0")]);
        f1.basic_blocks.push(f2.basic_blocks.pop().unwrap());
        let changed = GilFusion.run(&mut f1);
        assert!(!changed);
        assert_eq!(count_gil_calls(&f1, "__olive_py_gil_begin"), 0);
    }

    #[test]
    fn fuses_binop_chain_on_pyobject_operands() {
        use crate::parser::BinOp;
        let mut func = base_func(vec![
            stmt(StatementKind::Assign(
                Local(0),
                Rvalue::BinaryOp(BinOp::Add, Operand::Copy(Local(0)), Operand::Copy(Local(1))),
            )),
            stmt(StatementKind::Assign(
                Local(0),
                Rvalue::BinaryOp(BinOp::Add, Operand::Copy(Local(0)), Operand::Copy(Local(1))),
            )),
        ]);
        let changed = GilFusion.run(&mut func);
        assert!(changed);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_begin"), 1);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_end"), 1);
    }

    #[test]
    fn excludes_gil_and_lifecycle_calls_from_fusion() {
        let mut func = base_func(vec![
            py_call_stmt(Local(0), "__olive_py_initialize"),
            py_call_stmt(Local(0), "__olive_py_finalize"),
        ]);
        let changed = GilFusion.run(&mut func);
        assert!(!changed);
    }

    /// `block0 -> block1(header) -> block2(body, latch) -> block1`, exit to
    /// `block3`. Mirrors `loop_utils`'s own `loop_exits_detected` shape.
    fn loop_func(body_statements: Vec<Statement>) -> MirFunction {
        use crate::semantic::types::Type;
        let span = Span::default();
        let mut locals = vec![decl(Type::PyObject), decl(Type::PyObject)];
        locals.push(LocalDecl {
            ty: Type::Bool,
            name: None,
            span,
            is_mut: true,
            is_owning: false,
        });
        let cond = Local(2);
        MirFunction {
            name: "test_fn".to_string(),
            locals,
            basic_blocks: vec![
                BasicBlock {
                    statements: vec![],
                    terminator: Some(Terminator {
                        kind: TerminatorKind::Goto {
                            target: BasicBlockId(1),
                        },
                        span,
                    }),
                },
                BasicBlock {
                    statements: vec![],
                    terminator: Some(Terminator {
                        kind: TerminatorKind::SwitchInt {
                            discr: Operand::Copy(cond),
                            targets: vec![(1, BasicBlockId(2))],
                            otherwise: BasicBlockId(3),
                        },
                        span,
                    }),
                },
                BasicBlock {
                    statements: body_statements,
                    terminator: Some(Terminator {
                        kind: TerminatorKind::Goto {
                            target: BasicBlockId(1),
                        },
                        span,
                    }),
                },
                BasicBlock {
                    statements: vec![],
                    terminator: Some(Terminator {
                        kind: TerminatorKind::Return,
                        span,
                    }),
                },
            ],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    #[test]
    fn wraps_a_qualifying_loop_body_in_one_held_region() {
        let mut func = loop_func(vec![py_call_stmt(Local(0), "__olive_py_call0")]);
        let changed = GilFusion.run(&mut func);
        assert!(changed);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_begin"), 1);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_end"), 1);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_checkpoint"), 1);
    }

    #[test]
    fn does_not_wrap_a_loop_with_no_python_touch() {
        let mut func = loop_func(vec![stmt(StatementKind::StorageLive(Local(0)))]);
        let changed = GilFusion.run(&mut func);
        assert!(!changed);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_begin"), 0);
    }

    #[test]
    fn does_not_wrap_a_loop_containing_an_opaque_call() {
        let mut func = loop_func(vec![
            py_call_stmt(Local(0), "__olive_py_call0"),
            py_call_stmt(Local(1), "__olive_unrecognized_call"),
        ]);
        let changed = GilFusion.run(&mut func);
        assert!(!changed);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_begin"), 0);
    }

    #[test]
    fn treats_str_and_bytes_drops_inside_a_loop_as_bridgeable() {
        use crate::semantic::types::Type;
        let mut func = loop_func(vec![
            py_call_stmt(Local(0), "__olive_py_call0"),
            stmt(StatementKind::Drop(Local(3))),
        ]);
        func.locals.push(decl(Type::Str));
        let changed = GilFusion.run(&mut func);
        assert!(changed);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_begin"), 1);
    }

    #[test]
    fn does_not_wrap_a_loop_dropping_a_struct() {
        use crate::semantic::types::Type;
        let mut func = loop_func(vec![
            py_call_stmt(Local(0), "__olive_py_call0"),
            stmt(StatementKind::Drop(Local(3))),
        ]);
        func.locals
            .push(decl(Type::Struct("Foo".to_string(), vec![], false)));
        let changed = GilFusion.run(&mut func);
        assert!(!changed);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_begin"), 0);
    }

    #[test]
    fn treats_set_fault_loc_inside_a_loop_as_bridgeable() {
        let mut func = loop_func(vec![
            py_call_stmt(Local(0), "__olive_set_fault_loc"),
            py_call_stmt(Local(1), "__olive_py_call0"),
        ]);
        let changed = GilFusion.run(&mut func);
        assert!(changed);
        assert_eq!(count_gil_calls(&func, "__olive_py_gil_begin"), 1);
    }

    #[test]
    fn checkpoint_call_itself_does_not_disqualify_a_loop_on_a_second_pass() {
        // Running fusion twice must be idempotent -- the checkpoint/begin/end
        // calls a first pass inserts must not themselves look Opaque to a
        // second pass over the same function.
        let mut func = loop_func(vec![py_call_stmt(Local(0), "__olive_py_call0")]);
        assert!(GilFusion.run(&mut func));
        let changed_again = GilFusion.run(&mut func);
        assert!(!changed_again);
    }
}
