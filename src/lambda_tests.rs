//! E5.1 lambda-as-value regression tests: parsing (bare vs parenthesized
//! annotated params), call-site type inference, uninferable-param rejection
//! (E0433), and the direct-call (non-escaping) capturing-lambda path -- a
//! capturing lambda called at its definition site or as an IIFE, which
//! escaping-closure rejection (E0423) does not cover.

use crate::test_utils::{call_i64, check_closure_codes, check_codes, compile};

#[test]
fn bare_unannotated_params_parse() {
    // The `let`'s own type annotation is the expected-type context that
    // infers both params, matching an empty list literal adopting its
    // annotation's element type.
    let mut cg = compile(
        "fn f() -> int:\n    let g: fn(int, int) -> int = lambda x, y: x + y\n    return g(2, 3)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 5);
}

#[test]
fn no_params_parse() {
    let mut cg = compile("fn f() -> int:\n    let g = lambda: 42\n    return g()\n");
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn parenthesized_annotated_params_parse() {
    let mut cg =
        compile("fn f() -> int:\n    let g = lambda (x: int, y: int): x * y\n    return g(6, 7)\n");
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn param_type_inferred_from_call_site_hint() {
    // `apply`'s declared param type flows into the lambda literal's own
    // unannotated param, exactly like an empty list literal adopts its
    // expected element type.
    let mut cg = compile(
        "fn apply(f: fn(int) -> int, v: int) -> int:\n    return f(v)\n\
         fn f() -> int:\n    return apply(lambda x: x * 2, 21)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn uninferable_param_is_compile_error() {
    let codes = check_codes("fn f():\n    let g = lambda x: x\n    print(g)\n");
    assert!(codes.contains(&"E0433".to_string()), "codes: {codes:?}");
}

#[test]
fn param_used_in_arithmetic_infers_without_annotation() {
    // `x`'s var unifies with `int` via `x + 1`, no annotation needed.
    let mut cg = compile("fn f() -> int:\n    let g = lambda x: x + 1\n    return g(41)\n");
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn capturing_lambda_called_after_let_binding() {
    let mut cg =
        compile("fn f() -> int:\n    let n = 10\n    let g = lambda x: x + n\n    return g(5)\n");
    assert_eq!(call_i64(&mut cg, "f"), 15);
}

#[test]
fn capturing_lambda_multi_capture() {
    let mut cg = compile(
        "fn f() -> int:\n    let a = 3\n    let b = 4\n    let g = lambda x, y: x * a + y * b\n    return g(2, 5)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 26);
}

#[test]
fn capturing_lambda_iife() {
    let mut cg = compile("fn f() -> int:\n    let n = 100\n    return (lambda x: x + n)(5)\n");
    assert_eq!(call_i64(&mut cg, "f"), 105);
}

#[test]
fn capturing_lambda_reassigned_still_calls_correctly() {
    // `g = lambda ...` (plain assignment, not `let`) registers the same way.
    let mut cg = compile(
        "fn f() -> int:\n    let n = 7\n    let mut g = lambda (x: int): x\n    g = lambda x: x + n\n    return g(3)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 10);
}

#[test]
fn capturing_lambda_in_loop_recaptures_each_iteration() {
    let mut cg = compile(
        "fn f() -> int:\n    let mut total = 0\n    for i in 0..3:\n        let g = lambda x: x + i\n        total = total + g(10)\n    return total\n",
    );
    // i = 0,1,2 -> (10+0)+(10+1)+(10+2) = 33
    assert_eq!(call_i64(&mut cg, "f"), 33);
}

#[test]
fn noncapturing_lambda_can_escape() {
    // No captures means nothing to reject: a plain lambda is a value like
    // any other named fn reference.
    let mut cg = compile(
        "fn make() -> fn(int) -> int:\n    return lambda x: x * 2\n\
         fn f() -> int:\n    let g = make()\n    return g(21)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn capturing_lambda_returned_is_compile_error() {
    let codes = check_closure_codes(
        "fn make() -> fn(int) -> int:\n    let n = 10\n    return lambda x: x + n\n",
    );
    assert!(codes.contains(&"E0423".to_string()), "codes: {codes:?}");
}

#[test]
fn capturing_lambda_passed_as_arg_is_compile_error() {
    let codes = check_closure_codes(
        "fn apply(f: fn(int) -> int, v: int) -> int:\n    return f(v)\n\
         fn f() -> int:\n    let n = 10\n    return apply(lambda x: x + n, 5)\n",
    );
    assert!(codes.contains(&"E0423".to_string()), "codes: {codes:?}");
}

#[test]
fn capturing_lambda_assigned_then_escaped_is_compile_error() {
    let codes = check_closure_codes(
        "fn make() -> fn(int) -> int:\n    let n = 10\n    let g = lambda x: x + n\n    return g\n",
    );
    assert!(codes.contains(&"E0423".to_string()), "codes: {codes:?}");
}
