#[cfg(test)]
use crate::test_utils::{call_i64, compile};

#[test]
fn abs_negative_int() {
    let mut cg = compile("fn f() -> int:\n    abs(-5)\n");
    assert_eq!(call_i64(&mut cg, "f"), 5);
}

#[test]
fn abs_positive_int() {
    let mut cg = compile("fn f() -> int:\n    abs(3)\n");
    assert_eq!(call_i64(&mut cg, "f"), 3);
}

#[test]
fn abs_zero() {
    let mut cg = compile("fn f() -> int:\n    abs(0)\n");
    assert_eq!(call_i64(&mut cg, "f"), 0);
}

#[test]
fn abs_variable() {
    let mut cg = compile("fn f() -> int:\n    let x = -7\n    abs(x)\n");
    assert_eq!(call_i64(&mut cg, "f"), 7);
}

#[test]
fn round_down() {
    let mut cg = compile("fn f() -> int:\n    round(3.2)\n");
    assert_eq!(call_i64(&mut cg, "f"), 3);
}

#[test]
fn round_up() {
    let mut cg = compile("fn f() -> int:\n    round(3.8)\n");
    assert_eq!(call_i64(&mut cg, "f"), 4);
}

#[test]
fn round_half() {
    let mut cg = compile("fn f() -> int:\n    round(2.5)\n");
    assert_eq!(call_i64(&mut cg, "f"), 3);
}

#[test]
fn round_negative() {
    let mut cg = compile("fn f() -> int:\n    round(-3.8)\n");
    assert_eq!(call_i64(&mut cg, "f"), -4);
}

#[test]
fn round_variable() {
    let mut cg = compile("fn f() -> int:\n    let x = 4.49\n    round(x)\n");
    assert_eq!(call_i64(&mut cg, "f"), 4);
}

#[test]
fn sorted_int_list_leaves_source_untouched() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let xs = [3, 1, 2]\n",
        "    let ys = sorted(xs)\n",
        "    if ys[0] == 1 and ys[1] == 2 and ys[2] == 3 and xs[0] == 3 and xs[1] == 1 and xs[2] == 2:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn sorted_float_list() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let xs = [3.5, 1.1, 2.2]\n",
        "    let ys = sorted(xs)\n",
        "    if ys[0] < ys[1] and ys[1] < ys[2]:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn sorted_str_list() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let xs = [\"banana\", \"apple\", \"cherry\"]\n",
        "    let ys = sorted(xs)\n",
        "    if ys[0] == \"apple\" and ys[1] == \"banana\" and ys[2] == \"cherry\" and xs[0] == \"banana\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn sorted_any_list_falls_back_to_int_order() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let xs: [Any] = [3, 1, 2]\n",
        "    let ys = sorted(xs)\n",
        "    if int(ys[0]) == 1 and int(ys[1]) == 2 and int(ys[2]) == 3:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn reversed_list_leaves_source_untouched() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let xs = [1, 2, 3]\n",
        "    let rs = reversed(xs)\n",
        "    if rs[0] == 3 and rs[1] == 2 and rs[2] == 1 and xs[0] == 1 and xs[2] == 3:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn any_true_when_one_element_true() {
    let mut cg = compile(
        "fn f() -> int:\n    let xs = [False, False, True]\n    if any(xs):\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn any_false_when_all_false() {
    let mut cg = compile(
        "fn f() -> int:\n    let xs = [False, False, False]\n    if any(xs):\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 0);
}

#[test]
fn all_true_when_all_true() {
    let mut cg = compile(
        "fn f() -> int:\n    let xs = [True, True, True]\n    if all(xs):\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn all_false_when_one_element_false() {
    let mut cg = compile(
        "fn f() -> int:\n    let xs = [True, False, True]\n    if all(xs):\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 0);
}

#[test]
fn any_all_empty_list() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let xs: [bool] = []\n",
        "    if any(xs) == False and all(xs) == True:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn any_all_over_any_list() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let xs: [Any] = [False, True, False]\n",
        "    if any(xs) == True and all(xs) == False:\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}
