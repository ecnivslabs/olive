use crate::mir::{Local, MirFunction, Rvalue};

pub mod algebraic;
pub mod bounds_check_elim;
pub mod const_fold;
pub mod const_prop;
pub mod copy_prop;
pub mod cse;
pub mod dce;
pub mod devirtualize;
pub mod devirtualize_closure;
pub mod drop_hooks;
pub mod gencheck;
pub mod gil_fusion;
pub mod gvn;
pub mod inliner;
pub mod licm;
pub mod loop_unroll;
pub mod move_elision;
pub mod ownership;
pub mod peephole;
pub mod scalarize;
pub mod simplify_cfg;
pub mod strength_reduction;
pub mod tail_call;
pub mod vectorize;

pub trait Transform {
    fn run(&self, func: &mut MirFunction) -> bool;
}

/// Locals a `Rvalue` reads. Shared by passes that need to know whether a
/// value they're tracking escapes into an operand they don't special-case.
pub(crate) fn rvalue_operand_locals(rval: &Rvalue, out: &mut Vec<Local>) {
    fn op_local(op: &crate::mir::Operand) -> Option<Local> {
        match op {
            crate::mir::Operand::Copy(l) | crate::mir::Operand::Move(l) => Some(*l),
            crate::mir::Operand::Constant(_) => None,
        }
    }
    match rval {
        Rvalue::Use(op)
        | Rvalue::UnaryOp(_, op)
        | Rvalue::Cast(op, _)
        | Rvalue::GetAttr(op, _)
        | Rvalue::GetTag(op)
        | Rvalue::GetTypeId(op)
        | Rvalue::VectorSplat(op, _)
        | Rvalue::VectorReduce(_, op, _)
        | Rvalue::PtrLoad(op)
        | Rvalue::GenOf(op)
        | Rvalue::FatPtrData(op)
        | Rvalue::VTableLoad { vtable: op, .. } => out.extend(op_local(op)),
        Rvalue::BinaryOp(_, a, b) | Rvalue::GetIndex(a, b, _) | Rvalue::VectorLoad(a, b, _) => {
            out.extend(op_local(a));
            out.extend(op_local(b));
        }
        Rvalue::VectorFMA(a, b, c) => {
            out.extend(op_local(a));
            out.extend(op_local(b));
            out.extend(op_local(c));
        }
        Rvalue::Call { func, args } => {
            out.extend(op_local(func));
            for a in args {
                out.extend(op_local(a));
            }
        }
        Rvalue::Aggregate(_, ops) => {
            for op in ops {
                out.extend(op_local(op));
            }
        }
        Rvalue::Ref(l) | Rvalue::MutRef(l) => out.push(*l),
    }
}

#[cfg(test)]
mod tests;
