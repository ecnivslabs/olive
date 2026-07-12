//! E2 flow-narrowing regression tests (roadmap.md Phase E2, steps 1-3).
#[cfg(test)]
use crate::test_utils::{call_i64, call_i64_1, call_i64_2, check_codes, compile};

#[test]
fn narrow_branch_form_arithmetic() {
    let mut cg = compile(
        "fn f(x: int | None) -> int:\n    if x != None:\n        return x + 1\n    return -1\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 41), 42);
    assert_eq!(call_i64_1(&mut cg, "f", 0), -1);
}

#[test]
fn narrow_branch_form_str_concat_len() {
    // `str` is heap-backed, unlike `int`; a plain `str | None` FFI parameter
    // can't be driven from a raw i64 test call, so the value is built
    // inside the function instead, keyed by a plain int flag.
    let mut cg = compile(
        "fn f(pick: int) -> int:\n    let mut s: str | None = None\n    if pick == 1:\n        s = \"hi\"\n    if s != None:\n        return len(s + \"!\")\n    return -1\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 1), 3);
    assert_eq!(call_i64_1(&mut cg, "f", 0), -1);
}

#[test]
fn narrow_else_branch() {
    let mut cg = compile(
        "fn f(x: int | None) -> int:\n    if x == None:\n        return -1\n    else:\n        return x + 1\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 9), 10);
    assert_eq!(call_i64_1(&mut cg, "f", 0), -1);
}

#[test]
fn narrow_guard_form_return() {
    let mut cg = compile(
        "fn f(x: int | None) -> int:\n    if x == None:\n        return -1\n    return x + 1\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 9), 10);
    assert_eq!(call_i64_1(&mut cg, "f", 0), -1);
}

#[test]
fn narrow_guard_form_continue() {
    let mut cg = compile(
        "fn f(n: int) -> int:\n    let mut total = 0\n    let mut i = 0\n    while i < n:\n        let x: int | None = i\n        i = i + 1\n        if x == None:\n            continue\n        total = total + x\n    return total\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 4), 1 + 2 + 3);
}

#[test]
fn narrow_and_chain() {
    // `int | None`'s runtime representation is the raw int with `0` as the
    // `None` sentinel, so a literal `0` argument drives the `None` case.
    let mut cg = compile(
        "fn f(a: int | None, b: int | None) -> int:\n    if a != None and b != None:\n        return a + b\n    return -1\n",
    );
    assert_eq!(call_i64_2(&mut cg, "f", 1, 2), 3);
    assert_eq!(call_i64_2(&mut cg, "f", 1, 0), -1);
    assert_eq!(call_i64_2(&mut cg, "f", 0, 2), -1);
}

#[test]
fn narrow_and_chain_same_var_field() {
    // Right side of `and` must see left side's narrow facts so field
    // access on the narrowed type compiles without E0428/E0404.
    let codes = check_codes(
        "struct Foo:\n    val: int\nfn f(x: Foo | None) -> int:\n    if x != None and int(x.val) == 0:\n        return 1\n    return -1\n",
    );
    assert!(
        !codes.contains(&"E0428".to_string()),
        "expected no E0428 (access on None), got {codes:?}"
    );
}

#[test]
fn narrow_not_wrap() {
    let mut cg = compile(
        "fn f(x: int | None) -> int:\n    if not (x == None):\n        return x + 1\n    return -1\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 5), 6);
    assert_eq!(call_i64_1(&mut cg, "f", 0), -1);
}

#[test]
fn narrow_reassignment_kills_fact() {
    // Reassigning inside the narrowed branch must invalidate the fact; a
    // later union-typed use inside the SAME branch is still `int | None`,
    // not silently `int`.
    let mut cg = compile(
        "fn f() -> int:\n    let mut x: int | None = 5\n    if x != None:\n        x = None\n        if x == None:\n            return 1\n        return 0\n    return -1\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn narrow_does_not_leak_past_branch() {
    // The narrow fact from the `then` branch must not survive into code
    // after the whole `if`/`else` when neither side diverges.
    let codes = check_codes(
        "fn f(x: int | None) -> int:\n    if x != None:\n        pass\n    else:\n        pass\n    return x + 1\n",
    );
    assert!(
        codes.contains(&"E0404".to_string()),
        "expected E0404 (union used outside narrowing), got {codes:?}"
    );
}

#[test]
fn narrow_elif_chain_not_narrowed() {
    // v1 scope: `elif` chains are not narrowed; a bare-union use in an elif
    // body is still a compile error, not silently accepted.
    let codes = check_codes(
        "fn f(x: int | None) -> int:\n    if x == 0:\n        return 0\n    elif x != None:\n        return x + 1\n    return -1\n",
    );
    assert!(
        codes.contains(&"E0404".to_string()),
        "expected E0404 (elif not narrowed in v1), got {codes:?}"
    );
}
