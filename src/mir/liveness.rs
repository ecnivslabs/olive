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
                live.remove(local); // def kills
                Self::use_rvalue(live, rvalue);
            }
            StatementKind::SetAttr(obj, _, val) => {
                Self::use_op(live, obj);
                Self::use_op(live, val);
            }
            StatementKind::SetIndex(obj, idx, val) => {
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
        }
    }

    fn use_rvalue(live: &mut HashSet<Local>, rv: &Rvalue) {
        match rv {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => Self::use_op(live, op),
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
            Rvalue::GetIndex(o, i) => {
                Self::use_op(live, o);
                Self::use_op(live, i);
            }
            Rvalue::GetTag(o) | Rvalue::GetTypeId(o) => Self::use_op(live, o),
            Rvalue::Ref(l) | Rvalue::MutRef(l) => {
                live.insert(*l);
            }
            Rvalue::PtrLoad(op) => Self::use_op(live, op),
            Rvalue::VectorSplat(op, _) => Self::use_op(live, op),
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
