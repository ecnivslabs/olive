//! E2 optional-chaining (`?.`) regression tests (roadmap.md Phase E2, step 4).
#[cfg(test)]
use crate::test_utils::{call_i64_1, check_codes, compile};

#[test]
fn opt_attr_flagship_field_and_coalesce() {
    // `user?.name ?? "anon"`, driven by a pick flag since a struct can't
    // cross the raw-i64 FFI test boundary.
    let mut cg = compile(
        "struct User:\n    name: str\n\nfn f(pick: int) -> int:\n    let mut u: User | None = None\n    if pick == 1:\n        u = User(\"alice\")\n    return len(u?.name ?? \"anon\")\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 1), 5); // len(\"alice\")
    assert_eq!(call_i64_1(&mut cg, "f", 0), 4); // len(\"anon\")
}

#[test]
fn opt_attr_chained_two_levels() {
    let mut cg = compile(
        "struct Inner:\n    val: int\n\nstruct Outer:\n    inner: Inner | None\n\nfn f(pick: int) -> int:\n    let mut o: Outer | None = None\n    if pick == 1:\n        o = Outer(Inner(42))\n    return o?.inner?.val ?? -1\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 1), 42);
    assert_eq!(call_i64_1(&mut cg, "f", 0), -1);
}

#[test]
fn opt_attr_nullable_field_stays_nullable() {
    // `outer?.inner` yields `Inner | None`: the field's own nullability
    // must survive the receiver's narrowing, not get collapsed away.
    // `pick` picks whether the (always-present) Outer's Inner field is
    // itself None, exercising both sides of the field's own nullability.
    let mut cg = compile(
        "struct Inner:\n    val: int\n\nstruct Outer:\n    inner: Inner | None\n\nfn f(pick: int) -> int:\n    let mut o: Outer = Outer(None)\n    if pick == 1:\n        o = Outer(Inner(7))\n    let mid: Inner | None = o.inner\n    if mid == None:\n        return -1\n    return mid.val\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 0), -1);
    assert_eq!(call_i64_1(&mut cg, "f", 1), 7);
}

#[test]
fn opt_attr_receiver_never_none_is_error() {
    let codes = check_codes(
        "struct User:\n    name: str\n\nfn f(u: User) -> str:\n    return u?.name ?? \"x\"\n",
    );
    assert!(
        codes.contains(&"E0404".to_string()),
        "expected E0404 (receiver can never be None), got {codes:?}"
    );
}

#[test]
fn opt_attr_unknown_field_is_error() {
    let codes = check_codes(
        "struct User:\n    name: str\n\nfn f(u: User | None) -> str:\n    return u?.nope ?? \"x\"\n",
    );
    assert!(
        codes.contains(&"E0422".to_string()),
        "expected E0422 (unknown field), got {codes:?}"
    );
}
