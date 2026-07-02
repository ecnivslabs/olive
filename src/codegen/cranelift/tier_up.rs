use super::{ANY_SITE_GRADUATED, CraneliftCodegen};
use cranelift::prelude::*;
use cranelift_jit::JITModule;
use cranelift_module::{Linkage, Module};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Call count a function needs before it's worth recompiling with the full
/// optimizer. A conservative starting point, not a tuned value -- real tuning
/// needs profiling across representative Olive programs this project doesn't
/// have yet (see plan Phase 1 gate).
const TIER_UP_THRESHOLD: i64 = 1000;
const TIER_UP_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Spawns the background recompiler thread: polls every candidate function's
/// hotcount, and calls `retier` on any that crossed `TIER_UP_THRESHOLD`.
/// Detached by design -- the caller doesn't join it, matching how real JIT
/// engines run their optimizing-compiler thread as a daemon reclaimed at
/// process exit.
///
/// `catch_unwind` around `retier`: defense in depth against a broken
/// optimizer thread poisoning the `Mutex` and silently stopping all tiering.
pub(crate) fn spawn_tier_up_thread(
    codegen: Arc<Mutex<CraneliftCodegen<JITModule>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(TIER_UP_POLL_INTERVAL);
            let Ok(mut cg) = codegen.lock() else {
                return;
            };
            let candidates: Vec<String> = cg.dispatch_ids.keys().cloned().collect();
            for name in candidates {
                if cg.hotcount(&name).is_some_and(|c| c >= TIER_UP_THRESHOLD) {
                    let cg_ref = &mut *cg;
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        cg_ref.retier(&name)
                    }));
                }
            }
        }
    })
}

impl CraneliftCodegen<JITModule> {
    /// Recompiles `func_name` under a fresh name, atomically retargets its
    /// dispatch cell. `false` if no cell, or already retiered.
    ///
    /// Does not re-run the MIR optimizer on the clone: re-optimizing can
    /// split one Any-add into several, shifting site indices out from under
    /// `specialize_sites` -- silently defeats specialization while still
    /// passing correctness tests (guard-fallback masks it).
    ///
    /// Single-shot: no re-tiering, no de-tiering.
    ///
    /// Dispatch-cell store is `AtomicI64` Release; readers (`translate_call.rs`'s
    /// plain load) see old-or-new, never torn. In-flight calls in the old body
    /// finish unaffected -- no OSR needed.
    pub fn retier(&mut self, func_name: &str) -> bool {
        if !self.dispatch_ids.contains_key(func_name) {
            return false;
        }
        let tier1_name = format!("{func_name}$tier1");
        if self.func_ids.contains_key(&tier1_name) {
            return false;
        }
        let Some(orig) = self.functions.iter().find(|f| f.name == func_name) else {
            return false;
        };
        let mut retiered = orig.clone();
        retiered.name = tier1_name.clone();

        let mut sig = self.module.make_signature();
        for i in 0..retiered.arg_count {
            let ty = &retiered.locals[i + 1].ty;
            sig.params.push(AbiParam::new(super::imports::cl_type(ty)));
        }
        sig.returns.push(AbiParam::new(super::imports::cl_type(
            &retiered.locals[0].ty,
        )));

        let new_func_id = self
            .module
            .declare_function(&tier1_name, Linkage::Local, &sig)
            .unwrap();
        self.func_ids.insert(tier1_name.clone(), new_func_id);

        // Reuse `func_name`'s own original site range so `translate_binop.rs`
        // reads back *this function's* observed kind-history instead of running
        // off the end of `any_add_site_ids` (which is safe -- every consumer
        // bounds-checks -- but would mean the fast path never fires). The range
        // was finalized when the original `generate()` + `finalize()` ran, before
        // this function was ever called, so `get_finalized_data` is valid here --
        // unlike during the initial compile, where nothing is finalized yet and
        // every site necessarily reads as freshly-unseen anyway.
        self.specialize_sites.clear();
        if let Some(&(start, end)) = self.any_add_site_ranges.get(func_name) {
            self.any_add_site_cursor = start;
            for idx in start..end {
                let id = self.any_add_site_ids[idx];
                let byte = unsafe { *self.module.get_finalized_data(id).0 };
                if byte == ANY_SITE_GRADUATED {
                    self.specialize_sites.insert(idx);
                }
            }
        }
        self.translate_function(&retiered);
        self.specialize_sites.clear();
        self.module.finalize_definitions().unwrap();

        let new_addr = self.module.get_finalized_function(new_func_id) as i64;
        let cell_id = self.dispatch_ids[func_name];
        let (cell_ptr, _) = self.module.get_finalized_data(cell_id);
        let atomic = unsafe { &*(cell_ptr as *const AtomicI64) };
        atomic.store(new_addr, Ordering::Release);
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::{call_i64_1, compile};

    #[test]
    fn retier_keeps_result_correct() {
        // Needs an `Any`-typed `+` site to be `retier`-eligible at all --
        // dispatch cells are only installed where retiering can actually
        // specialize something (`setup/dispatch.rs`), and a plain-`i64` loop
        // has nothing for it to improve.
        let mut cg = compile(concat!(
            "fn sum_to(n: i64) -> i64:\n",
            "    let mut total: Any = 0\n",
            "    let mut i = 0\n",
            "    while i < n:\n",
            "        let a: Any = i\n",
            "        total = total + a\n",
            "        i = i + 1\n",
            "    return int(total)\n",
            "\n",
            "fn f(x: i64) -> i64:\n",
            "    return sum_to(x)\n",
        ));
        assert_eq!(call_i64_1(&mut cg, "f", 100), (0..100).sum::<i64>());
        assert!(cg.retier("sum_to"));
        assert_eq!(call_i64_1(&mut cg, "f", 100), (0..100).sum::<i64>());
        assert_eq!(call_i64_1(&mut cg, "f", 37), (0..37).sum::<i64>());
    }

    /// The actual Phase 2 payoff: an Any-add site inside a loop-bearing
    /// (retier-eligible) function, called past the sample window with plain
    /// ints so it graduates, then retiered. Checks the guarded fast path in
    /// `translate_binop.rs::translate_any_binop_specialized` produces
    /// identical results to the unspecialized runtime call, both in the
    /// fast-path range and across the 61-bit overflow boundary the guard has
    /// to fall back on.
    ///
    /// Calls route through `driver`, not `sum_loop` directly: a direct
    /// `call_i64_1(&mut cg, "sum_loop", ...)` resolves `sum_loop`'s *original*
    /// FuncId (`get_function` -> `func_ids["sum_loop"]`) and calls it straight,
    /// bypassing the dispatch cell entirely -- retiering only redirects calls
    /// compiled as `call_indirect` through that cell, i.e. calls made from
    /// *other compiled Olive functions*. Testing directly would silently
    /// exercise the untouched original body and never catch a broken guard.
    #[test]
    fn retier_specializes_graduated_any_add_site_correctly() {
        use crate::test_utils::compile_minimal;
        let src = concat!(
            "fn sum_loop(n: i64) -> i64:\n",
            "    let mut total = 0\n",
            "    let mut i = 0\n",
            "    while i < n:\n",
            "        let a: Any = i\n",
            "        let b: Any = 1\n",
            "        let x = a + b\n",
            "        total = total + int(x)\n",
            "        i = i + 1\n",
            "    return total\n",
            "\n",
            // compile_minimal skips the MIR inliner (Optimizer::minimal()),
            // so unlike the release-optimizer benchmark below, a plain
            // passthrough here is not at risk of being folded into its
            // caller and needs no loop-wrapping to stay a real call.
            "fn driver(n: i64) -> i64:\n",
            "    return sum_loop(n)\n",
        );

        // Baseline: unspecialized result for comparison, from a fresh compile
        // (compile_minimal avoids inlining changing which function owns the site).
        let mut baseline = compile_minimal(src);
        let expected_20 = call_i64_1(&mut baseline, "driver", 20);
        assert_eq!(expected_20, (1..=20).sum::<i64>());

        let mut cg = compile_minimal(src);
        // 20 iterations, well past the 8-call sample window, so the site
        // graduates to all-int before retier reads its history back.
        call_i64_1(&mut cg, "driver", 20);
        assert!(cg.retier("sum_loop"));

        assert_eq!(call_i64_1(&mut cg, "driver", 20), expected_20);
        assert_eq!(call_i64_1(&mut cg, "driver", 0), 0);
        assert_eq!(call_i64_1(&mut cg, "driver", 1), 1);
    }

    /// Same specialized site, but the loop's `i` operand crosses the inline
    /// `TAG_INT` 61-bit boundary partway through -- the guard's overflow check
    /// must catch this and fall back to the runtime call, not silently wrap.
    #[test]
    fn retier_specialized_site_falls_back_correctly_past_61_bits() {
        use crate::test_utils::compile_minimal;
        // `i` itself never gets huge, but `a` is seeded near INT_MAX so `a + b`
        // (b = 1) crosses the boundary on later iterations while staying a
        // plain int throughout -- exercises the fast path's own overflow arm,
        // not a different `any_is_*` branch.
        // `total` stays a small per-iteration counter, not a sum of huge values
        // -- accumulating `a` itself would overflow genuine i64 arithmetic
        // (unrelated to the 61-bit tag boundary this test targets) once enough
        // near-INT_MAX terms pile up.
        let int_max: i64 = (1i64 << 60) - 1;
        let src = format!(
            concat!(
                "fn sum_loop(n: i64) -> i64:\n",
                "    let mut total = 0\n",
                "    let mut a: Any = {int_max} - 5\n",
                "    let mut i = 0\n",
                "    while i < n:\n",
                "        let b: Any = 1\n",
                "        a = a + b\n",
                "        if int(a) > {int_max}:\n",
                "            total = total + 1\n",
                "        i = i + 1\n",
                "    return total\n",
                "\n",
                "fn driver(n: i64) -> i64:\n",
                "    return sum_loop(n)\n",
            ),
            int_max = int_max
        );

        let mut baseline = compile_minimal(&src);
        let expected = call_i64_1(&mut baseline, "driver", 20);

        let mut cg = compile_minimal(&src);
        call_i64_1(&mut cg, "driver", 20);
        assert!(cg.retier("sum_loop"));
        assert_eq!(call_i64_1(&mut cg, "driver", 20), expected);
    }

    /// The actual payoff, measured: specialized (post-retier) vs unspecialized
    /// execution of the same Any-add-heavy loop this project's whole point is to
    /// speed up. `#[ignore]`, same as the Phase 0/1 overhead benchmarks --
    /// `cargo test --release -- --ignored --nocapture retier_speedup`.
    #[test]
    #[ignore]
    fn retier_speedup_on_any_add_heavy_workload() {
        use crate::test_utils::compile_minimal;
        use std::time::Instant;

        // `compile_minimal`, not `compile_release`: default `pit run` (no
        // `--release`) uses `Optimizer::minimal()` for MIR *and* pins the
        // Cranelift backend to `opt_level = none` -- exactly what
        // `compile_minimal` does, and exactly the config this benchmark needs
        // to be representative. `compile_release`'s `Optimizer::new()` runs
        // the inliner, whose threshold is "callee has fewer than 100 basic
        // blocks" (`mir/optimizations/inliner.rs`) -- i.e. it inlines nearly
        // everything, including `sum_loop` itself, straight into `driver`.
        // Once that happens there is no runtime call left for the dispatch
        // cell to intercept: `sum_loop`'s standalone body (and its hotcount)
        // never execute again, retiering it is a complete no-op, and
        // `--release` builds don't need runtime tier-up in the first place
        // (they start at full optimization). Tier-up's actual job is
        // default, non-`--release` `pit run`, where nothing gets inlined away
        // and specialization has real calls to redirect -- confirmed via
        // `diagnostic_specialized_loop_avoids_runtime_calls` showing 100% of
        // calls hitting the fast path under this exact configuration.
        //
        // `driver` calls `sum_loop` from *inside a loop* (matches the
        // already-working Phase 1 `driver`/`small_work` benchmark shape) so
        // the call compiles as `call_indirect` through the dispatch cell --
        // calling `sum_loop` directly from the test harness bypasses the
        // cell entirely (resolves its original FuncId straight), which is
        // why the very first cut of this benchmark measured 0.93-0.97x: it
        // silently timed the unspecialized original body on both sides,
        // never the retiered one.
        let src = concat!(
            "fn sum_loop(n: i64) -> i64:\n",
            "    let mut result: Any = 0\n",
            "    let mut i = 0\n",
            "    while i < n:\n",
            "        let a: Any = i\n",
            "        let b: Any = 1\n",
            "        result = a + b\n",
            "        i = i + 1\n",
            "    return int(result)\n",
            "\n",
            "fn driver(calls: i64) -> i64:\n",
            "    let mut total = 0\n",
            "    let mut i = 0\n",
            "    while i < calls:\n",
            "        total = total + sum_loop(8)\n",
            "        i = i + 1\n",
            "    return total\n",
        );
        const TRIALS: usize = 21;
        const CALLS: i64 = 3_000_000;

        // Baseline: stays at tier-0 forever (never retiered) -- the same
        // instance shape a program would see if it never ran long enough to
        // cross the tier-up threshold.
        let mut tier0_runs = Vec::with_capacity(TRIALS);
        for _ in 0..TRIALS {
            let mut cg = compile_minimal(src);
            let start = Instant::now();
            call_i64_1(&mut cg, "driver", CALLS);
            tier0_runs.push(start.elapsed());
        }

        // Specialized: warm up past the sample window (25 calls to
        // sum_loop(8), well past the 8-call window), retier, then measure.
        let mut specialized_runs = Vec::with_capacity(TRIALS);
        for _ in 0..TRIALS {
            let mut cg = compile_minimal(src);
            call_i64_1(&mut cg, "driver", 25);
            assert!(cg.retier("sum_loop"));
            let start = Instant::now();
            call_i64_1(&mut cg, "driver", CALLS);
            specialized_runs.push(start.elapsed());
        }

        tier0_runs.sort();
        specialized_runs.sort();
        let tier0_median = tier0_runs[TRIALS / 2];
        let specialized_median = specialized_runs[TRIALS / 2];
        let speedup = tier0_median.as_secs_f64() / specialized_median.as_secs_f64();

        println!(
            "driver({CALLS}) x sum_loop(8) median: tier0={tier0_median:?} specialized={specialized_median:?} speedup={speedup:.2}x"
        );
    }

    #[test]
    fn retier_is_single_shot() {
        let mut cg = compile(concat!(
            "fn sum_to(n: i64) -> i64:\n",
            "    let mut total: Any = 0\n",
            "    let mut i = 0\n",
            "    while i < n:\n",
            "        let a: Any = i\n",
            "        total = total + a\n",
            "        i = i + 1\n",
            "    return int(total)\n",
        ));
        assert!(cg.retier("sum_to"));
        assert!(!cg.retier("sum_to"));
    }

    #[test]
    fn retier_refuses_non_candidate() {
        let mut cg = compile("fn f(n: i64) -> i64:\n    return n + 1\n");
        assert!(!cg.retier("f"));
        assert!(!cg.retier("does_not_exist"));
    }
}
