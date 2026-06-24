//! Flow-narrowing regression tests.
//!
//! Scalar `T | None` is Any-tag encoded at runtime (see `boxed.rs`): a raw
//! i64 cannot be passed straight into a union parameter from the test
//! harness, so drivers build the union in-language from plain scalars.
#[cfg(test)]
use crate::test_utils::{call_i64, call_i64_1, call_i64_2, check_codes, compile};

#[test]
fn narrow_branch_form_arithmetic() {
    let mut cg = compile(concat!(
        "fn f(x: int | None) -> int:\n    if x != None:\n        return x + 1\n    return -1\n",
        "fn g(n: int) -> int:\n    let x: int | None = n\n    return f(x)\n",
        "fn g_none() -> int:\n    let x: int | None = None\n    return f(x)\n",
    ));
    assert_eq!(call_i64_1(&mut cg, "g", 41), 42);
    assert_eq!(call_i64_1(&mut cg, "g", 0), 1);
    assert_eq!(call_i64(&mut cg, "g_none"), -1);
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
    let mut cg = compile(concat!(
        "fn f(x: int | None) -> int:\n    if x == None:\n        return -1\n    else:\n        return x + 1\n",
        "fn g(n: int) -> int:\n    let x: int | None = n\n    return f(x)\n",
        "fn g_none() -> int:\n    let x: int | None = None\n    return f(x)\n",
    ));
    assert_eq!(call_i64_1(&mut cg, "g", 9), 10);
    assert_eq!(call_i64_1(&mut cg, "g", 0), 1);
    assert_eq!(call_i64(&mut cg, "g_none"), -1);
}

#[test]
fn narrow_guard_form_return() {
    let mut cg = compile(concat!(
        "fn f(x: int | None) -> int:\n    if x == None:\n        return -1\n    return x + 1\n",
        "fn g(n: int) -> int:\n    let x: int | None = n\n    return f(x)\n",
        "fn g_none() -> int:\n    let x: int | None = None\n    return f(x)\n",
    ));
    assert_eq!(call_i64_1(&mut cg, "g", 9), 10);
    assert_eq!(call_i64_1(&mut cg, "g", 0), 1);
    assert_eq!(call_i64(&mut cg, "g_none"), -1);
}

#[test]
fn narrow_guard_form_continue() {
    let mut cg = compile(
        "fn f(n: int) -> int:\n    let mut total = 0\n    let mut i = 0\n    while i < n:\n        let x: int | None = i\n        i = i + 1\n        if x == None:\n            continue\n        total = total + x\n    return total\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 4), 6);
}

#[test]
fn narrow_and_chain() {
    // A real 0 is a value, not None: only the explicit None drivers take
    // the -1 path.
    let mut cg = compile(concat!(
        "fn f(a: int | None, b: int | None) -> int:\n    if a != None and b != None:\n        return a + b\n    return -1\n",
        "fn g(a: int, b: int) -> int:\n    let ua: int | None = a\n    let ub: int | None = b\n    return f(ua, ub)\n",
        "fn g_left_none(b: int) -> int:\n    let ub: int | None = b\n    return f(None, ub)\n",
        "fn g_right_none(a: int) -> int:\n    let ua: int | None = a\n    return f(ua, None)\n",
    ));
    assert_eq!(call_i64_2(&mut cg, "g", 1, 2), 3);
    assert_eq!(call_i64_2(&mut cg, "g", 0, 2), 2);
    assert_eq!(call_i64_1(&mut cg, "g_left_none", 2), -1);
    assert_eq!(call_i64_1(&mut cg, "g_right_none", 1), -1);
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
    let mut cg = compile(concat!(
        "fn f(x: int | None) -> int:\n    if not (x == None):\n        return x + 1\n    return -1\n",
        "fn g(n: int) -> int:\n    let x: int | None = n\n    return f(x)\n",
        "fn g_none() -> int:\n    let x: int | None = None\n    return f(x)\n",
    ));
    assert_eq!(call_i64_1(&mut cg, "g", 5), 6);
    assert_eq!(call_i64_1(&mut cg, "g", 0), 1);
    assert_eq!(call_i64(&mut cg, "g_none"), -1);
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

#[test]
fn narrow_zero_is_not_none() {
    // The historical scalar-union defect: a computed 0 in an `int | None`
    // read back as None. The tagged encoding keeps them distinct.
    let mut cg = compile(concat!(
        "fn identity(n: int) -> int | None:\n    return n\n",
        "fn f() -> int:\n    let x = identity(5 - 5)\n    if x == None:\n        return -1\n    return x ?? -2\n",
        "fn g() -> int:\n    let x: int | None = None\n    return x ?? -1\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 0);
    assert_eq!(call_i64(&mut cg, "g"), -1);
}

#[test]
fn narrow_float_union_zero_is_not_none() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let x: float | None = 0.0\n",
        "    if x == None:\n",
        "        return -1\n",
        "    if x != None:\n",
        "        if x < 0.5:\n",
        "            return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn narrow_float_union_decodes_value() {
    // Narrowing a `float | None` must decode the tagged word back to the
    // original float value, not reinterpret or numerically convert bits.
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let x: float | None = 3.14\n",
        "    if x != None:\n",
        "        let diff = x - 3.14\n",
        "        if diff > -0.0001 and diff < 0.0001:\n",
        "            return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn narrow_float_union_parameter_decodes_value() {
    let mut cg = compile(concat!(
        "fn check(x: float | None) -> int:\n",
        "    if x != None:\n",
        "        let diff = x - 2.5\n",
        "        if diff > -0.0001 and diff < 0.0001:\n",
        "            return 1\n",
        "    return 0\n",
        "fn f() -> int:\n",
        "    return check(2.5)\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn mixed_union_literal_match_is_kind_safe() {
    // The historical mixed-union defect: a str literal arm raw-compared
    // against an int word and dereferenced it as a string pointer.
    let mut cg = compile(concat!(
        "fn m(v: int | str) -> int:\n    match v:\n        case \"hi\":\n            return 1\n        case 0:\n            return 2\n        case _:\n            return 3\n",
        "fn f_int() -> int:\n    let v: int | str = 7\n    return m(v)\n",
        "fn f_zero() -> int:\n    let v: int | str = 0\n    return m(v)\n",
        "fn f_hi() -> int:\n    let v: int | str = \"hi\"\n    return m(v)\n",
        "fn f_other() -> int:\n    let v: int | str = \"yo\"\n    return m(v)\n",
    ));
    assert_eq!(call_i64(&mut cg, "f_int"), 3);
    assert_eq!(call_i64(&mut cg, "f_zero"), 2);
    assert_eq!(call_i64(&mut cg, "f_hi"), 1);
    assert_eq!(call_i64(&mut cg, "f_other"), 3);
}

#[test]
fn mixed_union_eq_never_parses_strings() {
    // Strict union equality: an int member never equals a numeric string.
    let mut cg = compile(
        "fn f() -> int:\n    let a: int | str = 7\n    let b: int | str = \"7\"\n    if a == \"7\":\n        return -1\n    if b == 7:\n        return -2\n    if a == b:\n        return -3\n    if a == 7 and b == \"7\":\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn mixed_union_none_zero_empty_distinct() {
    let mut cg = compile(concat!(
        "fn m(v: int | str | None) -> int:\n    match v:\n        case None:\n            return 1\n        case \"\":\n            return 2\n        case 0:\n            return 3\n        case _:\n            return 4\n",
        "fn f_none() -> int:\n    let v: int | str | None = None\n    return m(v)\n",
        "fn f_empty() -> int:\n    let v: int | str | None = \"\"\n    return m(v)\n",
        "fn f_zero() -> int:\n    let v: int | str | None = 0\n    return m(v)\n",
        "fn f_str() -> int:\n    let v: int | str | None = \"x\"\n    return m(v)\n",
    ));
    assert_eq!(call_i64(&mut cg, "f_none"), 1);
    assert_eq!(call_i64(&mut cg, "f_empty"), 2);
    assert_eq!(call_i64(&mut cg, "f_zero"), 3);
    assert_eq!(call_i64(&mut cg, "f_str"), 4);
}

#[test]
fn struct_union_none_zero_struct_distinct() {
    // The historical struct-union defect: a 1-field struct's header word
    // read as KIND_LIST on free, and int 0 read as None.
    let mut cg = compile(concat!(
        "struct Dog:\n    name: str\n",
        "fn m(v: int | Dog | None) -> int:\n    match v:\n        case None:\n            return 0\n        case 0:\n            return 1\n        case _:\n            return 2\n",
        "fn f_zero() -> int:\n    let v: int | Dog | None = 0\n    return m(v)\n",
        "fn f_none() -> int:\n    let v: int | Dog | None = None\n    return m(v)\n",
        "fn f_dog() -> int:\n    let v: int | Dog | None = Dog(\"rex\")\n    return m(v)\n",
    ));
    assert_eq!(call_i64(&mut cg, "f_zero"), 1);
    assert_eq!(call_i64(&mut cg, "f_none"), 0);
    assert_eq!(call_i64(&mut cg, "f_dog"), 2);
}

#[test]
fn struct_union_eq_is_content_equality() {
    let mut cg = compile(concat!(
        "struct Dog:\n    name: str\n",
        "fn f() -> int:\n    let a: int | Dog = Dog(\"rex\")\n    let b: int | Dog = Dog(\"rex\")\n    let c: int | Dog = Dog(\"max\")\n    let d: int | Dog = 5\n    if a == c:\n        return -1\n    if a == d:\n        return -2\n    if a == b and d == 5:\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn struct_only_union_drops_safely() {
    // Dog has 1 field, Cat also 1: raw headers would both misread as
    // KIND_LIST when freed through the erased union path.
    let mut cg = compile(concat!(
        "struct Dog:\n    name: str\n",
        "struct Cat:\n    lives: int\n",
        "fn f(n: int) -> int:\n    let mut i = 0\n    while i < n:\n        let v: Dog | Cat = Dog(\"a\")\n        let w: Dog | Cat = Cat(9)\n        i = i + 1\n    return i\n",
    ));
    assert_eq!(call_i64_1(&mut cg, "f", 500), 500);
}

#[test]
fn list_union_zero_is_not_none() {
    let mut cg = compile(concat!(
        "fn m(v: int | [int] | None) -> int:\n    if v == None:\n        return -1\n    return 1\n",
        "fn f_zero() -> int:\n    let v: int | [int] | None = 0\n    return m(v)\n",
        "fn f_none() -> int:\n    let v: int | [int] | None = None\n    return m(v)\n",
        "fn f_list() -> int:\n    let v: int | [int] | None = [1, 2]\n    return m(v)\n",
    ));
    assert_eq!(call_i64(&mut cg, "f_zero"), 1);
    assert_eq!(call_i64(&mut cg, "f_none"), -1);
    assert_eq!(call_i64(&mut cg, "f_list"), 1);
}

#[test]
fn mixed_union_float_str_distinct() {
    let mut cg = compile(concat!(
        "fn m(v: float | str) -> int:\n    match v:\n        case 2.5:\n            return 1\n        case \"2.5\":\n            return 2\n        case _:\n            return 3\n",
        "fn f_float() -> int:\n    let v: float | str = 2.5\n    return m(v)\n",
        "fn f_str() -> int:\n    let v: float | str = \"2.5\"\n    return m(v)\n",
    ));
    assert_eq!(call_i64(&mut cg, "f_float"), 1);
    assert_eq!(call_i64(&mut cg, "f_str"), 2);
}
