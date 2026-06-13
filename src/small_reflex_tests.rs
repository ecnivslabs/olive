//! Small reflex regression tests.
#[cfg(test)]
use crate::test_utils::{call_i64, check_codes, compile};

#[test]
fn int_literal_coerces_to_float_in_let() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let y: float = 5\n",
        "    if y == 5.0:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn int_literal_coerces_to_float_in_call_arg() {
    let mut cg = compile(concat!(
        "fn g(x: float) -> float:\n",
        "    return x\n",
        "fn f() -> int:\n",
        "    if g(3) == 3.0:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn negative_int_literal_coerces_to_float() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let y: float = -5\n",
        "    if y == -5.0:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn int_in_stepless_range() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    if 5 in 0..10 and 15 not in 0..10 and 10 in 0..=10 and 10 not in 0..10:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn in_range_still_works_alongside_normal_in() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    if 2 in [1, 2, 3] and 5 in 0..10:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn str_in_int_range_is_a_clean_rejection() {
    let codes = check_codes("fn f():\n    \"x\" in 0..10\n");
    assert_eq!(codes, vec!["E0404"]);
}

#[test]
fn fstring_debug_form() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let x = 5\n",
        "    if f\"{x=}\" == \"x=5\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn fstring_debug_form_with_spec() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let y = 3.14159\n",
        "    if f\"{y=:.2f}\" == \"y=3.14\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn fstring_equality_comparison_not_mistaken_for_debug_form() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let a = 1\n",
        "    let b = 2\n",
        "    if f\"{a == b}\" == \"False\" and f\"{a != b}\" == \"True\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn struct_construction_with_field_keywords() {
    let mut cg = compile(concat!(
        "struct User:\n",
        "    email: str\n",
        "    username: str\n",
        "\n",
        "fn f() -> int:\n",
        "    let u = User(email=\"e@x.com\", username=\"u1\")\n",
        "    if u.email == \"e@x.com\" and u.username == \"u1\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}
