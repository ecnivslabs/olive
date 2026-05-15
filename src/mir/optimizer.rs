use crate::mir::*;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::mir::optimizations::{
    Transform, algebraic::AlgebraicSimplification, bounds_check_elim::BoundsCheckElim,
    const_fold::ConstantFolding, const_prop::ConstantPropagation, copy_prop::CopyPropagation,
    cse::CommonSubexpressionElimination, dce::DeadCodeElimination, gvn::GlobalValueNumbering,
    inliner::Inliner, licm::Licm, loop_unroll::LoopUnroll, move_elision::MoveElision,
    peephole::PeepholeOptimize, scalarize::ScalarizeStructs, simplify_cfg::SimplifyCfg,
    strength_reduction::StrengthReduction, tail_call::TailCallOpt, vectorize::LoopVectorizer,
};

pub struct Optimizer {
    scalar_passes: Vec<Box<dyn Transform>>,
    late_passes: Vec<Box<dyn Transform>>,
    inliner: Inliner,
    release: bool,
}

impl Default for Optimizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Optimizer {
    /// Full optimizing pipeline. Used for release builds and by the in-process
    /// test harness so every pass stays exercised.
    pub fn new() -> Self {
        Self::with_release(true, HashSet::default())
    }

    /// Lean pipeline for debug builds: only the cleanup needed to keep codegen
    /// fast and the MIR sane, trading runtime speed for quick compiles.
    pub fn minimal() -> Self {
        Self::with_release(false, HashSet::default())
    }

    /// `new()`'s pipeline, but the inliner favors callees named in `hot_functions`.
    pub fn new_with_hot_functions(hot_functions: HashSet<String>) -> Self {
        Self::with_release(true, hot_functions)
    }

    fn with_release(release: bool, hot_functions: HashSet<String>) -> Self {
        Self {
            scalar_passes: vec![
                Box::new(CopyPropagation),
                Box::new(ConstantPropagation),
                Box::new(ConstantFolding),
                Box::new(AlgebraicSimplification),
                Box::new(StrengthReduction),
                Box::new(CommonSubexpressionElimination),
                Box::new(PeepholeOptimize),
                Box::new(GlobalValueNumbering),
                Box::new(SimplifyCfg),
                Box::new(DeadCodeElimination),
                Box::new(MoveElision),
            ],
            late_passes: vec![
                Box::new(TailCallOpt),
                Box::new(Licm),
                Box::new(LoopVectorizer),
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
            inliner: Inliner::with_hot_functions(hot_functions),
            release,
        }
    }

    pub fn run(&self, functions: &mut [MirFunction]) {
        if !self.release {
            for func in functions.iter_mut() {
                SimplifyCfg.run(func);
                DeadCodeElimination.run(func);
            }
            return;
        }

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

            ScalarizeStructs.run(func);

            for pass in &self.late_passes {
                pass.run(func);
            }

            // Runs last so no later pass can rewrite an access whose bounds
            // check it has already proven redundant.
            BoundsCheckElim.run(func);
        }
    }
}
