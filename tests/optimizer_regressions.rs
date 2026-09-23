#[path = "support/program.rs"]
mod program;
use program::{assert_both, assert_fault_both};

#[test]
fn constant_negation_of_min_does_not_abort_compiler() {
    assert_both(
        r#"fn main():
    let x = 1 << 63
    let y = -x
    print(y)
"#,
        "-9223372036854775808\n",
    );
}

#[test]
fn algebraic_factoring_does_not_use_stale_values() {
    assert_both(
        r#"fn f(a: int, b: int) -> int:
    let mut y = a
    let x = y * 8
    y = b
    return x / 4

fn main():
    print(f(3, 5))
"#,
        "6\n",
    );
}

#[test]
fn loop_unroll_preserves_mutable_bound() {
    assert_both(
        r#"fn main():
    let mut limit = 10
    let mut i = 0
    let mut count = 0
    while i < limit:
        count = count + 1
        limit = limit - 3
        i = i + 1
    print(count)
"#,
        "3\n",
    );
}

#[test]
fn huge_step_loop_is_not_wrapped_by_optimizer() {
    assert_both(
        r#"fn main():
    let mut i = 0
    let mut count = 0
    while i < 9223372036854775807:
        count = count + 1
        i = i + 9223372036854775807
    print(count)
"#,
        "1\n",
    );
}

#[test]
fn dynamic_zero_trip_loop_avoids_preheader_arithmetic() {
    assert_both(
        r#"fn count(limit: int) -> int:
    let mut i = 0
    let mut result = 0
    while i < limit:
        result = result + 1
        i = i + 1
    return result

fn main():
    print(count(-9223372036854775807 - 1))
"#,
        "0\n",
    );
}

#[test]
fn zero_trip_loop_is_not_partially_unrolled() {
    assert_both(
        r#"fn main():
    let mut i = 0
    let mut count = 0
    while i < -9223372036854775807:
        count = count + 1
        i = i + 1
    print(count)
"#,
        "0\n",
    );
}

#[test]
fn vectorization_never_removes_counted_loop_bounds_checks() {
    assert_fault_both(
        r#"fn main():
    let xs = [10, 20, 30, 40]
    let mut total = 0
    for i in 0..8:
        total = total + xs[i]
    print(total)
"#,
        "[E0701]",
        "the index is 4",
    );
}

#[test]
fn licm_does_not_hoist_division_before_zero_trip_guard() {
    assert_both(
        r#"fn count(n: int) -> int:
    let mut i = 0
    let mut result = 0
    while i < n:
        result = 1 / 0
        i = i + 1
    return result

fn main():
    print(count(0))
"#,
        "0\n",
    );
}

#[test]
fn licm_does_not_hoist_bounds_check_before_zero_trip_guard() {
    assert_both(
        r#"fn count(limit: int, xs: [int]) -> int:
    let mut i = 0
    while i < limit:
        let value = xs[1]
        i = i + value + 1
    return 0

fn main():
    print(count(-1, [1]))
"#,
        "0\n",
    );
}

#[test]
fn narrow_element_loop_remains_correct_without_vectorization() {
    assert_both(
        r#"fn main():
    let xs: [u8] = [1, 2, 3, 4]
    let mut total = 0
    let mut i = 0
    while i < len(xs):
        total = total + xs[i]
        i = i + 1
    print(total)
"#,
        "10\n",
    );
}

#[test]
fn bounds_elimination_keeps_mutable_cached_length_checked() {
    assert_fault_both(
        r#"fn f(xs: [int]) -> int:
    let mut n = len(xs)
    let mut i = 0
    let mut total = 0
    while i < n:
        n = 100
        total = total + xs[i]
        i = i + 1
    return total

fn main():
    print(f([1, 2]))
"#,
        "[E0701]",
        "index",
    );
}

#[test]
fn bounds_elimination_tracks_aliases_that_can_shrink() {
    assert_fault_both(
        r#"fn run(n: int) -> int:
    let mut xs = list_new(n)
    let mut ys = xs
    let mut i = 0
    let mut total = 0
    while i < n:
        total = total + xs[i]
        ys.pop()
        i = i + 1
    return total

fn main():
    print(run(2))
"#,
        "[E0701]",
        "index",
    );
}

#[test]
fn bounds_elimination_requires_the_guard_to_dominate_the_access() {
    assert_fault_both(
        r#"fn main():
    let xs = [7]
    let mut i = 0
    while (xs[i] == 7) | (i < len(xs)):
        i = i + 1
    print("done")
"#,
        "[E0701]",
        "index",
    );
}
