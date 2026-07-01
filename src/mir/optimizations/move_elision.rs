use super::Transform;
use crate::mir::liveness::Liveness;
use crate::mir::*;

/// Upgrades the last use of an owning heap local from `Copy` to `Move` so the
/// value transfers instead of waiting for the scope-end drop.
///
/// A `Move` nulls the source variable, which turns its later `Drop` into a
/// no-op. That is only sound where the destination takes over responsibility
/// for freeing: an owning local, a container element, a stored field, or a
/// global. Read-only positions (operands of binops, call arguments, switch
/// discriminants, the container side of an indexed store) must keep `Copy`,
/// otherwise the value is nulled with no owner left and leaks.
///
/// Promoting to `Move` without removing the stale `Drop` would double-free.
pub struct MoveElision;

impl Transform for MoveElision {
    fn run(&self, func: &mut MirFunction) -> bool {
        let liveness = Liveness::compute(func);
        let mut moved: Vec<(usize, usize, Local)> = Vec::new();

        let locals = &func.locals;
        for (bb_idx, bb) in func.basic_blocks.iter_mut().enumerate() {
            for (stmt_idx, stmt) in bb.statements.iter_mut().enumerate() {
                let live_after = &liveness.live_after[bb_idx][stmt_idx + 1];
                self.optimize_statement(bb_idx, stmt_idx, stmt, live_after, locals, &mut moved);
            }
        }

        if moved.is_empty() {
            return false;
        }

        let mut drop_removals: Vec<(usize, usize)> = Vec::new();
        for (bb, idx, src) in &moved {
            let stmts = &func.basic_blocks[*bb].statements;
            for (j, stmt) in stmts.iter().enumerate().skip(idx + 1) {
                match &stmt.kind {
                    StatementKind::Drop(l) if l == src => {
                        drop_removals.push((*bb, j));
                        break;
                    }
                    StatementKind::Assign(d, _) if d == src => break,
                    _ => {}
                }
            }
        }
        drop_removals.sort_unstable_by(|a, b| b.cmp(a));
        for (bb, idx) in drop_removals {
            func.basic_blocks[bb].statements.remove(idx);
        }

        true
    }
}

impl MoveElision {
    #[allow(clippy::too_many_arguments)]
    fn optimize_statement(
        &self,
        bb_idx: usize,
        stmt_idx: usize,
        stmt: &mut Statement,
        live_after: &rustc_hash::FxHashSet<Local>,
        locals: &[LocalDecl],
        moved: &mut Vec<(usize, usize, Local)>,
    ) {
        match &mut stmt.kind {
            StatementKind::Assign(dst, rval) => {
                // The destination must be able to free the value it receives.
                let dst_owns = locals
                    .get(dst.0)
                    .is_some_and(|d| d.is_owning && d.ty.is_move_type());
                match rval {
                    Rvalue::Use(op) => {
                        if !dst_owns {
                            return;
                        }
                        // dst aliases src; src's value persists through dst, so
                        // moving is unsound while dst stays live.
                        if matches!(op, Operand::Copy(_)) && live_after.contains(dst) {
                            return;
                        }
                        self.optimize_operand(bb_idx, stmt_idx, op, live_after, locals, moved);
                    }
                    Rvalue::Aggregate(_, ops) => {
                        for op in ops {
                            self.optimize_operand(bb_idx, stmt_idx, op, live_after, locals, moved);
                        }
                    }
                    // `__olive_list_push` hands its element to the list, the
                    // same transfer a list literal's element makes. Everything
                    // else in call position is read-only.
                    Rvalue::Call { func, args } => {
                        let is_push = matches!(func, Operand::Constant(Constant::Function(n))
                            if n == "__olive_list_push");
                        if is_push && let Some(op) = args.get_mut(1) {
                            self.optimize_operand(bb_idx, stmt_idx, op, live_after, locals, moved);
                        }
                    }
                    _ => {}
                }
            }
            StatementKind::SetAttr(_, _, val) => {
                self.optimize_operand(bb_idx, stmt_idx, val, live_after, locals, moved);
            }
            StatementKind::SetIndex(_, _, val, _) => {
                self.optimize_operand(bb_idx, stmt_idx, val, live_after, locals, moved);
            }
            StatementKind::PtrStore(_, val) => {
                self.optimize_operand(bb_idx, stmt_idx, val, live_after, locals, moved);
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn optimize_operand(
        &self,
        bb_idx: usize,
        stmt_idx: usize,
        op: &mut Operand,
        live_after: &rustc_hash::FxHashSet<Local>,
        locals: &[LocalDecl],
        moved: &mut Vec<(usize, usize, Local)>,
    ) {
        if let Operand::Copy(local) = *op
            && !live_after.contains(&local)
        {
            let decl = &locals[local.0];
            if decl.ty.is_move_type() && decl.is_owning {
                *op = Operand::Move(local);
                moved.push((bb_idx, stmt_idx, local));
            }
        }
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

    fn func(name: &str, locals: Vec<LocalDecl>, stmts: Vec<Statement>) -> MirFunction {
        MirFunction {
            name: name.into(),
            locals,
            basic_blocks: vec![BasicBlock {
                statements: stmts,
                terminator: Some(Terminator {
                    kind: TerminatorKind::Return,
                    span: sp(),
                }),
            }],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    fn move_type_local() -> LocalDecl {
        LocalDecl {
            ty: crate::semantic::types::Type::Tuple(vec![crate::semantic::types::Type::Int]),
            name: None,
            span: sp(),
            is_mut: false,
            is_owning: true,
        }
    }

    #[test]
    fn copy_to_move_dead_local() {
        let mut f = func(
            "f",
            vec![move_type_local()],
            vec![assign(0, Rvalue::Use(Operand::Copy(Local(0))))],
        );
        assert!(MoveElision.run(&mut f));
        match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Use(Operand::Move(l))) if *l == Local(0) => {}
            _ => panic!("expected Move(Local(0))"),
        }
    }

    #[test]
    fn int_type_not_moved() {
        let mut f = func(
            "f",
            vec![LocalDecl {
                ty: crate::semantic::types::Type::Int,
                name: None,
                span: sp(),
                is_mut: false,
                is_owning: true,
            }],
            vec![assign(0, Rvalue::Use(Operand::Copy(Local(0))))],
        );
        // Int is copy type, not move type -> should stay Copy
        assert!(!MoveElision.run(&mut f));
    }

    #[test]
    fn call_args_never_moved() {
        let mut f = func(
            "f",
            vec![move_type_local(), move_type_local()],
            vec![assign(
                1,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("g".into())),
                    args: vec![Operand::Copy(Local(0))],
                },
            )],
        );
        // Moving an argument nulls the caller's variable with no new owner.
        assert!(!MoveElision.run(&mut f));
        match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Call { args, .. }) => {
                assert!(matches!(args[0], Operand::Copy(_)));
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn view_destination_keeps_copy() {
        let mut view_dst = move_type_local();
        view_dst.is_owning = false;
        let mut f = func(
            "f",
            vec![move_type_local(), view_dst],
            vec![assign(1, Rvalue::Use(Operand::Copy(Local(0))))],
        );
        // A non-owning destination will never free the value; moving into it
        // would leave the value ownerless.
        assert!(!MoveElision.run(&mut f));
    }

    #[test]
    fn aggregate_elements_moved() {
        let mut f = func(
            "f",
            vec![move_type_local(), move_type_local()],
            vec![assign(
                1,
                Rvalue::Aggregate(AggregateKind::List, vec![Operand::Copy(Local(0))]),
            )],
        );
        assert!(MoveElision.run(&mut f));
        match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Aggregate(_, ops)) => {
                assert!(matches!(ops[0], Operand::Move(_)));
            }
            _ => panic!("expected aggregate"),
        }
    }
}
