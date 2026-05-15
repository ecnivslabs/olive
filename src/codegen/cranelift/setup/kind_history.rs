use super::super::imports::is_any_op;
use super::super::{CraneliftCodegen, is_specializable_any_binop};
use crate::mir::{MirFunction, Rvalue, StatementKind};
use cranelift_jit::JITModule;
use cranelift_module::{DataDescription, Linkage, Module};
use rustc_hash::FxHashMap as HashMap;

/// Despite the name, counts every specializable binop site, not just `+`.
fn count_any_add_sites_in(func: &MirFunction) -> usize {
    let mut n = 0;
    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            if let StatementKind::Assign(_, Rvalue::BinaryOp(op, lhs, rhs)) = &stmt.kind
                && is_specializable_any_binop(op)
                && (is_any_op(func, lhs) || is_any_op(func, rhs))
            {
                n += 1;
            }
        }
    }
    n
}

impl<M: Module> CraneliftCodegen<M> {
    /// Counts specializable-binop call sites and each function's `[start,
    /// end)` range -- must stay in lockstep with `translate_binop.rs`'s
    /// cursor. Name kept as `_add_` despite covering all ops now (rename
    /// across a dozen call sites for zero behavior change isn't worth it).
    ///
    /// Excludes state-machine-capable async functions (their body is
    /// translated by `translate_async_sm_poll`, a separate path the cursor
    /// isn't threaded into) but includes non-SM-capable async functions,
    /// whose body-fallback clone (`{name}__async_body`, `is_async` cleared) *does*
    /// go through the ordinary `translate_function` path -- mirrors the exact
    /// branch `generate()` takes in `setup/mod.rs`. Ranges are recorded under the
    /// function's original name even for the async case; only non-async
    /// (dispatch-cell-eligible, per `generate_dispatch_cells`) functions are ever
    /// looked up by `retier()`, so an async entry existing is harmless.
    pub(crate) fn count_any_add_sites(&self) -> (usize, HashMap<String, (usize, usize)>) {
        let mut total = 0;
        let mut ranges = HashMap::default();
        for func in &self.functions {
            if func.is_async && Self::analyze_async_sm(func).is_some() {
                continue;
            }
            let start = total;
            total += count_any_add_sites_in(func);
            ranges.insert(func.name.clone(), (start, total));
        }
        (total, ranges)
    }

    /// One zero-init byte per specializable-binop site, consumed in order by
    /// `translate_binop.rs`. Read back by `retier()` and `profile::export_profile`.
    pub(super) fn generate_kind_history(&mut self) {
        if !self.profile {
            return;
        }
        let (n, ranges) = self.count_any_add_sites();
        self.any_add_site_ranges = ranges;
        self.any_add_site_ids = (0..n)
            .map(|i| {
                let mut data_ctx = DataDescription::new();
                data_ctx.define_zeroinit(1);
                let id = self
                    .module
                    .declare_data(
                        &format!("__olive_any_site${i}"),
                        Linkage::Local,
                        true,
                        false,
                    )
                    .unwrap();
                self.module.define_data(id, &data_ctx).unwrap();
                id
            })
            .collect();
    }
}

impl CraneliftCodegen<JITModule> {
    /// Nth (0-indexed, source order) site's recorded kind-history byte.
    pub(crate) fn any_add_site_kind(&mut self, index: usize) -> Option<u8> {
        let &id = self.any_add_site_ids.get(index)?;
        let bytes = self.module.get_finalized_data(id).0;
        Some(unsafe { *bytes })
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::{compile, compile_minimal};

    #[test]
    fn counts_only_any_typed_add_sites() {
        let cg = compile(concat!(
            "fn f(x: [Any]) -> Any:\n",
            "    let mut a = x[0]\n",
            "    let mut b = x[1]\n",
            "    let mut c = a + b\n",
            "    let mut d = c + a\n",
            "    return d\n",
            "\n",
            "fn g(x: i64, y: i64) -> i64:\n",
            "    return x + y\n",
        ));
        // 2 Any-typed adds in f, 0 in g (concrete i64 + i64 isn't an Any op).
        assert_eq!(cg.any_add_site_ids.len(), 2);
    }

    #[test]
    fn zero_sites_for_concrete_only_program() {
        let cg = compile("fn f(a: i64, b: i64) -> i64:\n    return a + b\n");
        assert_eq!(cg.any_add_site_ids.len(), 0);
    }

    #[test]
    fn counts_sites_for_every_specializable_op() {
        let cg = compile_minimal(concat!(
            "fn f(a: Any, b: Any) -> Any:\n",
            "    let s = a - b\n",
            "    let m = a * b\n",
            "    let d = a / b\n",
            "    let r = a % b\n",
            "    return s\n",
            "\n",
            "fn cmp(a: Any, b: Any) -> i64:\n",
            "    let lt = a < b\n",
            "    let le = a <= b\n",
            "    let gt = a > b\n",
            "    let ge = a >= b\n",
            "    let eq = a == b\n",
            "    let ne = a != b\n",
            "    return int(lt)\n",
        ));
        assert_eq!(cg.any_add_site_ids.len(), 10);
    }

    #[test]
    fn count_matches_method_directly() {
        let cg = compile("fn f(x: [Any]) -> Any:\n    return x[0] + x[1]\n");
        let (n, ranges) = cg.count_any_add_sites();
        assert_eq!(n, 1);
        assert_eq!(ranges.get("f"), Some(&(0, 1)));
        assert_eq!(cg.any_add_site_ids.len(), 1);
    }

    #[test]
    fn ranges_partition_sites_per_function() {
        let cg = compile_minimal(concat!(
            "fn f(a: Any, b: Any) -> Any:\n",
            "    return a + b\n",
            "\n",
            "fn g(a: Any, b: Any, c: Any) -> Any:\n",
            "    let x = a + b\n",
            "    return x + c\n",
        ));
        assert_eq!(cg.any_add_site_ids.len(), 3);
        assert_eq!(cg.any_add_site_ranges.get("f"), Some(&(0, 1)));
        assert_eq!(cg.any_add_site_ranges.get("g"), Some(&(1, 3)));
    }

    // Kind-history byte semantics (std_lib/src/lib.rs): 0..8 = still sampling,
    // N calls observed so far, all plain ints; 8 (ANY_SITE_SAMPLE_WINDOW) =
    // graduated all-int, stop recording; 254 (ANY_SITE_MIXED) = confirmed
    // mixed, terminal. A fixed sample window bounds recording cost to a
    // handful of calls per site instead of the whole program's lifetime.
    const SAMPLE_WINDOW: u8 = 8;
    const MIXED: u8 = 254;

    #[test]
    fn records_all_int_for_monomorphic_site() {
        use crate::test_utils::{call_i64_1, compile_minimal};
        // compile_minimal (no inlining): keeps `f`'s body as the one and only
        // textual site, so `f`'s calls are what actually update site 0. Under
        // the full optimizer, `f` gets inlined into `driver` and site 0 becomes
        // `f`'s now-dead original body instead -- also correct (each inlined
        // copy is its own genuine call site), just not what this test isolates.
        let mut cg = compile_minimal(concat!(
            "fn f(a: Any, b: Any) -> Any:\n",
            "    return a + b\n",
            "\n",
            "fn driver(n: i64) -> i64:\n",
            "    let r = f(n, n)\n",
            "    return int(r)\n",
        ));
        assert_eq!(cg.any_add_site_ids.len(), 1);
        assert_eq!(cg.any_add_site_kind(0), Some(0));
        for i in 0..3 {
            call_i64_1(&mut cg, "driver", i);
        }
        // Still sampling: 3 calls in, fewer than SAMPLE_WINDOW.
        assert_eq!(cg.any_add_site_kind(0), Some(3));
        for i in 0..SAMPLE_WINDOW as i64 {
            call_i64_1(&mut cg, "driver", i);
        }
        // Graduated: recording stops once SAMPLE_WINDOW consecutive all-int
        // calls land, however many more calls happen after.
        assert_eq!(cg.any_add_site_kind(0), Some(SAMPLE_WINDOW));
    }

    #[test]
    fn records_mixed_when_a_call_uses_a_non_int() {
        use crate::test_utils::{call_i64_1, compile_minimal};
        let mut cg = compile_minimal(concat!(
            "fn f(a: Any, b: Any) -> Any:\n",
            "    return a + b\n",
            "\n",
            "fn driver(use_str: i64) -> i64:\n",
            "    if use_str == 1:\n",
            "        let r = f(\"a\", \"b\")\n",
            "        return int(str(r) == \"ab\")\n",
            "    let r = f(1, 2)\n",
            "    return int(r)\n",
        ));
        assert_eq!(cg.any_add_site_ids.len(), 1);
        call_i64_1(&mut cg, "driver", 0);
        assert_eq!(cg.any_add_site_kind(0), Some(1)); // 1 all-int call sampled
        call_i64_1(&mut cg, "driver", 1);
        assert_eq!(cg.any_add_site_kind(0), Some(MIXED)); // locked once a str call lands
    }

    #[test]
    fn mixed_site_stays_mixed_regardless_of_later_calls() {
        use crate::test_utils::{call_i64_1, compile_minimal};
        let mut cg = compile_minimal(concat!(
            "fn f(a: Any, b: Any) -> Any:\n",
            "    return a + b\n",
            "\n",
            "fn driver(use_str: i64) -> i64:\n",
            "    if use_str == 1:\n",
            "        let r = f(\"a\", \"b\")\n",
            "        return int(str(r) == \"ab\")\n",
            "    let r = f(1, 2)\n",
            "    return int(r)\n",
        ));
        call_i64_1(&mut cg, "driver", 1);
        assert_eq!(cg.any_add_site_kind(0), Some(MIXED));
        for _ in 0..5 {
            call_i64_1(&mut cg, "driver", 0);
        }
        assert_eq!(cg.any_add_site_kind(0), Some(MIXED)); // terminal, never resets
    }
}
