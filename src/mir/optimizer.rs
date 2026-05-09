use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

use crate::mir::optimizations::{
    Transform, const_fold::ConstantFolding, const_prop::ConstantPropagation,
    copy_prop::CopyPropagation, cse::CommonSubexpressionElimination, dce::DeadCodeElimination,
    inliner::Inliner, peephole::PeepholeOptimize, simplify_cfg::SimplifyCfg,
    strength_reduction::StrengthReduction,
    licm::LICM,
};

pub struct Optimizer {
    scalar_passes: Vec<Box<dyn Transform>>,
    late_passes: Vec<Box<dyn Transform>>,
    inliner: Inliner,
}

impl Optimizer {
    pub fn new() -> Self {
        Self {
            scalar_passes: vec![
                Box::new(CopyPropagation),
                Box::new(ConstantPropagation),
                Box::new(ConstantFolding),
                Box::new(StrengthReduction),
                Box::new(PeepholeOptimize),
                Box::new(CommonSubexpressionElimination),
                Box::new(SimplifyCfg),
                Box::new(DeadCodeElimination),
                Box::new(SimplifyCfg),
            ],
            late_passes: vec![
                Box::new(LICM),
                Box::new(SimplifyCfg),
                Box::new(DeadCodeElimination),
            ],
            inliner: Inliner::new(),
        }
    }

    pub fn run(&self, functions: &mut Vec<MirFunction>) {
        let fn_map: HashMap<String, MirFunction> = functions
            .iter()
            .map(|f| (f.name.clone(), f.clone()))
            .collect();

        for func in functions.iter_mut() {
            self.inliner.inline_function(func, &fn_map, 10);

            // Phase 1: Iterative scalar optimization to fixed-point.
            let mut changed = true;
            let mut iterations = 0;
            while changed {
                iterations += 1;
                if iterations > 100 {
                    break;
                }
                changed = false;
                for pass in &self.scalar_passes {
                    if pass.run(func) {
                        changed = true;
                    }
                }
            }

            // Phase 2: One-shot late passes (vectorization, cleanup).
            // These run exactly once — vectorization is a structural CFG
            // transformation that must not re-enter the iterative loop.
            for pass in &self.late_passes {
                pass.run(func);
            }
        }
    }
}
