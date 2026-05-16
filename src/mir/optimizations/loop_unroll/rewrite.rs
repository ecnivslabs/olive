use crate::mir::*;

/// Constant `c` from `i + c`, either operand order.
pub(super) fn step_from_add(a: &Operand, b: &Operand, induction: Local) -> Option<i64> {
    match (a, b) {
        (Operand::Copy(s), Operand::Constant(Constant::Int(c)))
        | (Operand::Constant(Constant::Int(c)), Operand::Copy(s))
            if *s == induction =>
        {
            Some(*c)
        }
        _ => None,
    }
}

/// Whether a statement reads `local` as a value or by reference.
pub(super) fn stmt_reads_local(stmt: &Statement, local: Local) -> bool {
    let mut hit = false;
    let mut check = |op: &Operand| {
        if matches!(op, Operand::Copy(l) | Operand::Move(l) if *l == local) {
            hit = true;
        }
    };
    match &stmt.kind {
        StatementKind::Assign(_, rval) => match rval {
            Rvalue::Use(op)
            | Rvalue::UnaryOp(_, op)
            | Rvalue::Cast(op, _)
            | Rvalue::GetAttr(op, _)
            | Rvalue::GetTag(op)
            | Rvalue::GetTypeId(op)
            | Rvalue::VectorSplat(op, _)
            | Rvalue::PtrLoad(op)
            | Rvalue::GenOf(op)
            | Rvalue::FatPtrData(op)
            | Rvalue::VTableLoad { vtable: op, .. } => check(op),
            Rvalue::BinaryOp(_, a, b) | Rvalue::GetIndex(a, b, _) | Rvalue::VectorLoad(a, b, _) => {
                check(a);
                check(b);
            }
            Rvalue::VectorFMA(a, b, c) => {
                check(a);
                check(b);
                check(c);
            }
            Rvalue::Call { func, args } => {
                check(func);
                args.iter().for_each(&mut check);
            }
            Rvalue::Aggregate(_, ops) => ops.iter().for_each(&mut check),
            Rvalue::Ref(l) | Rvalue::MutRef(l) => hit |= *l == local,
        },
        StatementKind::SetAttr(a, _, b) | StatementKind::PtrStore(a, b) => {
            check(a);
            check(b);
        }
        StatementKind::SetIndex(a, b, c, _) | StatementKind::VectorStore(a, b, c) => {
            check(a);
            check(b);
            check(c);
        }
        StatementKind::GenCheck { value, generation } => {
            hit |= *value == local || *generation == local;
        }
        StatementKind::StorageLive(_) | StatementKind::StorageDead(_) | StatementKind::Drop(_) => {}
    }
    hit
}

/// Whether an rvalue takes `local` by reference.
pub(super) fn rvalue_refs_local(rval: &Rvalue, local: Local) -> bool {
    matches!(rval, Rvalue::Ref(l) | Rvalue::MutRef(l) if *l == local)
}

/// Whether a statement is a storage marker for `local`.
pub(super) fn is_storage_of(stmt: &Statement, local: Local) -> bool {
    matches!(
        &stmt.kind,
        StatementKind::StorageLive(l) | StatementKind::StorageDead(l) if *l == local
    )
}

/// Replaces every read of `from` with `to`.
pub(super) fn subst_local(stmt: &mut Statement, from: Local, to: &Operand) {
    match &mut stmt.kind {
        StatementKind::Assign(_, rval) => subst_rvalue(rval, from, to),
        StatementKind::SetAttr(obj, _, val) => {
            subst_operand(obj, from, to);
            subst_operand(val, from, to);
        }
        StatementKind::SetIndex(obj, idx, val, _) => {
            subst_operand(obj, from, to);
            subst_operand(idx, from, to);
            subst_operand(val, from, to);
        }
        StatementKind::PtrStore(a, b) | StatementKind::VectorStore(a, _, b) => {
            subst_operand(a, from, to);
            subst_operand(b, from, to);
        }
        // A gen check names locals directly; substitution never applies.
        StatementKind::GenCheck { .. } => {}
        StatementKind::StorageLive(_) | StatementKind::StorageDead(_) | StatementKind::Drop(_) => {}
    }
}

fn subst_rvalue(rval: &mut Rvalue, from: Local, to: &Operand) {
    match rval {
        Rvalue::Use(op)
        | Rvalue::UnaryOp(_, op)
        | Rvalue::Cast(op, _)
        | Rvalue::GetAttr(op, _)
        | Rvalue::GetTag(op)
        | Rvalue::GetTypeId(op)
        | Rvalue::PtrLoad(op)
        | Rvalue::GenOf(op)
        | Rvalue::FatPtrData(op)
        | Rvalue::VTableLoad { vtable: op, .. } => subst_operand(op, from, to),
        Rvalue::BinaryOp(_, a, b) | Rvalue::GetIndex(a, b, _) => {
            subst_operand(a, from, to);
            subst_operand(b, from, to);
        }
        Rvalue::Call { func, args } => {
            subst_operand(func, from, to);
            for a in args {
                subst_operand(a, from, to);
            }
        }
        Rvalue::Aggregate(_, ops) => {
            for op in ops {
                subst_operand(op, from, to);
            }
        }
        Rvalue::Ref(_)
        | Rvalue::MutRef(_)
        | Rvalue::VectorSplat(..)
        | Rvalue::VectorLoad(..)
        | Rvalue::VectorFMA(..) => {}
    }
}

fn subst_operand(op: &mut Operand, from: Local, to: &Operand) {
    if matches!(op, Operand::Copy(l) | Operand::Move(l) if *l == from) {
        *op = to.clone();
    }
}
