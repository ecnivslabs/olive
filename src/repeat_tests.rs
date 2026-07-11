//! E3.4 repetition operator regression tests (roadmap.md Phase E3.4).
#[cfg(test)]
use crate::test_utils::{call_i64, compile};

#[test]
fn str_times_int() {
    let mut cg = compile(
        "fn f() -> int:\n    let s = \"ab\" * 3\n    if s == \"ababab\":\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn int_times_str_mirrors() {
    let mut cg = compile(
        "fn f() -> int:\n    let s = 3 * \"ab\"\n    if s == \"ababab\":\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn str_times_zero_is_empty() {
    let mut cg = compile(
        "fn f() -> int:\n    let s = \"x\" * 0\n    if len(s) == 0:\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn str_times_negative_is_empty() {
    let mut cg = compile(
        "fn f() -> int:\n    let s = \"x\" * -2\n    if len(s) == 0:\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn int_list_times_int_leaves_source_untouched() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let xs = [1, 2]\n",
        "    let ys = xs * 3\n",
        "    if len(ys) == 6 and ys[0] == 1 and ys[5] == 2 and len(xs) == 2 and xs[0] == 1:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn int_times_int_list_mirrors() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let xs = [1, 2]\n",
        "    let ys = 3 * xs\n",
        "    if len(ys) == 6 and ys[4] == 1 and ys[5] == 2:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn int_list_times_zero_is_empty() {
    let mut cg = compile(
        "fn f() -> int:\n    let xs = [1, 2, 3]\n    let ys = xs * 0\n    if len(ys) == 0:\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn nested_list_rows_are_independent_copies() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let rows = [[1]]\n",
        "    let tripled = rows * 3\n",
        "    tripled[0].append(9)\n",
        "    if len(tripled[0]) == 2 and len(tripled[1]) == 1 and len(tripled[2]) == 1 and len(rows[0]) == 1:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn str_list_times_int() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let ss = [\"a\", \"b\"]\n",
        "    let rs = ss * 2\n",
        "    if len(rs) == 4 and rs[0] == \"a\" and rs[2] == \"a\" and len(ss) == 2:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}
