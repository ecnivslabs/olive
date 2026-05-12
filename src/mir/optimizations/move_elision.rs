use super::Transform;
use crate::mir::liveness::Liveness;
use crate::mir::*;

// elide copies if local not live after stmt
pub struct MoveElision;

impl Transform for MoveElision {
    fn name(&self) -> &'static str {
        "move_elision"
    }

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
            StatementKind::Assign(_, rval) => self.optimize_rvalue(rval, live_after, locals),
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
            | Rvalue::GetTag(op) => self.optimize_operand(op, live_after, locals),
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
                let mut changed = self.optimize_operand(l, live_after, locals);
                changed |= self.optimize_operand(r, live_after, locals);
                changed
            }
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
        if let Operand::Copy(local) = op {
            if !live_after.contains(local) {
                let ty = &locals[local.0].ty;
                if ty.is_move_type() {
                    if !live_after.contains(local) {
                        *op = Operand::Move(*local);
                        return true;
                    }
                }
            }
        }
        false
    }
}
