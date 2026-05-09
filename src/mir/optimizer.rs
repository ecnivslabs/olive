use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

use crate::mir::optimizations::{
    Transform, const_fold::ConstantFolding, const_prop::ConstantPropagation,
    copy_prop::CopyPropagation, dce::DeadCodeElimination, gvn::GlobalValueNumbering,
    inliner::Inliner, licm::LICM, loop_unroll::LoopUnroll, peephole::PeepholeOptimize,
    simplify_cfg::SimplifyCfg, strength_reduction::StrengthReduction, tail_call::TailCallOpt,
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
                Box::new(GlobalValueNumbering),
                Box::new(SimplifyCfg),
                Box::new(DeadCodeElimination),
            ],
            late_passes: vec![
                Box::new(TailCallOpt),
                Box::new(LICM),
                Box::new(LoopUnroll),
                Box::new(SimplifyCfg),
                Box::new(DeadCodeElimination),
                Box::new(CopyPropagation),
                Box::new(ConstantPropagation),
                Box::new(ConstantFolding),
                Box::new(StrengthReduction),
                Box::new(PeepholeOptimize),
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
            let is_trivial = func.basic_blocks.len() <= 2
                && func.basic_blocks.iter().all(|bb| {
                    bb.statements.iter().all(|s| {
                        matches!(
                            &s.kind,
                            StatementKind::Assign(_, Rvalue::Call { .. })
                                | StatementKind::Assign(_, Rvalue::Use(_))
                                | StatementKind::StorageLive(_)
                                | StatementKind::StorageDead(_)
                                | StatementKind::Drop(_)
                        )
                    })
                });

            if is_trivial && func.name == "__main__" {
                SimplifyCfg.run(func);
                DeadCodeElimination.run(func);
                continue;
            }

            self.inliner.inline_function(func, &fn_map, 12);

            let mut changed = true;
            let mut iterations = 0;
            while changed {
                iterations += 1;
                if iterations > 10 {
                    break;
                }
                changed = false;
                for pass in &self.scalar_passes {
                    if pass.run(func) {
                        changed = true;
                    }
                }
            }

            for pass in &self.late_passes {
                pass.run(func);
            }
        }
    }
}
