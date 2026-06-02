//! E4.4 starred unpacking (`a, *rest = xs` / `let a, *rest = xs`) and E4.5
//! parenthesized destructuring (`let (a, b) = t`) regression tests.
//!
//! E4.4 scope: list RHS only (arity is a runtime property, unlike a
//! tuple's fixed shape); at most one starred name per target list; a
//! shortfall faults E0710; a lone `*ptr = v` (no comma) keeps meaning
//! pointer-dereference, unaffected. E4.5: parens around a `let` target
//! list are pure grouping, parsing to the identical AST as the bare form.

use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::test_utils::{call_i64, check_codes, compile};

#[test]
fn let_head_and_rest() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [1, 2, 3, 4, 5]\n    let first, *rest = xs\n    return first * 100 + len(rest)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 104);
}

#[test]
fn let_head_mid_tail() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [1, 2, 3, 4, 5]\n    let a, *mid, z = xs\n    return a * 1000 + z * 100 + len(mid)\n",
    );
    // a=1, z=5, len(mid)=3 -> 1000 + 500 + 3
    assert_eq!(call_i64(&mut cg, "f"), 1503);
}

#[test]
fn let_starred_empty_tail() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [42]\n    let only, *rest = xs\n    return only * 100 + len(rest)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 4200);
}

#[test]
fn plain_assign_head_and_rest() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [1, 2, 3, 4, 5]\n    let mut p = 0\n    let mut q = [0]\n    p, *q = xs\n    return p * 100 + len(q)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 104);
}

#[test]
fn starred_slice_is_independent_copy() {
    // The starred slice must not alias the source list: mutating one must
    // not be observable through the other.
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [1, 2, 3]\n    let a, *rest = xs\n    rest[0] = 99\n    return xs[1] * 100 + rest[0]\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 299);
}

#[test]
fn shortfall_is_compile_clean_but_faults_at_runtime() {
    // The program itself must type-check (the list's length is a runtime
    // property); the fault is exercised manually (aborting the process
    // isn't safe to trigger in-process from a unit test).
    let codes = check_codes(
        "fn f():\n    let xs: [int] = [1]\n    let a, b, *rest = xs\n    print(a, b, rest)\n",
    );
    assert!(codes.is_empty(), "codes: {codes:?}");
}

#[test]
fn two_starred_targets_is_compile_error() {
    // Rejected at parse time, so this checks the parser directly rather
    // than `check_codes` (which unwraps a successful parse).
    let src = "fn f():\n    let xs = [1, 2, 3]\n    let a, *b, *c = xs\n    print(a)\n";
    let tokens = Lexer::new(src, 0).tokenise().unwrap();
    assert!(Parser::new(tokens).parse_program().is_err());
}

#[test]
fn starred_target_on_tuple_rhs_is_compile_error() {
    let codes =
        check_codes("fn f():\n    let t = (1, 2, 3)\n    let a, *rest = t\n    print(a, rest)\n");
    assert!(codes.contains(&"E0417".to_string()), "codes: {codes:?}");
}

#[test]
fn lone_star_assign_is_still_pointer_dereference() {
    // A single target (no comma) keeps `*` meaning dereference, never a
    // starred-unpacking target -- unambiguous only in a multi-target list.
    let codes = check_codes("fn f(p: *i64):\n    unsafe:\n        *p = 5\n");
    assert!(codes.is_empty(), "codes: {codes:?}");
}

#[test]
fn parenthesized_let_multi_target() {
    let mut cg = compile(
        "fn f() -> i64:\n    let t = (10, 20, 30)\n    let (a, b, c) = t\n    return a + b + c\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 60);
}

#[test]
fn parenthesized_let_single_target() {
    let mut cg = compile("fn f() -> i64:\n    let (x) = 42\n    return x\n");
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn parenthesized_let_starred_target() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [1, 2, 3, 4]\n    let (first, *rest) = xs\n    return first * 100 + len(rest)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 103);
}
