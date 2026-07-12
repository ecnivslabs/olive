//! PREREQ-ITER-BORROW, E4.1 (`enumerate`), E4.2 (`zip`), E4.3 (range `by`
//! step) regression tests.
//!
//! `for`/comprehension iteration borrows its iterable (`Rvalue::Ref`, never
//! a copy); the borrow checker enforces exclusivity against it for the
//! loop's duration. `enumerate`/`zip` are `for`-head-only desugars with
//! real (not `Any`-boxed) element types. `by` is a contextual step on `..`/
//! `..=` ranges; direction follows the step's sign.

use crate::mir::ir::{Rvalue, StatementKind};
use crate::test_utils::{build_mir, call_i64, check_borrow_codes, check_codes, compile};

fn has_ref_of(mir: &crate::mir::ir::MirFunction, target_name: &str) -> bool {
    let target_local = mir
        .locals
        .iter()
        .position(|l| l.name.as_deref() == Some(target_name));
    let Some(target_local) = target_local else {
        return false;
    };
    mir.basic_blocks.iter().any(|bb| {
        bb.statements.iter().any(|s| {
            matches!(
                &s.kind,
                StatementKind::Assign(_, Rvalue::Ref(l) | Rvalue::MutRef(l))
                    if l.0 == target_local
            )
        })
    })
}

#[test]
fn for_loop_borrows_named_list_no_copy() {
    let fns = build_mir(
        "fn f() -> i64:\n    let xs = [1, 2, 3]\n    let mut s = 0\n    for x in xs:\n        s = s + x\n    return s + len(xs)\n",
    );
    let f = fns.iter().find(|f| f.name == "f").unwrap();
    assert!(has_ref_of(f, "xs"), "expected a Ref(xs) borrow in MIR");
}

#[test]
fn for_loop_over_named_list_runtime_value() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [1, 2, 3]\n    let mut s = 0\n    for x in xs:\n        s = s + x\n    return s + len(xs)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 9);
}

#[test]
fn for_loop_reassigning_borrowed_iterable_is_compile_error() {
    let codes = check_borrow_codes(
        "fn f():\n    let mut xs = [1, 2, 3]\n    for x in xs:\n        xs = [9, 9]\n",
    );
    assert!(codes.contains(&"E0500".to_string()), "codes: {codes:?}");
}

#[test]
fn for_loop_mutating_borrowed_iterable_is_compile_error() {
    let codes = check_borrow_codes(
        "fn f():\n    let mut xs = [1, 2, 3]\n    for x in xs:\n        xs.append(x)\n",
    );
    assert!(codes.contains(&"E0504".to_string()), "codes: {codes:?}");
}

#[test]
fn for_loop_reading_borrowed_iterable_is_not_an_error() {
    let codes =
        check_codes("fn f():\n    let xs = [1, 2, 3]\n    for x in xs:\n        let y = len(xs)\n");
    assert!(codes.is_empty(), "codes: {codes:?}");
}

#[test]
fn comprehension_borrows_named_list_no_copy() {
    let fns = build_mir(
        "fn f() -> i64:\n    let xs = [1, 2, 3]\n    let ys = [x * x for x in xs]\n    return ys[0] + len(xs)\n",
    );
    let f = fns.iter().find(|f| f.name == "f").unwrap();
    assert!(has_ref_of(f, "xs"), "expected a Ref(xs) borrow in MIR");
}

#[test]
fn enumerate_borrows_named_list_no_copy() {
    let fns = build_mir(
        "fn f() -> i64:\n    let xs = [1, 2, 3]\n    let mut s = 0\n    for i, x in enumerate(xs):\n        s = s + i + x\n    return s + len(xs)\n",
    );
    let f = fns.iter().find(|f| f.name == "f").unwrap();
    assert!(has_ref_of(f, "xs"), "expected a Ref(xs) borrow in MIR");
}

#[test]
fn zip_borrows_both_named_lists_no_copy() {
    let fns = build_mir(
        "fn f() -> i64:\n    let xs = [1, 2, 3]\n    let ys = [4, 5, 6]\n    let mut s = 0\n    for a, b in zip(xs, ys):\n        s = s + a + b\n    return s + len(xs) + len(ys)\n",
    );
    let f = fns.iter().find(|f| f.name == "f").unwrap();
    assert!(has_ref_of(f, "xs"), "expected a Ref(xs) borrow in MIR");
    assert!(has_ref_of(f, "ys"), "expected a Ref(ys) borrow in MIR");
}

#[test]
fn enumerate_index_and_value_are_correct_and_typed() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [10, 20, 30]\n    let mut s = 0\n    for i, x in enumerate(xs):\n        s = s + i * 100 + x\n    return s\n",
    );
    // i: 0,1,2  x: 10,20,30 -> 10 + 120 + 230 = 360
    assert_eq!(call_i64(&mut cg, "f"), 360);
}

#[test]
fn enumerate_with_start_argument() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [10, 20, 30]\n    let mut s = 0\n    for i, x in enumerate(xs, 5):\n        s = s + i\n    return s\n",
    );
    // i: 5,6,7 -> 18
    assert_eq!(call_i64(&mut cg, "f"), 18);
}

#[test]
fn enumerate_whole_pair_binding() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [10, 20, 30]\n    let mut s = 0\n    for pair in enumerate(xs):\n        s = s + pair[0] * 100 + pair[1]\n    return s\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 360);
}

#[test]
fn enumerate_outside_for_head_is_compile_error() {
    let codes = check_codes("fn f():\n    let xs = [1, 2, 3]\n    let y = enumerate(xs)\n");
    assert!(codes.contains(&"E0429".to_string()), "codes: {codes:?}");
}

#[test]
fn zip_pairs_two_lists_correctly() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [1, 2, 3]\n    let ys = [10, 20, 30]\n    let mut s = 0\n    for a, b in zip(xs, ys):\n        s = s + a * 100 + b\n    return s\n",
    );
    // (1*100+10)+(2*100+20)+(3*100+30) = 110+220+330 = 660
    assert_eq!(call_i64(&mut cg, "f"), 660);
}

#[test]
fn zip_stops_at_shorter_iterable() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [1, 2, 3]\n    let ys = [10, 20]\n    let mut count = 0\n    for a, b in zip(xs, ys):\n        count = count + 1\n    return count\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 2);
}

#[test]
fn zip_whole_pair_binding() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [1, 2, 3]\n    let ys = [10, 20, 30]\n    let mut s = 0\n    for pair in zip(xs, ys):\n        s = s + pair[0] * 100 + pair[1]\n    return s\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 660);
}

#[test]
fn zip_three_args_is_compile_error() {
    let codes = check_codes(
        "fn f():\n    let xs = [1, 2]\n    let ys = [3, 4]\n    let zs = [5, 6]\n    for a, b, c in zip(xs, ys, zs):\n        let s = a\n",
    );
    assert!(codes.contains(&"E0421".to_string()), "codes: {codes:?}");
}

#[test]
fn zip_outside_for_head_is_compile_error() {
    let codes =
        check_codes("fn f():\n    let xs = [1, 2]\n    let ys = [3, 4]\n    let z = zip(xs, ys)\n");
    assert!(codes.contains(&"E0429".to_string()), "codes: {codes:?}");
}

#[test]
fn range_step_forward() {
    let mut cg = compile(
        "fn f() -> i64:\n    let mut s = 0\n    for i in 0..10 by 2:\n        s = s + i\n    return s\n",
    );
    // 0+2+4+6+8 = 20
    assert_eq!(call_i64(&mut cg, "f"), 20);
}

#[test]
fn range_step_reverse() {
    let mut cg = compile(
        "fn f() -> i64:\n    let mut s = 0\n    for i in 10..0 by -1:\n        s = s + i\n    return s\n",
    );
    // 10+9+...+1 = 55
    assert_eq!(call_i64(&mut cg, "f"), 55);
}

#[test]
fn range_step_reverse_inclusive() {
    let mut cg = compile(
        "fn f() -> i64:\n    let mut s = 0\n    for i in 10..=0 by -2:\n        s = s + i\n    return s\n",
    );
    // 10+8+6+4+2+0 = 30
    assert_eq!(call_i64(&mut cg, "f"), 30);
}

#[test]
fn range_stepless_backward_is_empty_like_python() {
    let mut cg = compile(
        "fn f() -> i64:\n    let mut count = 0\n    for i in 5..0:\n        count = count + 1\n    return count\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 0);
}

#[test]
fn range_step_no_step_matches_old_behavior() {
    let mut cg = compile(
        "fn f() -> i64:\n    let mut s = 0\n    for i in 0..5:\n        s = s + i\n    return s\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 10);
}

#[test]
fn range_step_in_comprehension() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [i for i in 0..10 by 3]\n    let mut s = 0\n    for x in xs:\n        s = s + x\n    return s\n",
    );
    // 0+3+6+9 = 18
    assert_eq!(call_i64(&mut cg, "f"), 18);
}

#[test]
fn range_step_runtime_variable_direction() {
    let mut cg = compile(
        "fn count(step: i64) -> i64:\n    let mut n = 0\n    for i in 0..10 by step:\n        n = n + 1\n    return n\n\nfn f() -> i64:\n    return count(2) * 100 + count(-1)\n",
    );
    // count(2): 0,2,4,6,8 -> 5 iterations, count(-1) over 0..10 by -1 (ascending
    // range, descending step) never advances toward 10 from 0, so 0 iterations.
    assert_eq!(call_i64(&mut cg, "f"), 500);
}

#[test]
fn range_literal_zero_step_is_compile_error() {
    let codes = check_codes("fn f():\n    for i in 0..10 by 0:\n        print(i)\n");
    assert!(codes.contains(&"E0430".to_string()), "codes: {codes:?}");
}

#[test]
fn range_stepped_in_operator_is_compile_error() {
    let codes = check_codes("fn f() -> bool:\n    return 5 in 0..10 by 2\n");
    assert!(codes.contains(&"E0431".to_string()), "codes: {codes:?}");
}

#[test]
fn range_plain_in_operator_still_works() {
    let codes = check_codes("fn f() -> bool:\n    return 5 in 0..10\n");
    assert!(codes.is_empty(), "codes: {codes:?}");
}
