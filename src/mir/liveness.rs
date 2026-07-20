use crate::mir::ir::*;
use rustc_hash::FxHashSet as HashSet;

pub struct Liveness {
    pub live_after: Vec<Vec<HashSet<Local>>>,
}

impl Liveness {
    pub fn compute(func: &MirFunction) -> Self {
        let mut live_after = Vec::new();
        for bb in &func.basic_blocks {
            live_after.push(vec![HashSet::default(); bb.statements.len() + 1]);
        }

        let mut changed = true;
        while changed {
            changed = false;

            for (bb_idx, bb) in func.basic_blocks.iter().enumerate().rev() {
                let mut current_live = HashSet::default();
                let succs = Self::successors(bb);
                for succ in &succs {
                    for &l in &live_after[*succ][0] {
                        current_live.insert(l);
                    }
                }

                Self::update_liveness(&mut current_live, bb.terminator.as_ref());

                if live_after[bb_idx][bb.statements.len()] != current_live {
                    live_after[bb_idx][bb.statements.len()] = current_live.clone();
                    changed = true;
                }

                for stmt_idx in (0..bb.statements.len()).rev() {
                    Self::update_stmt_liveness(&mut current_live, &bb.statements[stmt_idx]);
                    if live_after[bb_idx][stmt_idx] != current_live {
                        live_after[bb_idx][stmt_idx] = current_live.clone();
                        changed = true;
                    }
                }
            }
        }

        Self { live_after }
    }

    fn successors(bb: &BasicBlock) -> Vec<usize> {
        match &bb.terminator {
            Some(t) => match &t.kind {
                TerminatorKind::Goto { target } => vec![target.0],
                TerminatorKind::SwitchInt {
                    targets, otherwise, ..
                } => {
                    let mut s: Vec<_> = targets.iter().map(|(_, b)| b.0).collect();
                    s.push(otherwise.0);
                    s
                }
                TerminatorKind::Return | TerminatorKind::Unreachable => vec![],
            },
            None => vec![],
        }
    }

    fn update_liveness(live: &mut HashSet<Local>, term: Option<&Terminator>) {
        if let Some(t) = term
            && let TerminatorKind::SwitchInt { discr, .. } = &t.kind
        {
            Self::use_op(live, discr);
        }
    }

    fn update_stmt_liveness(live: &mut HashSet<Local>, stmt: &Statement) {
        match &stmt.kind {
            StatementKind::Assign(local, rvalue) => {
                live.remove(local);
                Self::use_rvalue(live, rvalue);
            }
            StatementKind::SetAttr(obj, _, val) => {
                Self::use_op(live, obj);
                Self::use_op(live, val);
            }
            StatementKind::SetIndex(obj, idx, val, _) => {
                Self::use_op(live, obj);
                Self::use_op(live, idx);
                Self::use_op(live, val);
            }
            StatementKind::Drop(local) | StatementKind::StorageDead(local) => {
                live.remove(local);
            }
            StatementKind::StorageLive(_) => {}
            StatementKind::VectorStore(obj, idx, val) => {
                Self::use_op(live, obj);
                Self::use_op(live, idx);
                Self::use_op(live, val);
            }
            StatementKind::PtrStore(ptr, val) => {
                Self::use_op(live, ptr);
                Self::use_op(live, val);
            }
            StatementKind::GenCheck { value, generation } => {
                live.insert(*value);
                live.insert(*generation);
            }
        }
    }

    fn use_rvalue(live: &mut HashSet<Local>, rv: &Rvalue) {
        match rv {
            Rvalue::Use(op)
            | Rvalue::UnaryOp(_, op)
            | Rvalue::FatPtrData(op)
            | Rvalue::GenOf(op) => Self::use_op(live, op),
            Rvalue::BinaryOp(_, l, r) => {
                Self::use_op(live, l);
                Self::use_op(live, r);
            }
            Rvalue::Call { func, args } => {
                Self::use_op(live, func);
                for a in args {
                    Self::use_op(live, a);
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for o in ops {
                    Self::use_op(live, o);
                }
            }
            Rvalue::GetAttr(o, _) => Self::use_op(live, o),
            Rvalue::GetIndex(o, i, _) => {
                Self::use_op(live, o);
                Self::use_op(live, i);
            }
            Rvalue::GetTag(o) | Rvalue::GetTypeId(o) | Rvalue::Cast(o, _) => Self::use_op(live, o),
            Rvalue::Ref(l) | Rvalue::MutRef(l) => {
                live.insert(*l);
            }
            Rvalue::PtrLoad(op) => Self::use_op(live, op),
            Rvalue::VTableLoad { vtable, .. } => Self::use_op(live, vtable),
            Rvalue::VectorSplat(op, _) | Rvalue::VectorReduce(_, op, _) => Self::use_op(live, op),
            Rvalue::VectorLoad(obj, idx, _) => {
                Self::use_op(live, obj);
                Self::use_op(live, idx);
            }
            Rvalue::VectorFMA(a, b, c) => {
                Self::use_op(live, a);
                Self::use_op(live, b);
                Self::use_op(live, c);
            }
        }
    }

    fn use_op(live: &mut HashSet<Local>, op: &Operand) {
        match op {
            Operand::Copy(l) | Operand::Move(l) => {
                live.insert(*l);
            }
            Operand::Constant(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::BinOp;
    use crate::semantic::types::Type;

    fn sp() -> crate::span::Span {
        crate::span::Span {
            file_id: 0,
            line: 0,
            col: 0,
            start: 0,
            end: 0,
        }
    }

    fn func(
        name: &str,
        locals: Vec<LocalDecl>,
        blocks: Vec<BasicBlock>,
        args: usize,
    ) -> MirFunction {
        MirFunction {
            name: name.into(),
            locals,
            basic_blocks: blocks,
            arg_count: args,
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

    fn local_decl() -> LocalDecl {
        LocalDecl {
            ty: Type::Int,
            name: None,
            span: sp(),
            is_mut: false,
            is_owning: false,
        }
    }

    #[test]
    fn empty_no_blocks() {
        let l = Liveness::compute(&func("f", vec![], vec![], 0));
        assert!(l.live_after.is_empty());
    }

    #[test]
    fn assign_use_makes_live() {
        let f = func(
            "f",
            vec![local_decl(), local_decl()],
            vec![bb(
                vec![
                    assign(0, Rvalue::Use(Operand::Copy(Local(1)))),
                    assign(1, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
                ],
                TerminatorKind::Return,
            )],
            1,
        );
        let l = Liveness::compute(&f);
        // Local(1) should be live before it's used, but not after it's assigned to
        assert!(l.live_after[0][0].contains(&Local(1)));
        assert!(
            !l.live_after[0][1].contains(&Local(1)) || !l.live_after[0][0].contains(&Local(1)),
            "local1 not live after its assign"
        );
    }

    #[test]
    fn binop_makes_both_live() {
        let f = func(
            "f",
            vec![local_decl(), local_decl(), local_decl()],
            vec![bb(
                vec![assign(
                    2,
                    Rvalue::BinaryOp(BinOp::Add, Operand::Copy(Local(0)), Operand::Copy(Local(1))),
                )],
                TerminatorKind::Return,
            )],
            2,
        );
        let l = Liveness::compute(&f);
        assert!(l.live_after[0][0].contains(&Local(0)));
        assert!(l.live_after[0][0].contains(&Local(1)));
    }

    #[test]
    fn call_makes_args_live() {
        let f = func(
            "f",
            vec![local_decl(), local_decl()],
            vec![bb(
                vec![assign(
                    0,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("g".into())),
                        args: vec![Operand::Copy(Local(1))],
                    },
                )],
                TerminatorKind::Return,
            )],
            1,
        );
        let l = Liveness::compute(&f);
        assert!(l.live_after[0][0].contains(&Local(1)));
    }

    #[test]
    fn switch_int_discr_live() {
        let f = func(
            "f",
            vec![local_decl()],
            vec![bb(
                vec![],
                TerminatorKind::SwitchInt {
                    discr: Operand::Copy(Local(0)),
                    targets: vec![],
                    otherwise: BasicBlockId(0),
                },
            )],
            1,
        );
        let l = Liveness::compute(&f);
        assert!(l.live_after[0][0].contains(&Local(0)));
    }

    #[test]
    fn goto_two_blocks() {
        let f = func(
            "f",
            vec![],
            vec![
                bb(
                    vec![],
                    TerminatorKind::Goto {
                        target: BasicBlockId(1),
                    },
                ),
                bb(vec![], TerminatorKind::Return),
            ],
            0,
        );
        let l = Liveness::compute(&f);
        assert_eq!(l.live_after.len(), 2);
    }

    #[test]
    fn drop_removes_from_live() {
        let f = func(
            "f",
            vec![local_decl()],
            vec![bb(
                vec![stmt(StatementKind::Drop(Local(0)))],
                TerminatorKind::Return,
            )],
            0,
        );
        let l = Liveness::compute(&f);
        assert!(!l.live_after[0][1].contains(&Local(0)));
    }

    #[test]
    fn storage_dead_removes() {
        let f = func(
            "f",
            vec![local_decl()],
            vec![bb(
                vec![
                    stmt(StatementKind::StorageLive(Local(0))),
                    stmt(StatementKind::StorageDead(Local(0))),
                ],
                TerminatorKind::Return,
            )],
            0,
        );
        let l = Liveness::compute(&f);
        assert!(!l.live_after[0][2].contains(&Local(0)));
    }

    #[test]
    fn get_attr_uses_obj() {
        let f = func(
            "f",
            vec![local_decl()],
            vec![bb(
                vec![assign(
                    0,
                    Rvalue::GetAttr(Operand::Copy(Local(0)), "x".into()),
                )],
                TerminatorKind::Return,
            )],
            1,
        );
        let l = Liveness::compute(&f);
        assert!(l.live_after[0][0].contains(&Local(0)));
    }

    #[test]
    fn ref_makes_local_live() {
        let f = func(
            "f",
            vec![local_decl()],
            vec![bb(
                vec![assign(0, Rvalue::Ref(Local(0)))],
                TerminatorKind::Return,
            )],
            1,
        );
        let l = Liveness::compute(&f);
        assert!(l.live_after[0][0].contains(&Local(0)));
    }
}
