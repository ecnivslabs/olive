use crate::mir::MirFunction;

pub mod algebraic;
pub mod bounds_check_elim;
pub mod const_fold;
pub mod const_prop;
pub mod copy_prop;
pub mod cse;
pub mod dce;
pub mod drop_hooks;
pub mod gencheck;
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

#[cfg(test)]
mod tests;
