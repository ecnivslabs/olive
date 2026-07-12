#[cfg(test)]
mod borrow_check_tests_extended {
    use crate::borrow_check::BorrowChecker;
    use crate::lexer::Lexer;
    use crate::mir::MirBuilder;
    use crate::parser::Parser;
    use crate::semantic::SemanticError;
    use crate::semantic::{Resolver, TypeChecker};
    use rustc_hash::FxHashSet as HashSet;

    fn borrow_check(src: &str) -> Vec<SemanticError> {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        let mut tc = TypeChecker::new();
        tc.check_program(&prog);
        let mut builder = MirBuilder::new(
            &tc.expr_types,
            &tc.expr_kwarg_maps,
            &tc.type_env[0],
            tc.struct_fields.clone(),
            &tc.traits,
            HashSet::default(),
            tc.enum_defs.clone(),
        );
        builder.build_program(&prog);
        let mut all_errors = Vec::new();
        for func in &builder.functions {
            let mut bc = BorrowChecker::new(func, &tc.struct_fields);
            bc.check();
            all_errors.extend(bc.errors);
        }
        all_errors
    }

    fn has_error_matching(errors: &[SemanticError], substr: &str) -> bool {
        errors.iter().any(|e| e.to_string().contains(substr))
    }

    #[test]
    fn use_after_call_is_valid() {
        // Arguments are borrows under inferred ownership: the caller keeps the
        // list and reading it after the call is fine.
        let errors = borrow_check(
            "fn consume(xs: [i64]) -> i64:\n    return 0\n\nfn caller() -> i64:\n    let xs = [1, 2]\n    consume(xs)\n    return xs[0]\n",
        );
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
    }

    #[test]
    fn mutable_borrow_conflict_detected() {
        let errors = borrow_check(
            "fn caller() -> i64:\n    let mut x = 42\n    let r1 = &mut x\n    let r2 = &mut x\n    return *r1 + *r2\n",
        );
        assert!(
            !errors.is_empty(),
            "double mutable borrow should produce an error"
        );
        assert!(
            has_error_matching(&errors, "cannot borrow"),
            "expected borrow conflict error, got: {:?}",
            errors
        );
    }

    #[test]
    fn mutable_borrow_of_immutable_detected() {
        let errors =
            borrow_check("fn caller() -> i64:\n    let x = 42\n    let r = &mut x\n    return 0\n");
        assert!(
            !errors.is_empty(),
            "mutable borrow of immutable should produce an error"
        );
        assert!(
            has_error_matching(&errors, "cannot mutably borrow immutable"),
            "expected immutable borrow error, got: {:?}",
            errors
        );
    }

    #[test]
    fn immutable_borrow_allows_shared_access() {
        let errors = borrow_check(
            "fn read(r: &i64) -> i64:\n    return 0\n\nfn caller() -> i64:\n    let x = 42\n    let r1 = &x\n    let r2 = &x\n    read(r1)\n    read(r2)\n    return 0\n",
        );
        assert!(
            errors.is_empty(),
            "multiple immutable borrows should be allowed: {:?}",
            errors
        );
    }

    #[test]
    fn borrow_released_after_last_use_nll() {
        let errors = borrow_check(
            "fn consume(xs: [i64]) -> i64:\n    return 0\n\nfn read(r: &[i64]) -> i64:\n    return 0\n\nfn caller() -> i64:\n    let xs = [1, 2]\n    let r = &xs\n    read(r)\n    consume(xs)\n    return 0\n",
        );
        assert!(
            errors.is_empty(),
            "borrow released before move should be valid: {:?}",
            errors
        );
    }

    #[test]
    fn borrow_through_function_call() {
        let errors = borrow_check(
            "fn read(r: &i64) -> i64:\n    return 0\n\nfn caller() -> i64:\n    let x = 42\n    read(&x)\n    return x\n",
        );
        assert!(
            errors.is_empty(),
            "borrow passed to fn then use after should be valid: {:?}",
            errors
        );
    }

    #[test]
    fn move_into_function_no_subsequent_use() {
        let errors = borrow_check(
            "fn consume(xs: [i64]) -> i64:\n    return 0\n\nfn caller() -> i64:\n    let xs = [1, 2]\n    consume(xs)\n    return 0\n",
        );
        assert!(
            errors.is_empty(),
            "move without subsequent use should be valid: {:?}",
            errors
        );
    }

    #[test]
    fn reborrow_from_mut_ref() {
        let errors = borrow_check(
            "fn caller() -> i64:\n    let mut x = 42\n    let r = &mut x\n    let r2 = &r\n    return 0\n",
        );
        assert!(
            errors.is_empty(),
            "reborrow from mut ref should be valid: {:?}",
            errors
        );
    }

    #[test]
    fn loop_iteration_borrow() {
        let errors = borrow_check(
            "fn caller(xs: [i64]) -> i64:\n    let mut s = 0\n    for x in xs:\n        s = s + x\n    return s\n",
        );
        assert!(
            errors.is_empty(),
            "for loop should not produce borrow errors: {:?}",
            errors
        );
    }

    #[test]
    fn conditional_borrow_in_branches() {
        let errors = borrow_check(
            "fn read(r: &i64) -> i64:\n    return 0\n\nfn caller(cond: bool) -> i64:\n    let x = 42\n    let y = 0\n    if cond:\n        read(&x)\n    else:\n        read(&y)\n    return x + y\n",
        );
        assert!(
            errors.is_empty(),
            "borrows in conditional branches should be valid: {:?}",
            errors
        );
    }

    #[test]
    fn copy_type_not_moved() {
        let errors =
            borrow_check("fn caller() -> i64:\n    let x = 42\n    let y = x\n    return x + y\n");
        assert!(
            errors.is_empty(),
            "copy type should not be moved: {:?}",
            errors
        );
    }

    #[test]
    fn struct_field_borrow_split() {
        let errors = borrow_check(
            "struct Point:\n    x: i64\n    y: i64\n\nfn caller() -> i64:\n    let mut p = Point(1, 2)\n    let rx = &p.x\n    let ry = &mut p.y\n    return 0\n",
        );
        assert!(
            errors.is_empty(),
            "split struct field borrows should be valid: {:?}",
            errors
        );
    }

    #[test]
    fn no_errors_on_simple_let() {
        let errors = borrow_check("let x = 42\n");
        assert!(
            errors.is_empty(),
            "simple let should have no borrow errors: {:?}",
            errors
        );
    }

    #[test]
    fn no_errors_on_fn_with_args() {
        let errors = borrow_check("fn add(a: i64, b: i64) -> i64:\n    return a + b\n");
        assert!(
            errors.is_empty(),
            "simple fn should have no borrow errors: {:?}",
            errors
        );
    }

    #[test]
    fn no_errors_on_while_loop() {
        let errors = borrow_check(
            "fn sum(n: i64) -> i64:\n    let mut s = 0\n    let mut i = 0\n    while i < n:\n        s = s + i\n        i = i + 1\n    return s\n",
        );
        assert!(
            errors.is_empty(),
            "while loop should have no borrow errors: {:?}",
            errors
        );
    }

    #[test]
    fn no_errors_on_if_else() {
        let errors = borrow_check(
            "fn abs(x: i64) -> i64:\n    if x < 0:\n        return 0 - x\n    return x\n",
        );
        assert!(
            errors.is_empty(),
            "if/else should have no borrow errors: {:?}",
            errors
        );
    }
}
