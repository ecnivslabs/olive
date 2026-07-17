//! `**` power operator regression tests.
#[cfg(test)]
use crate::test_utils::{call_i64, compile};

#[test]
fn negative_base_binds_looser_than_power() {
    let mut cg = compile("fn f() -> int:\n    -2 ** 2\n");
    assert_eq!(call_i64(&mut cg, "f"), -4);
}

#[test]
fn right_associative() {
    let mut cg = compile("fn f() -> int:\n    2 ** 3 ** 2\n");
    assert_eq!(call_i64(&mut cg, "f"), 512);
}

#[test]
fn int_power() {
    let mut cg = compile("fn f() -> int:\n    2 ** 10\n");
    assert_eq!(call_i64(&mut cg, "f"), 1024);
}

#[test]
fn power_of_zero_is_one() {
    let mut cg = compile("fn f() -> int:\n    3 ** 0\n");
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn float_power() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let x = 2.0 ** 10.0\n",
        "    if x == 1024.0:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}
