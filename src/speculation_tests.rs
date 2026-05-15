//! Differential correctness testing for JIT tier-up/specialization: a
//! tiered/specialized function must match an untiered baseline on every input.

#[cfg(test)]
mod speculation_proptests {
    use crate::test_utils::{call_i64_2, compile_minimal, compile_minimal_unprofiled};
    use proptest::prelude::*;

    // Must route through `driver`, not call `sum_loop` directly (bypasses the
    // dispatch cell) -- and `compile_minimal`, not `compile` (inliner defeats retiering).
    const SRC: &str = concat!(
        "fn sum_loop(start: i64, n: i64) -> i64:\n",
        "    let mut a: Any = start\n",
        "    let mut i = 0\n",
        "    while i < n:\n",
        "        let b: Any = 1\n",
        "        a = a + b\n",
        "        i = i + 1\n",
        "    return int(a)\n",
        "\n",
        "fn driver(start: i64, n: i64) -> i64:\n",
        "    return sum_loop(start, n)\n",
    );

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(40))]

        /// A retiered (specialized) instance must agree with a never-tiered
        /// baseline across the input space the guard actually branches on:
        /// comfortably in-range ints (always fast path), values straddling the
        /// 61-bit `TAG_INT` boundary (guard's overflow arm), and values already
        /// heap-boxed from the start (guard always misses, always falls back).
        /// `start` is a runtime argument, not baked into source, so both
        /// instances compile once and every case is just two cheap calls.
        #[test]
        fn tiered_matches_baseline_across_int_boundary(
            start in prop_oneof![
                -1000i64..1000i64,
                ((1i64 << 60) - 20)..((1i64 << 60) + 20),
                (i64::MIN / 2)..(i64::MIN / 2 + 40),
            ],
            iterations in 0i64..25,
        ) {
            let mut baseline = compile_minimal_unprofiled(SRC);
            let mut tiered = compile_minimal(SRC);
            // Warm up past the sample window (8) and retier before the real
            // per-case calls, so every case below actually exercises the
            // specialized path where the guard applies. The warmup call can
            // stay direct (kind-history updates whenever sum_loop's own
            // compiled body runs, regardless of caller) -- it's only the
            // *verification* calls below that must route through `driver` to
            // actually observe the retiered body.
            call_i64_2(&mut tiered, "sum_loop", 0, 20);
            prop_assert!(tiered.retier("sum_loop"));

            let expected = call_i64_2(&mut baseline, "driver", start, iterations);
            let actual = call_i64_2(&mut tiered, "driver", start, iterations);
            prop_assert_eq!(actual, expected, "start={} iterations={}", start, iterations);
        }
    }

    /// Same property, but the site is fed a long mixed sequence of magnitudes
    /// through repeated calls to the *same* retiered instance -- checks the
    /// guard's per-call correctness holds up under sustained reuse, not just a
    /// single call after retier, since the dispatch cell and specialized body
    /// stay live for the rest of the process.
    #[test]
    fn tiered_matches_baseline_across_repeated_calls_same_instance() {
        let mut baseline = compile_minimal_unprofiled(SRC);
        let mut tiered = compile_minimal(SRC);
        call_i64_2(&mut tiered, "sum_loop", 0, 20);
        assert!(tiered.retier("sum_loop"));

        let cases: &[(i64, i64)] = &[
            (0, 5),
            (1i64 << 60, 3),
            ((1i64 << 60) - 3, 10),
            (-(1i64 << 60), 7),
            (i64::MIN / 4, 12),
            (42, 0),
            (i64::MAX / 3, 1),
        ];
        for &(start, iterations) in cases {
            let expected = call_i64_2(&mut baseline, "driver", start, iterations);
            let actual = call_i64_2(&mut tiered, "driver", start, iterations);
            assert_eq!(actual, expected, "start={start} iterations={iterations}");
        }
    }
}

/// Same discipline, extended past `+` to `- * / %` and the six comparisons.
/// `*` needs a widening-multiply check; a naive one can silently wrap.
#[cfg(test)]
mod any_binop_specialization_proptests {
    use crate::test_utils::{call_i64_2, compile_minimal, compile_minimal_unprofiled};
    use proptest::prelude::*;

    const INT_MAX: i64 = (1i64 << 60) - 1;
    const INT_MIN: i64 = -(1i64 << 60);

    // `op_test` is Any on both sides; verify through `driver`, not directly.
    fn binop_src(expr: &str) -> String {
        format!(
            "fn op_test(a: Any, b: Any) -> i64:\n    return int({expr})\n\nfn driver(a: i64, b: i64) -> i64:\n    return op_test(a, b)\n"
        )
    }

    fn prepare(
        expr: &str,
    ) -> (
        crate::codegen::cranelift::CraneliftCodegen<cranelift_jit::JITModule>,
        crate::codegen::cranelift::CraneliftCodegen<cranelift_jit::JITModule>,
    ) {
        let src = binop_src(expr);
        let baseline = compile_minimal_unprofiled(&src);
        let mut tiered = compile_minimal(&src);
        for i in 0..12i64 {
            call_i64_2(&mut tiered, "op_test", i, i + 1);
        }
        assert!(tiered.retier("op_test"));
        (baseline, tiered)
    }

    macro_rules! binop_proptest {
        ($test_name:ident, $expr:literal, $a:expr, $b:expr) => {
            proptest! {
                #![proptest_config(ProptestConfig::with_cases(200))]
                #[test]
                fn $test_name(a in $a, b in $b) {
                    let (mut baseline, mut tiered) = prepare($expr);
                    let expected = call_i64_2(&mut baseline, "driver", a, b);
                    let actual = call_i64_2(&mut tiered, "driver", a, b);
                    prop_assert_eq!(actual, expected, "a={} b={}", a, b);
                }
            }
        };
    }

    // Includes values straddling the 61-bit boundary (overflow-fallback arm).
    binop_proptest!(
        sub_matches_baseline,
        "a - b",
        prop_oneof![-1000i64..1000i64, (INT_MIN + 1)..(INT_MIN + 40)],
        prop_oneof![-1000i64..1000i64, (INT_MAX - 40)..INT_MAX]
    );

    binop_proptest!(
        div_matches_baseline,
        "a / b",
        prop_oneof![
            -1000i64..1000i64,
            INT_MIN..(INT_MIN + 20),
            (INT_MAX - 20)..INT_MAX
        ],
        prop_oneof![-100i64..100i64, Just(0i64)]
    );

    binop_proptest!(
        mod_matches_baseline,
        "a % b",
        prop_oneof![
            -1000i64..1000i64,
            INT_MIN..(INT_MIN + 20),
            (INT_MAX - 20)..INT_MAX
        ],
        prop_oneof![-100i64..100i64, Just(0i64)]
    );

    binop_proptest!(
        lt_matches_baseline,
        "a < b",
        -1000i64..1000i64,
        -1000i64..1000i64
    );
    binop_proptest!(
        le_matches_baseline,
        "a <= b",
        -1000i64..1000i64,
        -1000i64..1000i64
    );
    binop_proptest!(
        gt_matches_baseline,
        "a > b",
        -1000i64..1000i64,
        -1000i64..1000i64
    );
    binop_proptest!(
        ge_matches_baseline,
        "a >= b",
        -1000i64..1000i64,
        -1000i64..1000i64
    );
    binop_proptest!(eq_matches_baseline, "a == b", -5i64..5i64, -5i64..5i64);
    binop_proptest!(ne_matches_baseline, "a != b", -5i64..5i64, -5i64..5i64);

    // Adversarial ranges: products that overflow i64 itself, not just the 61-bit range.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]

        #[test]
        fn mul_matches_baseline(
            a in prop_oneof![
                -1000i64..1000i64,
                (INT_MAX / 2)..INT_MAX,
                INT_MIN..(INT_MIN / 2),
            ],
            b in prop_oneof![
                -1000i64..1000i64,
                (INT_MAX / 2)..INT_MAX,
                INT_MIN..(INT_MIN / 2),
            ],
        ) {
            let (mut baseline, mut tiered) = prepare("a * b");
            let expected = call_i64_2(&mut baseline, "driver", a, b);
            let actual = call_i64_2(&mut tiered, "driver", a, b);
            prop_assert_eq!(actual, expected, "a={} b={}", a, b);
        }
    }

    #[test]
    fn mul_overflow_edge_cases_match_baseline() {
        let (mut baseline, mut tiered) = prepare("a * b");
        let cases: &[(i64, i64)] = &[
            (INT_MAX, INT_MAX),
            (INT_MIN, INT_MIN),
            (INT_MAX, INT_MIN),
            (INT_MAX, 2),
            (INT_MIN, 2),
            (INT_MAX, -1),
            (INT_MIN, -1),
            (1i64 << 59, 1i64 << 59),
            (1i64 << 50, 1i64 << 50),
            (INT_MAX, INT_MAX / 2),
        ];
        for &(a, b) in cases {
            let expected = call_i64_2(&mut baseline, "driver", a, b);
            let actual = call_i64_2(&mut tiered, "driver", a, b);
            assert_eq!(actual, expected, "a={a} b={b}");
        }
    }

    #[test]
    fn div_mod_by_zero_matches_baseline() {
        for expr in ["a / b", "a % b"] {
            let (mut baseline, mut tiered) = prepare(expr);
            for a in [0i64, 1, -1, 42, INT_MAX, INT_MIN] {
                let expected = call_i64_2(&mut baseline, "driver", a, 0);
                let actual = call_i64_2(&mut tiered, "driver", a, 0);
                assert_eq!(actual, expected, "expr={expr} a={a} b=0");
            }
        }
    }

    #[test]
    fn comparisons_match_baseline_at_boundaries() {
        let ops = ["a < b", "a <= b", "a > b", "a >= b", "a == b", "a != b"];
        let cases: &[(i64, i64)] = &[
            (0, 0),
            (INT_MAX, INT_MAX),
            (INT_MIN, INT_MIN),
            (INT_MAX, INT_MIN),
            (INT_MIN, INT_MAX),
            (-1, 0),
            (0, -1),
            (INT_MAX, INT_MAX - 1),
            (INT_MIN, INT_MIN + 1),
        ];
        for expr in ops {
            let (mut baseline, mut tiered) = prepare(expr);
            for &(a, b) in cases {
                let expected = call_i64_2(&mut baseline, "driver", a, b);
                let actual = call_i64_2(&mut tiered, "driver", a, b);
                assert_eq!(actual, expected, "expr={expr} a={a} b={b}");
            }
        }
    }
}
