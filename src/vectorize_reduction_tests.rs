//! Correctness tests for vectorized integer reductions: the folded result
//! must match the scalar sum for every trip-count shape, including counts
//! below the vector width and remainders the scalar epilogue handles.

#[cfg(test)]
use crate::test_utils::{call_i64_1, compile};

#[cfg(test)]
const SUM_SRC: &str = concat!(
    "fn f(n: i64) -> i64:\n",
    "    let mut xs: [int] = list_new(n)\n",
    "    let mut i = 0\n",
    "    while i < n:\n",
    "        xs[i] = i * 3 + 1\n",
    "        i = i + 1\n",
    "    let mut sum = 0\n",
    "    i = 0\n",
    "    while i < n:\n",
    "        sum = sum + xs[i]\n",
    "        i = i + 1\n",
    "    return sum\n",
);

#[cfg(test)]
fn scalar_sum(n: i64) -> i64 {
    (0..n).map(|i| i * 3 + 1).sum()
}

#[test]
fn reduction_sum_matches_scalar_across_trip_counts() {
    let mut cg = compile(SUM_SRC);
    for n in [0, 1, 2, 3, 4, 5, 7, 8, 15, 16, 17, 1000, 1001] {
        assert_eq!(call_i64_1(&mut cg, "f", n), scalar_sum(n), "n={n}");
    }
}

#[test]
fn reduction_product_matches_scalar() {
    let src = concat!(
        "fn f(n: i64) -> i64:\n",
        "    let mut xs: [int] = list_new(n)\n",
        "    let mut i = 0\n",
        "    while i < n:\n",
        "        xs[i] = i + 1\n",
        "        i = i + 1\n",
        "    let mut p = 1\n",
        "    i = 0\n",
        "    while i < n:\n",
        "        p = p * xs[i]\n",
        "        i = i + 1\n",
        "    return p\n",
    );
    let mut cg = compile(src);
    for n in [0, 1, 2, 3, 5, 10, 20] {
        let expected: i64 = (1..=n).product();
        assert_eq!(call_i64_1(&mut cg, "f", n), expected, "n={n}");
    }
}

#[test]
fn reduction_accumulator_seed_survives() {
    let src = concat!(
        "fn f(n: i64) -> i64:\n",
        "    let mut xs: [int] = list_new(n)\n",
        "    let mut i = 0\n",
        "    while i < n:\n",
        "        xs[i] = 2\n",
        "        i = i + 1\n",
        "    let mut sum = 100\n",
        "    i = 0\n",
        "    while i < n:\n",
        "        sum = sum + xs[i]\n",
        "        i = i + 1\n",
        "    return sum\n",
    );
    let mut cg = compile(src);
    for n in [0, 1, 5, 64, 101] {
        assert_eq!(call_i64_1(&mut cg, "f", n), 100 + 2 * n, "n={n}");
    }
}

#[test]
fn two_reductions_in_one_loop() {
    let src = concat!(
        "fn f(n: i64) -> i64:\n",
        "    let mut xs: [int] = list_new(n)\n",
        "    let mut i = 0\n",
        "    while i < n:\n",
        "        xs[i] = i\n",
        "        i = i + 1\n",
        "    let mut a = 0\n",
        "    let mut b = 0\n",
        "    i = 0\n",
        "    while i < n:\n",
        "        a = a + xs[i]\n",
        "        b = b + xs[i]\n",
        "        i = i + 1\n",
        "    return a * 1000000 + b\n",
    );
    let mut cg = compile(src);
    for n in [0, 3, 10, 33] {
        let s: i64 = (0..n).sum();
        assert_eq!(call_i64_1(&mut cg, "f", n), s * 1_000_000 + s, "n={n}");
    }
}
