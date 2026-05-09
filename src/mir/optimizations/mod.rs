use crate::mir::MirFunction;

pub mod const_fold;
pub mod const_prop;
pub mod copy_prop;
pub mod cse;
pub mod dce;
pub mod inliner;
pub mod licm;
pub mod peephole;
pub mod simplify_cfg;
pub mod strength_reduction;
pub mod vectorize;

#[allow(dead_code)]
pub trait Transform {
    fn name(&self) -> &'static str;
    fn run(&self, func: &mut MirFunction) -> bool;
}
