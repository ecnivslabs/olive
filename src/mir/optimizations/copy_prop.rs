use super::Transform;
use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

pub struct CopyPropagation;

impl Transform for CopyPropagation {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut assign_counts: HashMap<Local, usize> = HashMap::default();
        let mut copy_assignments: HashMap<Local, Local> = HashMap::default();

        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    *assign_counts.entry(*dest).or_insert(0) += 1;
                    if let Rvalue::Use(Operand::Copy(src)) = rval {
                        copy_assignments.insert(*dest, *src);
                    }
                }
            }
        }

        // Definition count. A parameter has an implicit entry binding on top of
        // its assigns, so a reassigned `mut` param has 2+ defs and is not a
        // stable copy source for this flow-insensitive pass.
        let def_count = |l: Local| -> usize {
            let base = *assign_counts.get(&l).unwrap_or(&0);
            if l.0 >= 1 && l.0 <= func.arg_count {
                base + 1
            } else {
                base
            }
        };

        // An owning move-type value copied into another local is a shallow
        // alias, not a real duplicate (no refcount bump) - dest and src are
        // both live handles to the same object. Forwarding such a copy lets
        // MoveElision independently "move" (consume) each alias in this same
        // fixpoint loop before the chain is fully collapsed, so two uses of
        // what is really one object end up anchored to different locals and
        // each looks like a single, unrelated move to the borrow checker -
        // which then sees what is really a double-move as if it were sound.
        let is_owning_move_local = |l: Local| -> bool {
            func.locals
                .get(l.0)
                .is_some_and(|decl| decl.ty.is_move_type() && decl.is_owning)
        };

        let mut safe_copies: HashMap<Local, Local> = HashMap::default();
        for (dest, src) in copy_assignments {
            if def_count(dest) == 1 && def_count(src) <= 1 && !is_owning_move_local(src) {
                safe_copies.insert(dest, src);
            }
        }

        if safe_copies.is_empty() {
            return false;
        }

        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                match &mut stmt.kind {
                    StatementKind::Assign(_, rval) => {
                        changed |= self.propagate_copies_in_rvalue(rval, &safe_copies);
                    }
                    StatementKind::SetIndex(obj, idx, val, _) => {
                        changed |= self.propagate_copies_in_operand(obj, &safe_copies);
                        changed |= self.propagate_copies_in_operand(idx, &safe_copies);
                        changed |= self.propagate_copies_in_operand(val, &safe_copies);
                    }
                    StatementKind::SetAttr(obj, _, val) => {
                        changed |= self.propagate_copies_in_operand(obj, &safe_copies);
                        changed |= self.propagate_copies_in_operand(val, &safe_copies);
                    }
                    StatementKind::VectorStore(obj, idx, val) => {
                        changed |= self.propagate_copies_in_operand(obj, &safe_copies);
                        changed |= self.propagate_copies_in_operand(idx, &safe_copies);
                        changed |= self.propagate_copies_in_operand(val, &safe_copies);
                    }
                    _ => {}
                }
            }
            if let Some(term) = &mut bb.terminator
                && let TerminatorKind::SwitchInt { discr, .. } = &mut term.kind
            {
                changed |= self.propagate_copies_in_operand(discr, &safe_copies);
            }
        }
        changed
    }
}

impl CopyPropagation {
    fn propagate_copies_in_rvalue(&self, rval: &mut Rvalue, map: &HashMap<Local, Local>) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => {
                self.propagate_copies_in_operand(op, map)
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r, _) => {
                let mut changed = self.propagate_copies_in_operand(l, map);
                changed |= self.propagate_copies_in_operand(r, map);
                changed
            }
            Rvalue::Call { func, args } => {
                let mut changed = self.propagate_copies_in_operand(func, map);
                for arg in args {
                    changed |= self.propagate_copies_in_operand(arg, map);
                }
                changed
            }
            Rvalue::Aggregate(_, ops) => {
                let mut changed = false;
                for op in ops {
                    changed |= self.propagate_copies_in_operand(op, map);
                }
                changed
            }
            Rvalue::PtrLoad(op) => self.propagate_copies_in_operand(op, map),
            Rvalue::VectorSplat(op, _) | Rvalue::VectorReduce(_, op, _) => {
                self.propagate_copies_in_operand(op, map)
            }
            Rvalue::VectorLoad(obj, idx, _) => {
                let mut changed = self.propagate_copies_in_operand(obj, map);
                changed |= self.propagate_copies_in_operand(idx, map);
                changed
            }
            Rvalue::VectorFMA(a, b, c) => {
                let mut changed = self.propagate_copies_in_operand(a, map);
                changed |= self.propagate_copies_in_operand(b, map);
                changed |= self.propagate_copies_in_operand(c, map);
                changed
            }
            _ => false,
        }
    }

    fn propagate_copies_in_operand(&self, op: &mut Operand, map: &HashMap<Local, Local>) -> bool {
        if let Operand::Copy(l) = op
            && let Some(new_l) = map.get(l)
        {
            *op = Operand::Copy(*new_l);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::BinOp;

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

    fn block(stmts: Vec<Statement>, term: TerminatorKind) -> BasicBlock {
        BasicBlock {
            statements: stmts,
            terminator: Some(Terminator {
                kind: term,
                span: sp(),
            }),
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

    #[test]
    fn copy_prop_eliminates_copy() {
        let mut f = func(vec![block(
            vec![
                assign(1, Rvalue::Use(Operand::Copy(Local(0)))),
                assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
            ],
            TerminatorKind::Return,
        )]);
        CopyPropagation.run(&mut f);
        if let StatementKind::Assign(_, Rvalue::Use(Operand::Copy(l))) =
            &f.basic_blocks[0].statements[1].kind
        {
            assert_eq!(*l, Local(0), "should propagate through copy chain");
        }
    }

    #[test]
    fn copy_prop_no_change() {
        let mut f = func(vec![block(
            vec![assign(
                0,
                Rvalue::BinaryOp(BinOp::Add, Operand::Copy(Local(1)), Operand::Copy(Local(2))),
            )],
            TerminatorKind::Return,
        )]);
        assert!(!CopyPropagation.run(&mut f));
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
    fn copy_prop_skips_owning_move_type_source() {
        // `dest = Copy(owning_src)` is a shallow alias, not a real duplicate.
        // Forwarding it would let two uses of the same object end up
        // anchored to different locals, each independently movable.
        let mut f = func(vec![block(
            vec![
                assign(1, Rvalue::Use(Operand::Copy(Local(0)))),
                assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
            ],
            TerminatorKind::Return,
        )]);
        f.locals = vec![move_type_local(), move_type_local(), move_type_local()];
        CopyPropagation.run(&mut f);
        if let StatementKind::Assign(_, Rvalue::Use(Operand::Copy(l))) =
            &f.basic_blocks[0].statements[1].kind
        {
            assert_eq!(*l, Local(1), "owning move-type alias must not be forwarded");
        } else {
            panic!("expected copy assignment");
        }
    }

    #[test]
    fn copy_prop_skips_reassigned_param_source() {
        // `tmp = Copy(param)` then a param reassignment: param isn't stable, so
        // tmp must not be propagated to it.
        let mut f = func(vec![block(
            vec![
                assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
                assign(
                    1,
                    Rvalue::BinaryOp(
                        BinOp::Add,
                        Operand::Copy(Local(1)),
                        Operand::Constant(Constant::Int(0)),
                    ),
                ),
                assign(0, Rvalue::Use(Operand::Copy(Local(2)))),
            ],
            TerminatorKind::Return,
        )]);
        f.arg_count = 1;
        CopyPropagation.run(&mut f);
        // The read of Local(2) must stay Local(2), not the reassigned Local(1).
        if let StatementKind::Assign(_, Rvalue::Use(Operand::Copy(l))) =
            &f.basic_blocks[0].statements[2].kind
        {
            assert_eq!(*l, Local(2), "reassigned param must not be propagated");
        } else {
            panic!("expected copy assignment");
        }
    }
}
