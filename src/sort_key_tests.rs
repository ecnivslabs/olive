//! E5.5: `sorted(xs, key=f)` and `xs.sort(key=f)`. `f: fn(T) -> K`, `K`
//! orderable (int, float, or str); both call forms share the
//! decorate-sort-undecorate lowering in `mir/builder/lower_expr/sort_key.rs`.

use crate::test_utils::{call_i64, check_codes, compile};

#[test]
fn sorted_by_str_len() {
    let mut cg = compile(
        r#"fn f() -> int:
    let words = ["banana", "kiwi", "fig", "apple"]
    let by_len = sorted(words, key=lambda w: len(w))
    return len(by_len[0]) * 1000 + len(by_len[1]) * 100 + len(by_len[2]) * 10 + len(by_len[3])
"#,
    );
    // fig(3), kiwi(4), apple(5), banana(6), ascending.
    assert_eq!(call_i64(&mut cg, "f"), 3000 + 400 + 50 + 6);
}

#[test]
fn sort_method_by_abs_value() {
    let mut cg = compile(
        r#"fn f() -> int:
    let mut nums = [5, -3, 2, -8, 1]
    nums.sort(key=lambda n: abs(n))
    return nums[0] * 10000 + nums[1] * 1000 + nums[2] * 100 + nums[3] * 10 + nums[4]
"#,
    );
    // Sorted by abs: 1, 2, -3, 5, -8
    assert_eq!(call_i64(&mut cg, "f"), 10000 + 2000 - 300 + 50 - 8);
}

#[test]
fn sorted_by_float_key() {
    let mut cg = compile(
        r#"fn f() -> int:
    let xs = [3, 1, 4, 1, 5]
    let by_neg = sorted(xs, key=lambda x: -(x as float))
    return by_neg[0] * 1000 + by_neg[1] * 100 + by_neg[2] * 10 + by_neg[3]
"#,
    );
    // Descending: 5, 4, 3, 1, 1 -- first four terms: 5000+400+30+1
    assert_eq!(call_i64(&mut cg, "f"), 5431);
}

#[test]
fn sort_key_is_stable() {
    // Equal keys keep their original relative order (Python's guarantee).
    let mut cg = compile(
        r#"struct Pair:
    tag: int
    order: int

fn f() -> int:
    let items = [Pair(1, 0), Pair(0, 1), Pair(1, 2), Pair(0, 3)]
    let by_tag = sorted(items, key=lambda p: p.tag)
    return by_tag[0].order * 1000 + by_tag[1].order * 100 + by_tag[2].order * 10 + by_tag[3].order
"#,
    );
    // tag=0 items (order 1,3) come first, in original order; then tag=1 (order 0,2).
    assert_eq!(call_i64(&mut cg, "f"), 1302);
}

#[test]
fn sorted_key_empty_list() {
    let mut cg = compile(
        r#"fn f() -> int:
    let xs: [int] = []
    let ys = sorted(xs, key=lambda x: x)
    return len(ys)
"#,
    );
    assert_eq!(call_i64(&mut cg, "f"), 0);
}

#[test]
fn key_captures_outer_variable() {
    // The key function is a genuine escaping closure (E5.2), not a bare fn.
    let mut cg = compile(
        r#"fn f() -> int:
    let target = 3
    let xs = [1, 5, 2, 8]
    let by_dist = sorted(xs, key=lambda x: abs(x - target))
    return by_dist[0]
"#,
    );
    assert_eq!(call_i64(&mut cg, "f"), 2); // |2-3| = 1, the smallest distance
}

#[test]
fn key_wrong_element_type_is_compile_error() {
    let codes = check_codes(
        "fn f():\n    let xs = [1, 2, 3]\n    let ys = sorted(xs, key=lambda (s: str): len(s))\n",
    );
    assert!(codes.contains(&"E0400".to_string()), "codes: {codes:?}");
}

#[test]
fn key_non_orderable_result_is_compile_error() {
    let codes = check_codes(
        "struct Blob:\n    v: int\nfn f():\n    let xs = [1, 2, 3]\n    let ys = sorted(xs, key=lambda x: Blob(x))\n",
    );
    assert!(codes.contains(&"E0404".to_string()), "codes: {codes:?}");
}

#[test]
fn sorted_unknown_kwarg_is_compile_error() {
    let codes =
        check_codes("fn f():\n    let xs = [1, 2, 3]\n    let ys = sorted(xs, reverse=True)\n");
    assert!(codes.contains(&"E0403".to_string()), "codes: {codes:?}");
}
