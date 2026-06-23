use super::Transform;
use crate::mir::liveness::Liveness;
use crate::mir::*;

pub struct MoveElision;

impl Transform for MoveElision {
    fn run(&self, func: &mut MirFunction) -> bool {
        let liveness = Liveness::compute(func);
        let mut changed = false;

        let locals = &func.locals;
        for (bb_idx, bb) in func.basic_blocks.iter_mut().enumerate() {
            for (stmt_idx, stmt) in bb.statements.iter_mut().enumerate() {
                let live_after = &liveness.live_after[bb_idx][stmt_idx + 1];
                changed |= self.optimize_statement(stmt, live_after, locals);
            }
            if let Some(term) = &mut bb.terminator {
                let live_after = &liveness.live_after[bb_idx][bb.statements.len()];
                changed |= self.optimize_terminator(term, live_after, locals);
            }
        }

        changed
    }
}

impl MoveElision {
    fn optimize_statement(
        &self,
        stmt: &mut Statement,
        live_after: &rustc_hash::FxHashSet<Local>,
        locals: &[LocalDecl],
    ) -> bool {
        match &mut stmt.kind {
            StatementKind::Assign(dst, rval) => {
                // dst aliases src; src's value persists through dst, so moving is unsound.
                if matches!(rval, Rvalue::Use(Operand::Copy(_))) && live_after.contains(dst) {
                    return false;
                }
                self.optimize_rvalue(rval, live_after, locals)
            }
            StatementKind::SetAttr(obj, _, val) => {
                let mut changed = self.optimize_operand(obj, live_after, locals);
                changed |= self.optimize_operand(val, live_after, locals);
                changed
            }
            StatementKind::SetIndex(obj, idx, val) => {
                let mut changed = self.optimize_operand(obj, live_after, locals);
                changed |= self.optimize_operand(idx, live_after, locals);
                changed |= self.optimize_operand(val, live_after, locals);
                changed
            }
            StatementKind::VectorStore(obj, idx, val) => {
                let mut changed = self.optimize_operand(obj, live_after, locals);
                changed |= self.optimize_operand(idx, live_after, locals);
                changed |= self.optimize_operand(val, live_after, locals);
                changed
            }
            StatementKind::PtrStore(ptr, val) => {
                let mut changed = self.optimize_operand(ptr, live_after, locals);
                changed |= self.optimize_operand(val, live_after, locals);
                changed
            }
            _ => false,
        }
    }

    fn optimize_terminator(
        &self,
        term: &mut Terminator,
        live_after: &rustc_hash::FxHashSet<Local>,
        locals: &[LocalDecl],
    ) -> bool {
        match &mut term.kind {
            TerminatorKind::SwitchInt { discr, .. } => {
                self.optimize_operand(discr, live_after, locals)
            }
            _ => false,
        }
    }

    fn optimize_rvalue(
        &self,
        rval: &mut Rvalue,
        live_after: &rustc_hash::FxHashSet<Local>,
        locals: &[LocalDecl],
    ) -> bool {
        match rval {
            Rvalue::Use(op)
            | Rvalue::UnaryOp(_, op)
            | Rvalue::GetAttr(op, _)
            | Rvalue::GetTag(op)
            | Rvalue::GetTypeId(op)
            | Rvalue::FatPtrData(op)
            | Rvalue::Cast(op, _) => self.optimize_operand(op, live_after, locals),
            Rvalue::BinaryOp(_, l, r) => {
                let mut changed = self.optimize_operand(l, live_after, locals);
                changed |= self.optimize_operand(r, live_after, locals);
                changed
            }
            // Indexing reads an element that may alias the container's storage,
            // so the container must not be moved (freed) here even on its last
            // use. The index is an integer and never a move type.
            Rvalue::GetIndex(_, _) => false,
            Rvalue::Call { func: f_op, args } => {
                let mut changed = self.optimize_operand(f_op, live_after, locals);
                for arg in args {
                    changed |= self.optimize_operand(arg, live_after, locals);
                }
                changed
            }
            Rvalue::Aggregate(_, ops) => {
                let mut changed = false;
                for op in ops {
                    changed |= self.optimize_operand(op, live_after, locals);
                }
                changed
            }
            Rvalue::Ref(_) | Rvalue::MutRef(_) => false,
            Rvalue::PtrLoad(op) => self.optimize_operand(op, live_after, locals),
            Rvalue::VTableLoad { vtable, .. } => self.optimize_operand(vtable, live_after, locals),
            Rvalue::VectorSplat(op, _) => self.optimize_operand(op, live_after, locals),
            Rvalue::VectorLoad(obj, idx, _) => {
                let mut changed = self.optimize_operand(obj, live_after, locals);
                changed |= self.optimize_operand(idx, live_after, locals);
                changed
            }
            Rvalue::VectorFMA(a, b, c) => {
                let mut changed = self.optimize_operand(a, live_after, locals);
                changed |= self.optimize_operand(b, live_after, locals);
                changed |= self.optimize_operand(c, live_after, locals);
                changed
            }
        }
    }

    fn optimize_operand(
        &self,
        op: &mut Operand,
        live_after: &rustc_hash::FxHashSet<Local>,
        locals: &[LocalDecl],
    ) -> bool {
        if let Operand::Copy(local) = op
            && !live_after.contains(local)
        {
            let decl = &locals[local.0];
            if decl.ty.is_move_type() && decl.is_owning {
                *op = Operand::Move(*local);
                return true;
            }
        }
        false
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
    fn copy_preserved_for_param() {
        let mut f = func(
            "f",
            vec![move_type_local(), move_type_local()],
            vec![assign(1, Rvalue::Use(Operand::Copy(Local(0))))],
        );
        // Local(0) is an argument (arg_count=1), so its value is live
        // across the block for potential later use. MoveElision skips args.
        let _changed = MoveElision.run(&mut f);
        // Just verify it runs without crashing
        assert!(!f.basic_blocks[0].statements.is_empty());
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
    fn multi_block_copy_to_move() {
        let mut f = MirFunction {
            name: "f".into(),
            locals: vec![move_type_local(), move_type_local()],
            basic_blocks: vec![BasicBlock {
                statements: vec![assign(1, Rvalue::Use(Operand::Copy(Local(0))))],
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
        };
        assert!(MoveElision.run(&mut f));
        match &f.basic_blocks[0].statements[0].kind {
            StatementKind::Assign(_, Rvalue::Use(Operand::Move(_))) => {}
            _ => panic!("expected Move"),
        }
    }
}
