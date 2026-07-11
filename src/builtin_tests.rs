#[cfg(test)]
use crate::test_utils::{call_i64, compile};

#[test]
fn abs_negative_int() {
    let mut cg = compile("fn f() -> int:\n    abs(-5)\n");
    assert_eq!(call_i64(&mut cg, "f"), 5);
}

#[test]
fn abs_positive_int() {
    let mut cg = compile("fn f() -> int:\n    abs(3)\n");
    assert_eq!(call_i64(&mut cg, "f"), 3);
}

#[test]
fn abs_zero() {
    let mut cg = compile("fn f() -> int:\n    abs(0)\n");
    assert_eq!(call_i64(&mut cg, "f"), 0);
}

#[test]
fn abs_variable() {
    let mut cg = compile("fn f() -> int:\n    let x = -7\n    abs(x)\n");
    assert_eq!(call_i64(&mut cg, "f"), 7);
}

#[test]
fn round_down() {
    let mut cg = compile("fn f() -> int:\n    round(3.2)\n");
    assert_eq!(call_i64(&mut cg, "f"), 3);
}

#[test]
fn round_up() {
    let mut cg = compile("fn f() -> int:\n    round(3.8)\n");
    assert_eq!(call_i64(&mut cg, "f"), 4);
}

#[test]
fn round_half() {
    let mut cg = compile("fn f() -> int:\n    round(2.5)\n");
    assert_eq!(call_i64(&mut cg, "f"), 3);
}

#[test]
fn round_negative() {
    let mut cg = compile("fn f() -> int:\n    round(-3.8)\n");
    assert_eq!(call_i64(&mut cg, "f"), -4);
}

#[test]
fn round_variable() {
    let mut cg = compile("fn f() -> int:\n    let x = 4.49\n    round(x)\n");
    assert_eq!(call_i64(&mut cg, "f"), 4);
}
