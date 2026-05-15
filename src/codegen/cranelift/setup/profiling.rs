use super::super::CraneliftCodegen;
use cranelift_module::{DataDescription, Linkage, Module};

/// Name of the zero-initialized i64 counter data segment for a function.
fn hotcount_name(func_name: &str) -> String {
    format!("__olive_hotcount${func_name}")
}

impl<M: Module> CraneliftCodegen<M> {
    /// Declares one zero-initialized i64 call-counter per (non-async) function
    /// when profiling is enabled. Async functions are not yet instrumented:
    /// their translation goes through `translate_async_sm_poll`/`generate_async_wrapper`
    /// rather than the plain `translate_function` entry this counter hooks into.
    pub(super) fn generate_hotcounts(&mut self) {
        if !self.profile {
            return;
        }
        let names: Vec<String> = self
            .functions
            .iter()
            .filter(|f| !f.is_async)
            .map(|f| f.name.clone())
            .collect();
        for name in names {
            let mut data_ctx = DataDescription::new();
            data_ctx.define_zeroinit(8);
            let id = self
                .module
                .declare_data(&hotcount_name(&name), Linkage::Local, true, false)
                .unwrap();
            self.module.define_data(id, &data_ctx).unwrap();
            self.hotcount_ids.insert(name, id);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::{call_i64_1, compile};

    #[test]
    fn hotcount_tracks_call_count() {
        let mut cg = compile("fn f(x: i64) -> i64:\n    return x + 1\n");
        assert_eq!(cg.hotcount("f"), Some(0));
        for i in 0..5 {
            call_i64_1(&mut cg, "f", i);
        }
        assert_eq!(cg.hotcount("f"), Some(5));
    }

    #[test]
    fn hotcount_absent_for_unknown_function() {
        let mut cg = compile("fn f(x: i64) -> i64:\n    return x\n");
        assert_eq!(cg.hotcount("does_not_exist"), None);
    }

    /// Measures the call-count counter's real cost on a call-heavy workload
    /// (naive recursive fibonacci, ~2.7M calls at n=30) to gate Phase 1 work
    /// on "negligible overhead" rather than eyeballing it. Ignored by default
    /// since it's a perf measurement, not a correctness check; run with
    /// `cargo test --release -- --ignored --nocapture profiling_overhead`.
    #[test]
    #[ignore]
    fn profiling_overhead_on_call_heavy_workload() {
        use crate::test_utils::{call_i64_1, compile_unprofiled};
        use std::time::Instant;

        let src = "fn fib(n: i64) -> i64:\n    if n < 2:\n        return n\n    return fib(n - 1) + fib(n - 2)\n";
        const TRIALS: usize = 21;
        const N: i64 = 30;

        let mut profiled_runs = Vec::with_capacity(TRIALS);
        for _ in 0..TRIALS {
            let mut cg = compile(src);
            let start = Instant::now();
            call_i64_1(&mut cg, "fib", N);
            profiled_runs.push(start.elapsed());
        }

        let mut unprofiled_runs = Vec::with_capacity(TRIALS);
        for _ in 0..TRIALS {
            let mut cg = compile_unprofiled(src);
            let start = Instant::now();
            call_i64_1(&mut cg, "fib", N);
            unprofiled_runs.push(start.elapsed());
        }

        profiled_runs.sort();
        unprofiled_runs.sort();
        let profiled_median = profiled_runs[TRIALS / 2];
        let unprofiled_median = unprofiled_runs[TRIALS / 2];
        let overhead_pct =
            (profiled_median.as_secs_f64() / unprofiled_median.as_secs_f64() - 1.0) * 100.0;

        println!(
            "fib({N}) median: unprofiled={unprofiled_median:?} profiled={profiled_median:?} overhead={overhead_pct:.2}%"
        );
    }

    /// Companion to `profiling_overhead_on_call_heavy_workload`: measures the
    /// case `setup/dispatch.rs`'s eligibility gate *does* apply to. That gate
    /// used to be "contains a loop"; it's now "has at least one graduated-
    /// eligible `Any`-typed `+` site", the only thing a `retier` can currently
    /// improve (Any-add specialization, Phase 2) -- so `small_work` here has
    /// one, unlike a plain-`i64` accumulator loop (which no longer gets a
    /// cell at all: see `dispatch_cell_skipped_for_loop_with_no_any_add_sites`
    /// in `setup/dispatch.rs`, and this benchmark's own history below for why
    /// that narrower gate exists). `driver` calls it through the cell in a
    /// tight loop; this measures what a real hot Any-typed loop actually pays
    /// for tiering eligibility before any specialization has happened yet.
    ///
    /// Uses `compile_minimal`/`compile_minimal_unprofiled`, not `compile`/
    /// `compile_unprofiled`: the first cut of this benchmark used the latter
    /// pair (`Optimizer::new()`) and reported "noise floor, 0% overhead" --
    /// which turned out to mean nothing, since that optimizer's inliner
    /// (`mir/optimizations/inliner.rs`, "callee under 100 basic blocks", i.e.
    /// nearly everything) folds `small_work` straight into `driver` before any
    /// dispatch-cell indirection exists in the compiled code. Confirmed via
    /// `hotcount("small_work")` staying `0` after calling `driver` under that
    /// config: the standalone body, cell and all, was simply never executed.
    /// `Optimizer::minimal()` (default `pit run`, no `--release`) skips the
    /// inliner, so this is the config where the dispatch cell this test is
    /// actually named for exists in the running program at all.
    #[test]
    #[ignore]
    fn profiling_overhead_on_dispatch_cell_workload() {
        use crate::test_utils::{call_i64_1, compile_minimal, compile_minimal_unprofiled};
        use std::time::Instant;

        let src = concat!(
            "fn small_work(n: i64) -> i64:\n",
            "    let mut total: Any = 0\n",
            "    let mut i = 0\n",
            "    while i < n:\n",
            "        let a: Any = i\n",
            "        total = total + a\n",
            "        i = i + 1\n",
            "    return int(total)\n",
            "\n",
            "fn driver(calls: i64) -> i64:\n",
            "    let mut total = 0\n",
            "    let mut i = 0\n",
            "    while i < calls:\n",
            "        total = total + small_work(8)\n",
            "        i = i + 1\n",
            "    return total\n",
        );
        const TRIALS: usize = 21;
        const CALLS: i64 = 3_000_000;

        let mut profiled_runs = Vec::with_capacity(TRIALS);
        for trial in 0..TRIALS {
            let mut cg = compile_minimal(src);
            assert!(cg.dispatch_ids.contains_key("small_work"));
            let start = Instant::now();
            call_i64_1(&mut cg, "driver", CALLS);
            profiled_runs.push(start.elapsed());
            if trial == 0 {
                assert!(
                    cg.hotcount("small_work").is_some_and(|c| c > 0),
                    "small_work was never actually called -- inlined away?"
                );
            }
        }

        let mut unprofiled_runs = Vec::with_capacity(TRIALS);
        for _ in 0..TRIALS {
            let mut cg = compile_minimal_unprofiled(src);
            let start = Instant::now();
            call_i64_1(&mut cg, "driver", CALLS);
            unprofiled_runs.push(start.elapsed());
        }

        profiled_runs.sort();
        unprofiled_runs.sort();
        let profiled_median = profiled_runs[TRIALS / 2];
        let unprofiled_median = unprofiled_runs[TRIALS / 2];
        let overhead_pct =
            (profiled_median.as_secs_f64() / unprofiled_median.as_secs_f64() - 1.0) * 100.0;

        println!(
            "driver({CALLS}) x small_work(8) median: unprofiled={unprofiled_median:?} profiled={profiled_median:?} overhead={overhead_pct:.2}%"
        );
    }

    /// Isolates the cost of `__olive_any_add_profiled`'s kind-history write
    /// (`kind_history.rs`/`std_lib`'s `olive_any_add_profiled`) on a tight loop
    /// that's nothing but Any-typed `+`, the worst case for this specific
    /// instrumentation since real per-call work is at its minimum.
    #[test]
    #[ignore]
    fn profiling_overhead_on_any_add_heavy_workload() {
        use crate::test_utils::{call_i64_1, compile_unprofiled};
        use std::time::Instant;

        let src = concat!(
            "fn add_any(a: Any, b: Any) -> Any:\n",
            "    return a + b\n",
            "\n",
            "fn driver(n: i64) -> i64:\n",
            "    let mut total = 0\n",
            "    let mut i = 0\n",
            "    while i < n:\n",
            "        let r = add_any(i, 1)\n",
            "        total = total + int(r)\n",
            "        i = i + 1\n",
            "    return total\n",
        );
        const TRIALS: usize = 21;
        const N: i64 = 3_000_000;

        let mut profiled_runs = Vec::with_capacity(TRIALS);
        for _ in 0..TRIALS {
            let mut cg = compile(src);
            let start = Instant::now();
            call_i64_1(&mut cg, "driver", N);
            profiled_runs.push(start.elapsed());
        }

        let mut unprofiled_runs = Vec::with_capacity(TRIALS);
        for _ in 0..TRIALS {
            let mut cg = compile_unprofiled(src);
            let start = Instant::now();
            call_i64_1(&mut cg, "driver", N);
            unprofiled_runs.push(start.elapsed());
        }

        profiled_runs.sort();
        unprofiled_runs.sort();
        let profiled_median = profiled_runs[TRIALS / 2];
        let unprofiled_median = unprofiled_runs[TRIALS / 2];
        let overhead_pct =
            (profiled_median.as_secs_f64() / unprofiled_median.as_secs_f64() - 1.0) * 100.0;

        println!(
            "driver({N}) any-add-only median: unprofiled={unprofiled_median:?} profiled={profiled_median:?} overhead={overhead_pct:.2}%"
        );
    }
}
