//! E5.1/E5.2 lambda-as-value regression tests: parsing (bare vs parenthesized
//! annotated params), call-site type inference, uninferable-param rejection
//! (E0433), the direct-call (non-escaping) capturing-lambda path (called at
//! its definition site or as an IIFE), and escaping capturing closures
//! (returned, stored, passed as an argument -- E5.2's closure records).

use crate::test_utils::{build_mir, call_i64, check_closure_codes, check_codes, compile};

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
fn capturing_lambda_returned_works() {
    // The classic make_adder shape: a closure record built at `return`,
    // called later through a plain `fn(int) -> int` local.
    let mut cg = compile(
        "fn make_adder(n: int) -> fn(int) -> int:\n    return lambda x: x + n\n\
         fn f() -> int:\n    let add5 = make_adder(5)\n    return add5(3)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 8);
}

#[test]
fn capturing_lambda_passed_as_arg_works() {
    let mut cg = compile(
        "fn apply(f: fn(int) -> int, v: int) -> int:\n    return f(v)\n\
         fn f() -> int:\n    let n = 10\n    return apply(lambda x: x + n, 5)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 15);
}

#[test]
fn capturing_lambda_assigned_then_escaped_works() {
    let mut cg = compile(
        "fn make() -> fn(int) -> int:\n    let n = 10\n    let g = lambda x: x + n\n    return g\n\
         fn f() -> int:\n    let h = make()\n    return h(5)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 15);
}

#[test]
fn named_nested_fn_returned_works() {
    // Same escape, through a named `fn` instead of a lambda literal.
    let mut cg = compile(
        "fn make(n: int) -> fn(int) -> int:\n    fn add(x: int) -> int:\n        return x + n\n    return add\n\
         fn f() -> int:\n    let g = make(7)\n    return g(2)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 9);
}

#[test]
fn closure_captures_value_not_live_variable() {
    // Captures copy the value at construction time; mutating the outer
    // variable afterward must not be visible through the closure.
    let mut cg = compile(
        "fn make() -> fn() -> int:\n    let mut n = 1\n    let g = lambda: n\n    n = 99\n    return g\n\
         fn f() -> int:\n    let h = make()\n    return h()\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn multiple_closures_independent_captures() {
    // Two escaping closures from the same factory each own their own
    // capture, not a shared record.
    let mut cg = compile(
        "fn make_adder(n: int) -> fn(int) -> int:\n    return lambda x: x + n\n\
         fn f() -> int:\n    let add1 = make_adder(1)\n    let add2 = make_adder(2)\n    return add1(10) + add2(10)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 23);
}

#[test]
fn closure_capture_stores_via_setattr() {
    // The builder lowers an escaping capture as a `SetAttr` write of a
    // `Copy` of the captured local into the closure record -- the same
    // shape `escape_copies` already classifies generically, so copy-vs-move
    // for captures falls out of the existing ownership pass with no new
    // capture-specific logic.
    let fns = build_mir("fn make_adder(n: int) -> fn(int) -> int:\n    return lambda x: x + n\n");
    let make_adder = fns.iter().find(|f| f.name == "make_adder").unwrap();
    let stores_capture = make_adder.basic_blocks.iter().any(|bb| {
        bb.statements.iter().any(|s| {
            matches!(
                &s.kind,
                crate::mir::ir::StatementKind::SetAttr(_, attr, crate::mir::ir::Operand::Copy(_))
                    if attr == "n"
            )
        })
    });
    assert!(
        stores_capture,
        "expected a SetAttr(.., \"n\", Copy(_)) capture store"
    );
}

#[test]
fn bound_lambda_sibling_cross_scope_call_is_compile_error() {
    // E5.2 lifted the escape restriction (E0423), but a direct call still
    // needs its captures in scope: calling a bound lambda from a sibling
    // function that never captured them is still E0424, same as a named
    // nested fn.
    let codes = check_closure_codes(
        "fn o() -> int:\n    let p = 1\n    let a = lambda: p\n    fn b() -> int:\n        return a()\n    return b()\n",
    );
    assert!(codes.contains(&"E0424".to_string()), "codes: {codes:?}");
}

// E5.3: the indirect calling convention generalizes to every position a
// `Type::Fn` value can live in, while a named call stays direct.

#[test]
fn direct_call_to_named_fn_value_param_stays_direct() {
    // docs/functions.md's `apply(square, 5)`: `square` is boxed into a
    // closure record (it's passed as a value), but the call to `apply`
    // itself is an ordinary direct call by name, never indirect.
    let fns = build_mir(
        "fn apply(f: fn(int) -> int, val: int) -> int:\n    return f(val)\n\
         fn square(x: int) -> int:\n    return x * x\n\
         fn main() -> int:\n    return apply(square, 5)\n",
    );
    let main = fns.iter().find(|f| f.name == "main").unwrap();
    let calls_apply_directly = main.basic_blocks.iter().any(|bb| {
        bb.statements.iter().any(|s| {
            matches!(
                &s.kind,
                crate::mir::ir::StatementKind::Assign(
                    _,
                    crate::mir::ir::Rvalue::Call {
                        func: crate::mir::ir::Operand::Constant(
                            crate::mir::ir::Constant::Function(name)
                        ),
                        ..
                    },
                ) if name == "apply"
            )
        })
    });
    assert!(
        calls_apply_directly,
        "apply(square, 5) must call `apply` directly by name"
    );
}

#[test]
fn indirect_call_through_struct_field_works() {
    let mut cg = compile(
        "struct Box:\n    f: fn(int) -> int\n\
         fn double(x: int) -> int:\n    return x * 2\n\
         fn f() -> int:\n    let b = Box(double)\n    return b.f(21)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn indirect_call_through_list_element_works() {
    let mut cg = compile(
        "fn square(x: int) -> int:\n    return x * x\n\
         fn double(x: int) -> int:\n    return x * 2\n\
         fn f() -> int:\n    let fns = [square, double]\n    return fns[0](5) + fns[1](5)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 35);
}
