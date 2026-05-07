#[cfg(test)]
mod semantic_tests_extended {
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::semantic::{Resolver, TypeChecker};

    fn typeck(src: &str) -> TypeChecker {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        let mut tc = TypeChecker::new();
        tc.check_program(&prog);
        tc
    }

    fn resolve(src: &str) -> Resolver {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        r
    }

    fn err_codes(tc: &TypeChecker) -> Vec<String> {
        tc.errors
            .iter()
            .filter_map(|e| e.to_diagnostic().code().map(str::to_string))
            .collect()
    }

    #[test]
    fn unresolved_name_reported() {
        let r = resolve("let x = undefined_var\n");
        assert!(!r.errors.is_empty(), "should report undefined name");
    }

    #[test]
    fn ffi_list_param_is_not_abi_safe() {
        let tc =
            typeck("import \"/usr/lib/libc.so.6\" as libc:\n    fn bad(x: list[i64]) -> i64\n");
        assert!(err_codes(&tc).contains(&"E0421".to_string()));
    }

    #[test]
    fn ffi_scalar_param_is_abi_safe() {
        let tc = typeck("import \"/usr/lib/libc.so.6\" as libc:\n    fn ok(x: i64) -> i64\n");
        assert!(!err_codes(&tc).contains(&"E0421".to_string()));
    }

    #[test]
    fn ffi_struct_field_managed_type_flagged() {
        let tc = typeck(
            "import \"/usr/lib/libc.so.6\" as libc:\n    struct Bad:\n        items: list[i64]\n",
        );
        assert!(err_codes(&tc).contains(&"E0421".to_string()));
    }

    #[test]
    fn py_unknown_attribute_reported() {
        let tc = typeck(
            "import py \"math\" as m:\n    fn sqrt(x: float) -> float\n\nfn main():\n    let r = m.cbrt(8.0)\n",
        );
        assert!(err_codes(&tc).contains(&"E0601".to_string()));
    }

    #[test]
    fn py_wrong_arity_reported() {
        let tc = typeck(
            "import py \"math\" as m:\n    fn sqrt(x: float) -> float\n\nfn main():\n    let r = m.sqrt(1.0, 2.0)\n",
        );
        assert!(err_codes(&tc).contains(&"E0602".to_string()));
    }

    #[test]
    fn py_correct_call_no_error() {
        let tc = typeck(
            "import py \"math\" as m:\n    fn sqrt(x: float) -> float\n\nfn main():\n    let r = m.sqrt(4.0)\n",
        );
        let codes = err_codes(&tc);
        assert!(!codes.contains(&"E0601".to_string()));
        assert!(!codes.contains(&"E0602".to_string()));
    }

    #[test]
    fn type_inference_nested_calls() {
        let tc = typeck(
            "fn add(a: i64, b: i64) -> i64:\n    return a + b\n\nlet r = add(add(1, 2), add(3, 4))\n",
        );
        assert!(tc.errors.is_empty(), "errors: {:?}", tc.errors);
    }

    #[test]
    fn branch_type_unification_no_error() {
        let tc = typeck(
            "fn f(x: i64):\n    if x > 0:\n        let y = 1\n    else:\n        let y = \"wrong\"\n",
        );
        assert!(tc.errors.is_empty(), "errors: {:?}", tc.errors);
    }

    #[test]
    fn unannotated_mixed_list_widens_to_any() {
        // A heterogeneous literal with no annotation is inferred as `[Any]`
        // rather than rejected.
        let tc = typeck("let xs = [1, \"hello\", 3]\n");
        assert!(
            tc.errors.is_empty(),
            "mixed-type list should widen to [Any], got: {:?}",
            tc.errors
        );
    }

    #[test]
    fn annotated_element_type_still_enforced() {
        // Soundness: an explicit element type rejects an incompatible element.
        let tc = typeck("let xs: [str] = [\"a\", 5]\n");
        assert!(
            !tc.errors.is_empty(),
            "a `[str]` annotation must reject a non-string element"
        );
    }

    #[test]
    fn return_type_missing_in_branch_reported() {
        let tc = typeck("fn f(x: i64) -> i64:\n    if x > 0:\n        return x\n");
        assert!(
            !tc.errors.is_empty(),
            "missing return on non-taken branch should be reported"
        );
    }

    #[test]
    fn struct_field_type_mismatch() {
        let tc = typeck("struct Point:\n    x: i64\n    y: i64\n\nlet p = Point(1, \"bad\")\n");
        assert!(!tc.errors.is_empty(), "wrong field type should error");
    }

    #[test]
    fn wrong_number_of_struct_fields() {
        let tc = typeck("struct Point:\n    x: i64\n    y: i64\n\nlet p = Point(1)\n");
        assert!(!tc.errors.is_empty(), "missing field should error");
    }

    #[test]
    fn enum_wrong_variant_type() {
        let tc = typeck("enum Opt:\n    Some(i64)\n\nlet o = Some(\"bad\")\n");
        assert!(
            !tc.errors.is_empty(),
            "wrong enum variant type should error"
        );
    }

    #[test]
    fn match_non_exhaustive_reported() {
        let tc = typeck(
            "enum Color:\n    Red\n    Green\n    Blue\n\nfn f(c: Color):\n    match c:\n        case Red:\n            pass\n        case Green:\n            pass\n",
        );
        assert!(!tc.errors.is_empty(), "non-exhaustive match should error");
    }

    #[test]
    fn trait_bound_violation_reported() {
        let tc = typeck(
            "trait Numeric:\n    fn add(self, other: Self) -> Self:\n        return self\n\nfn double[T](x: T) -> T:\n    return x + x\n",
        );
        assert!(
            !tc.errors.is_empty(),
            "trait bound violation should be reported"
        );
    }

    #[test]
    fn immutable_borrow_of_mutable() {
        let tc = typeck("let mut x = 42\nlet r = &x\n");
        assert!(
            tc.errors.is_empty(),
            "immutable borrow of mutable should be ok"
        );
    }

    #[test]
    fn mutable_borrow_of_immutable_reported() {
        let tc = typeck("let x = 42\nlet r = &mut x\n");
        assert!(
            !tc.errors.is_empty(),
            "mutable borrow of immutable should be reported"
        );
    }

    #[test]
    fn function_call_wrong_arg_count_reported() {
        let tc = typeck("fn f(a: i64, b: i64) -> i64:\n    return a + b\n\nf(1)\n");
        assert!(
            !tc.errors.is_empty(),
            "wrong arg count should produce a type error"
        );
    }

    #[test]
    fn assign_to_different_type_reported() {
        let tc = typeck("let mut x: i64 = 0\nx = \"hello\"\n");
        assert!(
            !tc.errors.is_empty(),
            "type mismatch in assignment should error"
        );
    }

    #[test]
    fn tuple_destructuring_length_mismatch_reported() {
        let tc = typeck("let a, b, c = 1, 2\n");
        assert!(!tc.errors.is_empty(), "tuple length mismatch should error");
    }

    #[test]
    fn function_with_vararg_ok() {
        let tc = typeck("fn f(a: i64, *args: i64) -> i64:\n    return a\n\nf(1, 2, 3)\n");
        assert!(
            tc.errors.is_empty(),
            "vararg function should be valid: {:?}",
            tc.errors
        );
    }

    #[test]
    fn return_type_inference_ok() {
        let tc = typeck("fn f(x: i64):\n    return x + 1\n");
        assert!(
            tc.errors.is_empty(),
            "return type inference should succeed: {:?}",
            tc.errors
        );
    }

    #[test]
    fn shadowing_in_nested_scope() {
        let tc = typeck("let x = 1\nif True:\n    let x = \"hello\"\n    pass\n");
        assert!(
            tc.errors.is_empty(),
            "shadowing in nested scope should be ok"
        );
    }

    #[test]
    fn comparison_result_type() {
        let tc = typeck("let b = 1 < 2\n");
        assert!(tc.errors.is_empty(), "comparison should produce bool");
    }

    #[test]
    fn resolver_undefined_variable() {
        let r = resolve("x = 1\n");
        assert!(!r.errors.is_empty(), "assign to undefined should error");
    }

    #[test]
    fn resolver_duplicate_param() {
        let r = resolve("fn f(x: i64, x: i64):\n    pass\n");
        assert!(!r.errors.is_empty(), "duplicate params should error");
    }

    #[test]
    fn resolver_use_before_let() {
        let r = resolve("let y = x\nlet x = 1\n");
        assert!(!r.errors.is_empty(), "use before let should error");
    }

    #[test]
    fn resolver_fn_hoisting() {
        let r = resolve(
            "fn caller() -> i64:\n    return callee()\n\nfn callee() -> i64:\n    return 42\n",
        );
        assert!(r.errors.is_empty(), "function hoisting should work");
    }

    #[test]
    fn resolver_struct_hoisting() {
        let r = resolve(
            "fn make() -> Point:\n    return Point(1, 2)\n\nstruct Point:\n    x: i64\n    y: i64\n",
        );
        assert!(r.errors.is_empty(), "struct hoisting should work");
    }

    #[test]
    fn resolver_undefined_in_fn_body() {
        let r = resolve("fn f():\n    print(undefined)\n");
        assert!(!r.errors.is_empty(), "undefined in fn body should error");
    }

    #[test]
    fn resolver_import_alias() {
        let r = resolve("import \"/usr/lib/libc.so.6\" as libc:\n    fn getpid() -> i64\n");
        assert!(r.errors.is_empty());
    }

    #[test]
    fn resolver_shadowing_in_nested_scope() {
        let r = resolve("let x = 1\nif True:\n    let x = 2\n    pass\n");
        assert!(r.errors.is_empty(), "shadowing should be ok");
    }

    #[test]
    fn function_with_no_return_type_defaults_to_void() {
        let tc = typeck("fn f():\n    pass\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn tuple_type_inference() {
        let tc = typeck("let t = (1, \"a\", True)\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn nested_type_inference_ok() {
        let tc = typeck("let a = 1\nlet b = a + 2\nlet c = b * 3\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn function_with_void_return_ok() {
        let tc = typeck("fn f():\n    pass\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn multiple_let_bindings_ok() {
        let tc = typeck("let x = 42\nlet y = \"hello\"\nlet z = True\n");
        assert!(tc.errors.is_empty());
    }
}
