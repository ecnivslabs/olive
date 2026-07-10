//! E2 derived structural equality regression tests (roadmap.md Phase E2, step 5).
#[cfg(test)]
use crate::test_utils::{call_i64, check_codes, compile};

#[test]
fn eq_struct_field_wise() {
    let mut cg = compile(
        "struct Point:\n    x: int\n    y: int\n\nfn f() -> int:\n    let p1 = Point(1, 2)\n    let p2 = Point(1, 2)\n    let p3 = Point(1, 3)\n    if p1 != p2:\n        return -1\n    if p1 == p3:\n        return -2\n    return 1\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn eq_enum_variant_and_payload() {
    let mut cg = compile(
        "enum Shape:\n    Circle(int)\n    Square(int)\n\nfn f() -> int:\n    let a = Circle(5)\n    let b = Circle(5)\n    let c = Circle(6)\n    let d = Square(5)\n    if a != b:\n        return -1\n    if a == c:\n        return -2\n    if a == d:\n        return -3\n    return 1\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn eq_tuple_element_wise() {
    let mut cg = compile(
        "fn f() -> int:\n    let t1 = (1, 2, 3)\n    let t2 = (1, 2, 3)\n    let t3 = (1, 2, 4)\n    if t1 != t2:\n        return -1\n    if t1 == t3:\n        return -2\n    return 1\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn eq_list_deep_not_pointer() {
    let mut cg = compile(
        "fn f() -> int:\n    let a = [1, 2, 3]\n    let b = [1, 2, 3]\n    let c = [1, 2, 4]\n    if a != b:\n        return -1\n    if a == c:\n        return -2\n    return 1\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn eq_set_order_independent() {
    let mut cg = compile(
        "fn f() -> int:\n    let a = {1, 2, 3}\n    let b = {3, 2, 1}\n    let c = {1, 2, 4}\n    if a != b:\n        return -1\n    if a == c:\n        return -2\n    return 1\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn eq_dict_order_independent() {
    let mut cg = compile(
        "fn f() -> int:\n    let a = {\"x\": 1, \"y\": 2}\n    let b = {\"y\": 2, \"x\": 1}\n    let c = {\"x\": 1, \"y\": 3}\n    if a != b:\n        return -1\n    if a == c:\n        return -2\n    return 1\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn eq_recursive_struct_no_infinite_loop() {
    let mut cg = compile(
        "struct Node:\n    val: int\n    next: Node | None\n\nfn f() -> int:\n    let n1 = Node(1, Node(2, None))\n    let n2 = Node(1, Node(2, None))\n    let n3 = Node(1, Node(3, None))\n    if n1 != n2:\n        return -1\n    if n1 == n3:\n        return -2\n    return 1\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn eq_deep_collection_case_table() {
    // Nested containers compared structurally, matching Python's `==` on the
    // equivalent list/dict/tuple values: recursion goes all the way down,
    // not just one level.
    let cases: &[(&str, i64)] = &[
        (
            "fn f() -> int:\n    return int([[1, 2], [3, 4]] == [[1, 2], [3, 4]])\n",
            1,
        ),
        (
            "fn f() -> int:\n    return int([[1, 2], [3, 4]] == [[1, 2], [3, 5]])\n",
            0,
        ),
        (
            "fn f() -> int:\n    return int([[1, 2], [3, 4]] != [[1, 2], [3, 5]])\n",
            1,
        ),
        (
            "fn f() -> int:\n    let a: [{str: int}] = [{\"x\": 1}]\n    let b: [{str: int}] = [{\"x\": 1}]\n    return int(a == b)\n",
            1,
        ),
        (
            "fn f() -> int:\n    let a: {str: [int]} = {\"k\": [1, 2]}\n    let b: {str: [int]} = {\"k\": [1, 2]}\n    return int(a == b)\n",
            1,
        ),
        (
            "fn f() -> int:\n    let a: {str: [int]} = {\"k\": [1, 2]}\n    let b: {str: [int]} = {\"k\": [1, 3]}\n    return int(a == b)\n",
            0,
        ),
        (
            "fn f() -> int:\n    return int((1, (2, 3)) == (1, (2, 3)))\n",
            1,
        ),
        (
            "fn f() -> int:\n    return int((1, (2, 3)) == (1, (2, 4)))\n",
            0,
        ),
        (
            "fn f() -> int:\n    let a: [int] = []\n    let b: [int] = []\n    return int(a == b)\n",
            1,
        ),
        ("fn f() -> int:\n    return int([1] == [1, 2])\n", 0),
    ];
    for (src, expected) in cases {
        let mut cg = compile(src);
        assert_eq!(call_i64(&mut cg, "f"), *expected, "case: {src}");
    }
}

#[test]
fn eq_mismatched_struct_types_rejected() {
    let codes = check_codes(
        "struct Point:\n    x: int\n\nstruct Line:\n    len: int\n\nfn f(p: Point, l: Line) -> bool:\n    return p == l\n",
    );
    assert!(
        codes
            .iter()
            .any(|c| c.as_str() == "E0400" || c.as_str() == "E0404"),
        "expected a type error for mismatched struct comparison, got {codes:?}"
    );
}

#[test]
fn eq_fn_field_struct_rejected() {
    let codes = check_codes(
        "struct Callback:\n    f: fn(int) -> int\n\nfn double(x: int) -> int:\n    return x * 2\n\nfn f() -> bool:\n    let a = Callback(double)\n    let b = Callback(double)\n    return a == b\n",
    );
    assert!(
        codes.contains(&"E0404".to_string()),
        "expected E0404 (no defined ==), got {codes:?}"
    );
}
