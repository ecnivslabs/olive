//! Regression tests: `str.to_int()`/`str.to_float()` share the same parsing
//! grammar as `int()`/`float()`'s runtime, but yield `None` on a parse
//! failure instead of panicking.
#[cfg(test)]
use crate::test_utils::{call_i64, check_codes, compile};

#[test]
fn to_int_parses_plain_digits() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let r = \"42\".to_int()\n",
        "    if r != None:\n",
        "        return r + 1\n",
        "    return -1\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 43);
}

#[test]
fn to_int_malformed_is_none() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let r = \"abc\".to_int()\n",
        "    if r == None:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn to_int_float_string_is_none() {
    // "1.5".to_int() is None, not an error -- a float string doesn't parse as int.
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let r = \"1.5\".to_int()\n",
        "    if r == None:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn to_int_overflow_is_none() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let r = \"99999999999999999999\".to_int()\n",
        "    if r == None:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn to_int_empty_is_none() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let r = \"\".to_int()\n",
        "    if r == None:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn to_int_leading_plus_and_whitespace_match_int_builtin() {
    // Whitespace tolerance and a `+` prefix must match `int()`'s own
    // grammar exactly, not a separately-decided one.
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let a = \"  7  \".to_int()\n",
        "    let b = \"+7\".to_int()\n",
        "    if a != None and b != None and a == 7 and b == 7:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn to_float_parses_decimal() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let r = \"3.14\".to_float()\n",
        "    if r != None:\n",
        "        let diff = r - 3.14\n",
        "        if diff > -0.0001 and diff < 0.0001:\n",
        "            return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn to_float_malformed_is_none() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let r = \"xyz\".to_float()\n",
        "    if r == None:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn to_int_coalesce_idiom() {
    // `s.to_int() ?? 0` (E7.2's idiom set).
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    return \"abc\".to_int() ?? 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 0);
}

#[test]
fn to_float_wrong_arity_is_clean_rejection() {
    let codes = check_codes("fn f():\n    \"1.5\".to_float(2)\n");
    assert_eq!(codes, vec!["E0403"]);
}

#[test]
fn to_int_match_idiom() {
    // `match` on `int | None` (E7.2's idiom set). A bound catch-all arm
    // isn't itself narrowed to `int` yet (full match-arm narrowing is
    // E12); guard inside the arm the same way E2's `if` narrowing does.
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    match \"5\".to_int():\n",
        "        None:\n",
        "            return -1\n",
        "        n:\n",
        "            if n != None:\n",
        "                return n * 2\n",
        "            return -2\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 10);
}

#[test]
fn to_int_guard_narrowing_idiom() {
    // E2's guard-form narrowing on a `.to_int()` result.
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let r = \"5\".to_int()\n",
        "    if r == None:\n",
        "        return -1\n",
        "    return r + 1\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 6);
}

#[test]
fn try_on_to_int_is_a_passthrough() {
    // `try` propagates the *error* variant of `T | Error` (the design
    // decision is explicit: "the `?` family is the absence grammar, three
    // forms, one model" -- `?`/`try` for errors, `??` fills `None`, `?.`
    // skips `None`). `None` is not an error, so `try` on a `T | None`
    // value is a harmless passthrough, not early-return propagation.
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let r = try \"5\".to_int()\n",
        "    if r == None:\n",
        "        return -1\n",
        "    return r\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 5);
}
