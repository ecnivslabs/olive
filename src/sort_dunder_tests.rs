//! Regression tests: `sort`/`sorted` on struct elements dispatch through
//! `__lt__` (`lower_sort_by_lt`) instead of the native int/float/str
//! comparator, which would otherwise silently order by raw struct pointer.
#[cfg(test)]
use crate::test_utils::{call_i64, check_codes, compile};

const P: &str = concat!(
    "struct P:\n",
    "    x: int\n",
    "\n",
    "impl P:\n",
    "    fn __lt__(self: &P, other: &P) -> bool:\n",
    "        self.x < other.x\n",
);

#[test]
fn sorted_orders_struct_list_by_lt() {
    let mut cg = compile(&format!(
        "{P}\nfn f() -> int:\n    let xs = [P(5), P(1), P(3)]\n    let ys = sorted(xs)\n    if ys[0].x == 1 and ys[1].x == 3 and ys[2].x == 5:\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn sort_method_orders_struct_list_by_lt_stably() {
    let mut cg = compile(&format!(
        "{P}\nfn f() -> int:\n    let mut xs = [P(9), P(1), P(9)]\n    xs.sort()\n    if xs[0].x == 1 and xs[1].x == 9 and xs[2].x == 9:\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn sorted_with_key_still_works_alongside_lt() {
    let mut cg = compile(&format!(
        "{P}\nfn f() -> int:\n    let xs = [P(5), P(1), P(3)]\n    let ys = sorted(xs, key=lambda p: -p.x)\n    if ys[0].x == 5 and ys[1].x == 3 and ys[2].x == 1:\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn missing_lt_on_sorted_is_clean_rejection() {
    let codes = check_codes(concat!(
        "struct Q:\n",
        "    n: int\n",
        "fn f():\n",
        "    let xs = [Q(3), Q(1)]\n",
        "    sorted(xs)\n",
    ));
    assert_eq!(codes, vec!["E0404"]);
}

#[test]
fn missing_lt_on_sort_method_is_clean_rejection() {
    let codes = check_codes(concat!(
        "struct Q:\n",
        "    n: int\n",
        "fn f():\n",
        "    let mut xs = [Q(3), Q(1)]\n",
        "    xs.sort()\n",
    ));
    assert_eq!(codes, vec!["E0404"]);
}
