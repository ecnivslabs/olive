//! Type alias regression tests.
#[cfg(test)]
use crate::test_utils::{call_i64, check_codes, compile};

#[test]
fn alias_tuple_destructure_under_full_optimizer() {
    // Regression guard: the MIR builder's own type-name resolver used to
    // default any name it didn't recognize to a nominal struct, so an alias
    // to a tuple silently became `Struct("Pair", [], false)` and crashed on
    // destructure once the full optimizer/AOT pipeline touched it.
    let mut cg = compile(
        "type Pair = (int, int)\n\nfn first(p: Pair) -> int:\n    let x, y = p\n    return x + y\n\nfn main() -> int:\n    let p: Pair = (3, 4)\n    return first(p)\n",
    );
    assert_eq!(call_i64(&mut cg, "main"), 7);
}

#[test]
fn alias_to_struct_roundtrips() {
    let mut cg = compile(
        "struct Point:\n    x: int\n    y: int\n\ntype Coord = Point\n\nfn dist(p: Coord) -> int:\n    return p.x + p.y\n\nfn main() -> int:\n    let p: Coord = Point(3, 4)\n    return dist(p)\n",
    );
    assert_eq!(call_i64(&mut cg, "main"), 7);
}

#[test]
fn alias_of_alias_resolves() {
    let mut cg = compile(
        "struct Point:\n    x: int\n    y: int\n\ntype Coord = Point\ntype Coord2 = Coord\n\nfn dist(p: Coord2) -> int:\n    return p.x + p.y\n\nfn main() -> int:\n    let p: Coord2 = Point(1, 2)\n    return dist(p)\n",
    );
    assert_eq!(call_i64(&mut cg, "main"), 3);
}

#[test]
fn alias_to_none_union_narrows() {
    let mut cg = compile(
        "struct ParseError:\n    msg: str\n\ntype ParseResult = int | ParseError\n\nfn f(v: ParseResult) -> int:\n    if v != None:\n        return 1\n    return -1\n\nfn main() -> int:\n    return f(5)\n",
    );
    assert_eq!(call_i64(&mut cg, "main"), 1);
}

#[test]
fn alias_self_cycle_rejected() {
    let codes = check_codes("type A = A\n\nfn main():\n    pass\n");
    assert!(codes.contains(&"E0426".to_string()), "codes: {codes:?}");
}

#[test]
fn alias_mutual_cycle_rejected() {
    let codes = check_codes("type A = B\ntype B = A\n\nfn main():\n    pass\n");
    assert!(codes.contains(&"E0426".to_string()), "codes: {codes:?}");
}

#[test]
fn alias_no_nominal_identity() {
    // An alias is pure substitution: a plain `[int]` and its aliased name are
    // the same type, interchangeable at a call boundary.
    let mut cg = compile(
        "type IntList = [int]\n\nfn total(xs: IntList) -> int:\n    let mut s = 0\n    for x in xs:\n        s = s + x\n    return s\n\nfn main() -> int:\n    let plain: [int] = [1, 2, 3]\n    return total(plain)\n",
    );
    assert_eq!(call_i64(&mut cg, "main"), 6);
}
