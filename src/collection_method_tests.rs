//! List/dict/set method regression tests.
#[cfg(test)]
use crate::test_utils::{call_i64, check_codes, compile};

#[test]
fn list_count() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let xs = [1, 2, 3, 2, 1]\n",
        "    if xs.count(2) == 2 and xs.count(9) == 0:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn list_index() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let xs = [10, 20, 30]\n",
        "    xs.index(20)\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn list_index_absent_faults() {
    let codes = check_codes("fn f():\n    let xs = [1, 2]\n    xs.index(9)\n");
    assert!(
        codes.is_empty(),
        "index() absent case should typecheck, faulting only at runtime"
    );
}

#[test]
fn list_clear_leaves_empty_and_returns_list() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let xs = [1, 2, 3]\n",
        "    let ys = xs.clear()\n",
        "    if len(xs) == 0 and len(ys) == 0:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn list_count_index_str_elements() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let ss = [\"a\", \"b\", \"a\"]\n",
        "    if ss.count(\"a\") == 2 and ss.index(\"b\") == 1:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn dict_pop_removes_and_returns() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let d = {\"a\": 1, \"b\": 2}\n",
        "    let v = d.pop(\"a\")\n",
        "    if v == 1 and len(d) == 1:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn dict_pop_with_default_on_absent_key() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let d = {\"a\": 1}\n",
        "    if d.pop(\"z\", 99) == 99 and len(d) == 1:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn dict_setdefault_keeps_existing_and_inserts_missing() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let d = {\"a\": 1}\n",
        "    let existing = d.setdefault(\"a\", 100)\n",
        "    let inserted = d.setdefault(\"b\", 2)\n",
        "    if existing == 1 and inserted == 2 and len(d) == 2:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn dict_update_merges_and_leaves_source_untouched() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let a = {\"x\": 1}\n",
        "    let b = {\"x\": 10, \"y\": 20}\n",
        "    a.update(b)\n",
        "    if a[\"x\"] == 10 and a[\"y\"] == 20 and len(b) == 2 and b[\"x\"] == 10:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn dict_update_str_values_deep_copy_independent() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let a = {\"k\": \"hello\"}\n",
        "    let b = {\"k\": \"world\"}\n",
        "    a.update(b)\n",
        "    a.pop(\"k\")\n",
        "    if len(a) == 0 and b[\"k\"] == \"world\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn dict_clear_empties_and_returns_dict() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let d = {\"a\": 1, \"b\": 2}\n",
        "    let e = d.clear()\n",
        "    if len(d) == 0 and len(e) == 0:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn dict_pop_absent_no_default_faults() {
    let codes = check_codes("fn f():\n    let d = {\"a\": 1}\n    d.pop(\"z\")\n");
    assert!(
        codes.is_empty(),
        "pop() absent case should typecheck, faulting only at runtime"
    );
}

#[test]
fn set_discard_never_faults() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let s = {1, 2, 3}\n",
        "    s.discard(99)\n",
        "    if len(s) == 3:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn set_remove_present_element() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let s = {1, 2, 3}\n",
        "    s.remove(2)\n",
        "    if len(s) == 2:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn set_clear_empties_and_returns_set() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let s = {1, 2, 3}\n",
        "    let t = s.clear()\n",
        "    if len(s) == 0 and len(t) == 0:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn struct_keyed_collections_use_structural_comparison() {
    let mut cg = compile(concat!(
        "struct Point:\n",
        "    x: int\n",
        "    y: int\n",
        "\n",
        "fn f() -> int:\n",
        "    let p1 = Point(1, 2)\n",
        "    let p2 = Point(1, 2)\n",
        "    let p3 = Point(9, 9)\n",
        "    let s = {p1}\n",
        "    s.discard(p3)\n",
        "    let before = len(s)\n",
        "    s.remove(p2)\n",
        "    let after = len(s)\n",
        "    let d = {p1: \"hi\"}\n",
        "    let v = d.pop(p2)\n",
        "    let xs = [p1, p3]\n",
        "    if before == 1 and after == 0 and v == \"hi\" and xs.count(p2) == 1 and xs.index(p3) == 1:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}
