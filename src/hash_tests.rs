//! Derived structural hashing regression tests.
#[cfg(test)]
use crate::test_utils::{call_i64, call_i64_1, compile};

#[test]
fn hash_struct_dict_key_roundtrip() {
    let mut cg = compile(
        "struct Point:\n    x: int\n    y: int\n\nfn f() -> int:\n    let p1 = Point(1, 2)\n    let p2 = Point(1, 2)\n    let mut d: {Point: str} = {}\n    d[p1] = \"origin-ish\"\n    if p2 in d:\n        return len(d[p2])\n    return -1\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 10); // len("origin-ish")
}

#[test]
fn hash_struct_dict_key_distinct_values_dont_collide() {
    let mut cg = compile(
        "struct Point:\n    x: int\n    y: int\n\nfn f() -> int:\n    let p1 = Point(1, 2)\n    let p3 = Point(3, 4)\n    let mut d: {Point: str} = {}\n    d[p1] = \"a\"\n    if p3 in d:\n        return -1\n    return 1\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn hash_struct_set_dedupes_structural_duplicates() {
    let mut cg = compile(
        "struct Point:\n    x: int\n    y: int\n\nfn f() -> int:\n    let p1 = Point(1, 2)\n    let p2 = Point(1, 2)\n    let p3 = Point(3, 4)\n    let mut s = {p1}\n    s.add(p2)\n    s.add(p3)\n    return len(s)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 2);
}

#[test]
fn hash_struct_set_contains_structural() {
    let mut cg = compile(
        "struct Point:\n    x: int\n    y: int\n\nfn f() -> int:\n    let p1 = Point(1, 2)\n    let p2 = Point(1, 2)\n    let p3 = Point(3, 4)\n    let s = {p1}\n    if not s.contains(p2):\n        return -1\n    if s.contains(p3):\n        return -2\n    return 1\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn hash_enum_dict_key() {
    let mut cg = compile(
        "enum Shape:\n    Circle(int)\n    Square(int)\n\nfn f() -> int:\n    let mut d: {Shape: str} = {}\n    d[Circle(5)] = \"round\"\n    if Circle(5) in d:\n        return len(d[Circle(5)])\n    return -1\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 5); // len("round")
}

#[test]
fn hash_set_remove_structural() {
    let mut cg = compile(
        "struct Point:\n    x: int\n    y: int\n\nfn f() -> int:\n    let p1 = Point(1, 2)\n    let p2 = Point(1, 2)\n    let mut s = {p1}\n    s.remove(p2)\n    return len(s)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 0);
}

#[test]
fn hash_int_keyed_dict_still_works() {
    // Non-structural keys (the overwhelmingly common case) must stay on the
    // fast untyped path -- this is the regression guard for that.
    let mut cg = compile(
        "fn f(pick: int) -> int:\n    let mut d: {str: int} = {}\n    d[\"a\"] = 10\n    d[\"b\"] = 20\n    if pick == 1:\n        return d[\"a\"]\n    return d[\"b\"]\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 1), 10);
    assert_eq!(call_i64_1(&mut cg, "f", 0), 20);
}
