use crate::compile::errors::Diagnostic;
use crate::mir::optimizations::ownership::CopySite;
use crate::mir::*;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cell::RefCell;

use crate::mir::optimizations::{
    Transform, algebraic::AlgebraicSimplification, bounds_check_elim::BoundsCheckElim,
    const_fold::ConstantFolding, const_prop::ConstantPropagation, copy_prop::CopyPropagation,
    cse::CommonSubexpressionElimination, dce::DeadCodeElimination, devirtualize::Devirtualize,
    devirtualize_closure::DevirtualizeClosures, drop_hooks, gencheck::GenCheckInsertion,
    gil_fusion::GilFusion, gvn::GlobalValueNumbering, inliner::Inliner, licm::Licm,
    list_append::ListAppend, loop_unroll::LoopUnroll, move_elision::MoveElision,
    ownership::OwnershipInference,
    peephole::PeepholeOptimize, scalarize::ScalarizeStructs, simplify_cfg::SimplifyCfg,
    strength_reduction::StrengthReduction, tail_call::TailCallOpt, vectorize::LoopVectorizer,
};

pub struct Optimizer {
    scalar_passes: Vec<Box<dyn Transform>>,
    late_passes: Vec<Box<dyn Transform>>,
    inliner: Inliner,
    release: bool,
    explain_copies: bool,
    vtables: HashMap<String, Vec<String>>,
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
            explain_copies: false,
            vtables: HashMap::default(),
        }
    }

    pub fn set_explain_copies(&mut self, val: bool) {
        self.explain_copies = val;
    }

    /// Trait vtables from the MIR builder; enables devirtualization.
    pub fn set_vtables(&mut self, vtables: HashMap<String, Vec<String>>) {
        self.vtables = vtables;
    }

    pub fn run(&self, functions: &mut [MirFunction]) -> (Vec<Diagnostic>, Vec<CopySite>) {
        // Ownership inference is semantic (not optional): drops must agree
        // with the inferred owner in every pipeline.
        let ownership = OwnershipInference {
            borrowed_returns: crate::mir::optimizations::ownership::compute_borrowed_returns(
                functions,
            ),
            param_escapes: crate::mir::optimizations::ownership::compute_param_escapes(functions),
            explain_copies: self.explain_copies,
            copy_sites: RefCell::new(Vec::new()),
        };
        for func in functions.iter_mut() {
            ownership.run(func);
            // Runs before drop lowering so the temporary list literal it
            // deletes still has a plain `Drop` to delete alongside it.
            ListAppend.run(func);
        }
        let has_drop = drop_hooks::collect_struct_has_drop(functions);
        for func in functions.iter_mut() {
            drop_hooks::lower_drop_hooks(func, &has_drop);
        }
        let copy_sites = ownership.copy_sites.replace(Vec::new());

        if !self.release {
            for func in functions.iter_mut() {
                SimplifyCfg.run(func);
                DeadCodeElimination.run(func);
                MoveElision.run(func);
            }
            let diags = self.insert_gen_checks(functions, ownership);
            return (diags, copy_sites);
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

            // Inlining can leave an orphaned block behind (an unused
            // continuation from splitting a callee's return site). Pruning
            // it here keeps single-def analysis in the devirtualize passes
            // from seeing a dead second definition and refusing to fire.
            SimplifyCfg.run(func);

            let devirt = Devirtualize {
                vtables: &self.vtables,
                has_drop: &has_drop,
            };
            let devirt_closures = DevirtualizeClosures.run(func);
            if devirt.run(func) || devirt_closures {
                self.inliner.inline_function(func, &fn_map, 12);
            }

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

            // Runs after everything else settles final statement order, so
            // the runs it fuses are the ones that actually reach codegen.
            GilFusion.run(func);
        }

        let diags = self.insert_gen_checks(functions, ownership);
        (diags, copy_sites)
    }

    /// Runs after every other pass in both pipelines: checks must sit exactly
    /// where the analysis proved them necessary on the final statement order.
    fn insert_gen_checks(
        &self,
        functions: &mut [MirFunction],
        ownership: OwnershipInference,
    ) -> Vec<Diagnostic> {
        let gencheck = GenCheckInsertion {
            borrowed_returns: ownership.borrowed_returns,
            param_escapes: ownership.param_escapes,
            diagnostics: Default::default(),
        };
        for func in functions.iter_mut() {
            gencheck.run(func);
        }
        gencheck.diagnostics.into_inner()
    }
}
