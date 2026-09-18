#[path = "support/program.rs"]
mod program;
use program::assert_both;

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
