use super::Transform;
use crate::mir::*;
use std::collections::HashSet;

pub struct DeadCodeElimination;

impl Transform for DeadCodeElimination {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut used = HashSet::new();
        used.insert(Local(0));

        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                match &stmt.kind {
                    StatementKind::Assign(_, rval) => self.record_rvalue_usage(rval, &mut used),
                    StatementKind::SetAttr(obj, _, val) => {
                        self.record_operand_usage(obj, &mut used);
                        self.record_operand_usage(val, &mut used);
                    }
                    StatementKind::SetIndex(obj, idx, val, _) => {
                        self.record_operand_usage(obj, &mut used);
                        self.record_operand_usage(idx, &mut used);
                        self.record_operand_usage(val, &mut used);
                    }
                    StatementKind::VectorStore(obj, idx, val) => {
                        self.record_operand_usage(obj, &mut used);
                        self.record_operand_usage(idx, &mut used);
                        self.record_operand_usage(val, &mut used);
                    }
                    StatementKind::PtrStore(ptr, val) => {
                        self.record_operand_usage(ptr, &mut used);
                        self.record_operand_usage(val, &mut used);
                    }
                    // A drop reads the owned pointer; the assignment feeding
                    // an owner must not be eliminated out from under it.
                    StatementKind::Drop(l) => {
                        if func.locals.get(l.0).is_some_and(|d| d.ty.is_move_type()) {
                            used.insert(*l);
                        }
                    }
                    StatementKind::GenCheck { value, generation } => {
                        used.insert(*value);
                        used.insert(*generation);
                    }
                    _ => {}
                }
            }
            if let Some(term) = &bb.terminator
                && let TerminatorKind::SwitchInt { discr, .. } = &term.kind
            {
                self.record_operand_usage(discr, &mut used)
            }
        }

        let mut changed = false;
        for bb in &mut func.basic_blocks {
            let old_len = bb.statements.len();
            bb.statements.retain(|stmt| {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    if matches!(rval, Rvalue::Call { .. }) {
                        return true;
                    }
                    used.contains(dest)
                } else {
                    true
                }
            });
            if bb.statements.len() != old_len {
                changed = true;
            }
        }
        changed
    }
}

impl DeadCodeElimination {
    fn record_rvalue_usage(&self, rval: &Rvalue, used: &mut HashSet<Local>) {
        match rval {
            Rvalue::Use(op)
            | Rvalue::UnaryOp(_, op)
            | Rvalue::GetAttr(op, _)
            | Rvalue::GetTag(op)
            | Rvalue::GetTypeId(op)
            | Rvalue::FatPtrData(op)
            | Rvalue::Cast(op, _) => self.record_operand_usage(op, used),
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r, _) => {
                self.record_operand_usage(l, used);
                self.record_operand_usage(r, used);
            }
            Rvalue::Call { func, args } => {
                self.record_operand_usage(func, used);
                for arg in args {
                    self.record_operand_usage(arg, used);
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for op in ops {
                    self.record_operand_usage(op, used);
                }
            }
            Rvalue::Ref(l) | Rvalue::MutRef(l) => {
                used.insert(*l);
            }
            Rvalue::PtrLoad(op) | Rvalue::GenOf(op) => self.record_operand_usage(op, used),
            Rvalue::VTableLoad { vtable, .. } => self.record_operand_usage(vtable, used),
            Rvalue::VectorSplat(op, _) => self.record_operand_usage(op, used),
            Rvalue::VectorLoad(obj, idx, _) => {
                self.record_operand_usage(obj, used);
                self.record_operand_usage(idx, used);
            }
            Rvalue::VectorFMA(a, b, c) => {
                self.record_operand_usage(a, used);
                self.record_operand_usage(b, used);
                self.record_operand_usage(c, used);
            }
        }
    }

    fn record_operand_usage(&self, op: &Operand, used: &mut HashSet<Local>) {
        if let Operand::Copy(l) | Operand::Move(l) = op {
            used.insert(*l);
        }
    }
}

#[cfg(test)]
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
    fn keep_used_assign() {
        let mut f = func(vec![block(
            vec![assign(0, Rvalue::Use(Operand::Constant(Constant::Int(1))))],
            TerminatorKind::Return,
        )]);
        assert!(!DeadCodeElimination.run(&mut f));
        assert_eq!(f.basic_blocks[0].statements.len(), 1);
    }

    #[test]
    fn remove_unused_assign() {
        let mut f = func(vec![block(
            vec![
                assign(0, Rvalue::Use(Operand::Constant(Constant::Int(1)))),
                assign(1, Rvalue::Use(Operand::Constant(Constant::Int(2)))),
            ],
            TerminatorKind::Return,
        )]);
        let changed = DeadCodeElimination.run(&mut f);
        assert!(changed);
        assert_eq!(f.basic_blocks[0].statements.len(), 1);
    }

    #[test]
    fn keep_unused_with_storage_live() {
        let mut f = func(vec![block(
            vec![
                Statement {
                    kind: StatementKind::StorageLive(Local(0)),
                    span: sp(),
                },
                assign(0, Rvalue::Use(Operand::Constant(Constant::Int(1)))),
                Statement {
                    kind: StatementKind::StorageDead(Local(0)),
                    span: sp(),
                },
            ],
            TerminatorKind::Return,
        )]);
        DeadCodeElimination.run(&mut f);
        assert_eq!(f.basic_blocks[0].statements.len(), 3);
    }

    #[test]
    fn no_dead_code_empty() {
        let mut f = func(vec![]);
        assert!(!DeadCodeElimination.run(&mut f));
    }

    #[test]
    fn drop_keeps_owner_assignment_alive() {
        let mut f = func(vec![block(
            vec![
                assign(
                    1,
                    Rvalue::Aggregate(
                        crate::mir::ir::AggregateKind::List,
                        vec![Operand::Constant(Constant::Int(1))],
                    ),
                ),
                Statement {
                    kind: StatementKind::Drop(Local(1)),
                    span: sp(),
                },
            ],
            TerminatorKind::Return,
        )]);
        f.locals = vec![
            LocalDecl {
                ty: crate::semantic::types::Type::Int,
                name: None,
                span: sp(),
                is_mut: false,
                is_owning: true,
            },
            LocalDecl {
                ty: crate::semantic::types::Type::List(Box::new(crate::semantic::types::Type::Int)),
                name: None,
                span: sp(),
                is_mut: false,
                is_owning: true,
            },
        ];
        assert!(!DeadCodeElimination.run(&mut f));
        assert_eq!(f.basic_blocks[0].statements.len(), 2);
    }
}
