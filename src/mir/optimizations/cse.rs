use super::Transform;
use crate::mir::*;

pub struct CommonSubexpressionElimination;

impl Transform for CommonSubexpressionElimination {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            let mut available_expressions: Vec<(Rvalue, Local)> = Vec::new();
            for stmt in &mut bb.statements {
                let moved = Self::moved_locals(&stmt.kind);

                match &mut stmt.kind {
                    StatementKind::Assign(dest, rval) => {
                        if let Rvalue::Call { .. } = rval {
                            available_expressions.clear();
                        } else {
                            // Expressions reading a moved-from local are not repeatable:
                            // a move zeroes its source at runtime.
                            let is_candidate = matches!(
                                rval,
                                Rvalue::BinaryOp(..) | Rvalue::UnaryOp(..) | Rvalue::Use(..)
                            ) && !Self::rvalue_has_move(rval);

                            if is_candidate {
                                let mut found = None;
                                for (expr, local) in &available_expressions {
                                    if expr == rval {
                                        found = Some(*local);
                                        break;
                                    }
                                }

                                if let Some(existing_local) = found {
                                    let new_rval = Rvalue::Use(Operand::Copy(existing_local));
                                    if *rval != new_rval {
                                        *rval = new_rval;
                                        changed = true;
                                    }
                                } else {
                                    available_expressions.push((rval.clone(), *dest));
                                }
                            }

                            let dest = *dest;
                            available_expressions.retain(|(expr, local)| {
                                *local != dest && !Self::uses_local(expr, dest)
                            });
                        }
                    }
                    StatementKind::SetIndex(..)
                    | StatementKind::SetAttr(..)
                    | StatementKind::VectorStore(..)
                    | StatementKind::PtrStore(..) => {
                        available_expressions.retain(|(expr, _)| {
                            !matches!(expr, Rvalue::GetIndex(..) | Rvalue::GetAttr(..))
                        });
                    }
                    StatementKind::Drop(l) | StatementKind::StorageDead(l) => {
                        let l = *l;
                        available_expressions
                            .retain(|(expr, local)| *local != l && !Self::uses_local(expr, l));
                    }
                    _ => {}
                }

                for m in moved {
                    available_expressions
                        .retain(|(expr, local)| *local != m && !Self::uses_local(expr, m));
                }
            }
        }
        changed
    }
}

impl CommonSubexpressionElimination {
    fn moved_locals(kind: &StatementKind) -> Vec<Local> {
        let mut out = Vec::new();
        let mut scan = |op: &Operand| {
            if let Operand::Move(l) = op {
                out.push(*l);
            }
        };
        match kind {
            StatementKind::Assign(_, rval) => Self::for_each_operand(rval, &mut scan),
            StatementKind::SetIndex(obj, idx, val) => {
                scan(obj);
                scan(idx);
                scan(val);
            }
            StatementKind::SetAttr(obj, _, val) => {
                scan(obj);
                scan(val);
            }
            StatementKind::VectorStore(obj, idx, val) => {
                scan(obj);
                scan(idx);
                scan(val);
            }
            StatementKind::PtrStore(ptr, val) => {
                scan(ptr);
                scan(val);
            }
            _ => {}
        }
        out
    }

    fn for_each_operand(rval: &Rvalue, f: &mut impl FnMut(&Operand)) {
        match rval {
            Rvalue::Use(op)
            | Rvalue::UnaryOp(_, op)
            | Rvalue::GetAttr(op, _)
            | Rvalue::GetTag(op)
            | Rvalue::GetTypeId(op)
            | Rvalue::FatPtrData(op)
            | Rvalue::Cast(op, _)
            | Rvalue::PtrLoad(op)
            | Rvalue::VectorSplat(op, _) => f(op),
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) | Rvalue::VectorLoad(l, r, _) => {
                f(l);
                f(r);
            }
            Rvalue::VectorFMA(a, b, c) => {
                f(a);
                f(b);
                f(c);
            }
            Rvalue::Call { func, args } => {
                f(func);
                for arg in args {
                    f(arg);
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for op in ops {
                    f(op);
                }
            }
            Rvalue::Ref(_) | Rvalue::MutRef(_) | Rvalue::VTableLoad { .. } => {}
        }
    }

    fn rvalue_has_move(rval: &Rvalue) -> bool {
        let mut has = false;
        Self::for_each_operand(rval, &mut |op| {
            if matches!(op, Operand::Move(_)) {
                has = true;
            }
        });
        has
    }

    fn uses_local(rval: &Rvalue, local: Local) -> bool {
        let mut uses = false;
        Self::for_each_operand(rval, &mut |op| {
            if let Operand::Copy(l) | Operand::Move(l) = op
                && *l == local
            {
                uses = true;
            }
        });
        uses
    }
}
