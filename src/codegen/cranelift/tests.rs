#[cfg(test)]
mod codegen_tests {
    use crate::codegen::cranelift::CraneliftCodegen;
    use crate::lexer::Lexer;
    use crate::mir::{MirBuilder, Optimizer};
    use crate::parser::Parser;
    use crate::semantic::{Resolver, TypeChecker};

    fn compile(src: &str) -> CraneliftCodegen<cranelift_jit::JITModule> {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        assert!(r.errors.is_empty(), "resolver errors: {:?}", r.errors);
        let mut tc = TypeChecker::new();
        tc.check_program(&prog);
        assert!(tc.errors.is_empty(), "type errors: {:?}", tc.errors);
        let mut builder =
            MirBuilder::new(&tc.expr_types, &tc.type_env[0], tc.struct_fields.clone());
        builder.build_program(&prog);
        let opt = Optimizer::new();
        opt.run(&mut builder.functions);
        let mut cg = CraneliftCodegen::new_jit(builder.functions, builder.struct_fields, &[]);
        cg.generate();
        cg.finalize();
        cg
    }

    fn call_i64(cg: &mut CraneliftCodegen<cranelift_jit::JITModule>, name: &str) -> i64 {
        let ptr = cg
            .get_function(name)
            .unwrap_or_else(|| panic!("function '{}' not found", name));
        let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(ptr) };
        f()
    }

    fn call_i64_1(cg: &mut CraneliftCodegen<cranelift_jit::JITModule>, name: &str, a: i64) -> i64 {
        let ptr = cg
            .get_function(name)
            .unwrap_or_else(|| panic!("function '{}' not found", name));
        let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(ptr) };
        f(a)
    }

    fn call_i64_2(
        cg: &mut CraneliftCodegen<cranelift_jit::JITModule>,
        name: &str,
        a: i64,
        b: i64,
    ) -> i64 {
        let ptr = cg
            .get_function(name)
            .unwrap_or_else(|| panic!("function '{}' not found", name));
        let f: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
        f(a, b)
    }

    #[test]
    fn integer_constant_return() {
        let mut cg = compile("fn f() -> i64:\n    return 42\n");
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn addition() {
        let mut cg = compile("fn add(a: i64, b: i64) -> i64:\n    return a + b\n");
        assert_eq!(call_i64_2(&mut cg, "add", 10, 32), 42);
    }

    #[test]
    fn subtraction() {
        let mut cg = compile("fn sub(a: i64, b: i64) -> i64:\n    return a - b\n");
        assert_eq!(call_i64_2(&mut cg, "sub", 50, 8), 42);
    }

    #[test]
    fn multiplication() {
        let mut cg = compile("fn mul(a: i64, b: i64) -> i64:\n    return a * b\n");
        assert_eq!(call_i64_2(&mut cg, "mul", 6, 7), 42);
    }

    #[test]
    fn integer_division() {
        let mut cg = compile("fn div(a: i64, b: i64) -> i64:\n    return a / b\n");
        assert_eq!(call_i64_2(&mut cg, "div", 84, 2), 42);
    }

    #[test]
    fn modulo() {
        let mut cg = compile("fn md(a: i64, b: i64) -> i64:\n    return a % b\n");
        assert_eq!(call_i64_2(&mut cg, "md", 100, 58), 42);
    }

    #[test]
    fn if_true_branch() {
        let mut cg =
            compile("fn f(x: i64) -> i64:\n    if x > 0:\n        return 1\n    return 0\n");
        assert_eq!(call_i64_1(&mut cg, "f", 5), 1);
    }

    #[test]
    fn if_false_branch() {
        let mut cg =
            compile("fn f(x: i64) -> i64:\n    if x > 0:\n        return 1\n    return 0\n");
        assert_eq!(call_i64_1(&mut cg, "f", -1), 0);
    }

    #[test]
    fn if_else_branch() {
        let mut cg =
            compile("fn abs(x: i64) -> i64:\n    if x < 0:\n        return 0 - x\n    return x\n");
        assert_eq!(call_i64_1(&mut cg, "abs", -7), 7);
        assert_eq!(call_i64_1(&mut cg, "abs", 7), 7);
    }

    #[test]
    fn while_loop_sum() {
        let mut cg = compile(
            "fn sum(n: i64) -> i64:\n    let mut s = 0\n    let mut i = 1\n    while i <= n:\n        s = s + i\n        i = i + 1\n    return s\n",
        );
        assert_eq!(call_i64_1(&mut cg, "sum", 10), 55);
    }

    #[test]
    fn recursive_factorial() {
        let mut cg = compile(
            "fn fact(n: i64) -> i64:\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n",
        );
        assert_eq!(call_i64_1(&mut cg, "fact", 10), 3628800);
    }

    #[test]
    fn recursive_fibonacci() {
        let mut cg = compile(
            "fn fib(n: i64) -> i64:\n    if n <= 1:\n        return n\n    return fib(n - 1) + fib(n - 2)\n",
        );
        assert_eq!(call_i64_1(&mut cg, "fib", 10), 55);
    }

    #[test]
    fn nested_function_calls() {
        let mut cg = compile(
            "fn add(a: i64, b: i64) -> i64:\n    return a + b\n\nfn quad_add(a: i64, b: i64) -> i64:\n    return add(add(a, b), add(a, b))\n",
        );
        assert_eq!(call_i64_2(&mut cg, "quad_add", 3, 4), 14);
    }

    #[test]
    fn comparison_true() {
        let mut cg = compile(
            "fn gt(a: i64, b: i64) -> i64:\n    if a > b:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_2(&mut cg, "gt", 10, 5), 1);
    }

    #[test]
    fn comparison_false() {
        let mut cg = compile(
            "fn gt(a: i64, b: i64) -> i64:\n    if a > b:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_2(&mut cg, "gt", 3, 7), 0);
    }

    #[test]
    fn local_variable_mutation() {
        let mut cg = compile(
            "fn f(n: i64) -> i64:\n    let mut x = 0\n    let mut i = 0\n    while i < n:\n        x = x + 2\n        i = i + 1\n    return x\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 5), 10);
    }

    #[test]
    fn pythagorean_sum() {
        let mut cg =
            compile("fn f(a: i64, b: i64) -> i64:\n    let x = a * a + b * b\n    return x\n");
        assert_eq!(call_i64_2(&mut cg, "f", 3, 4), 25);
    }

    #[test]
    fn early_return_from_loop() {
        let mut cg = compile(
            "fn find_first_gt10(n: i64) -> i64:\n    let mut i = 0\n    while i < n:\n        if i > 10:\n            return i\n        i = i + 1\n    return -1\n",
        );
        assert_eq!(call_i64_1(&mut cg, "find_first_gt10", 20), 11);
    }

    #[test]
    fn range_check_in() {
        let mut cg = compile(
            "fn in_range(x: i64) -> i64:\n    if x >= 0 and x <= 100:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_1(&mut cg, "in_range", 50), 1);
        assert_eq!(call_i64_1(&mut cg, "in_range", 150), 0);
    }

    #[test]
    fn const_folding_correctness() {
        let mut cg = compile("fn f() -> i64:\n    let x = 2 * 3 + 4 * 5\n    return x\n");
        assert_eq!(call_i64(&mut cg, "f"), 26);
    }

    #[test]
    fn left_shift() {
        let mut cg = compile("fn f(a: i64, b: i64) -> i64:\n    return a << b\n");
        assert_eq!(call_i64_2(&mut cg, "f", 1, 8), 256);
    }

    #[test]
    fn right_shift() {
        let mut cg = compile("fn f(a: i64, b: i64) -> i64:\n    return a >> b\n");
        assert_eq!(call_i64_2(&mut cg, "f", 256, 4), 16);
    }

    #[test]
    fn negation() {
        let mut cg = compile("fn f(x: i64) -> i64:\n    return 0 - x\n");
        assert_eq!(call_i64_1(&mut cg, "f", 42), -42);
    }

    #[test]
    fn power_of_two_loop() {
        let mut cg = compile(
            "fn pow2(n: i64) -> i64:\n    let mut r = 1\n    let mut i = 0\n    while i < n:\n        r = r * 2\n        i = i + 1\n    return r\n",
        );
        assert_eq!(call_i64_1(&mut cg, "pow2", 10), 1024);
    }

    #[test]
    fn gcd_euclid() {
        let mut cg = compile(
            "fn gcd(a: i64, b: i64) -> i64:\n    let mut x = a\n    let mut y = b\n    while y != 0:\n        let t = y\n        y = x % y\n        x = t\n    return x\n",
        );
        assert_eq!(call_i64_2(&mut cg, "gcd", 48, 18), 6);
        assert_eq!(call_i64_2(&mut cg, "gcd", 100, 75), 25);
    }

    #[test]
    fn top_level_code_runs() {
        let mut cg = compile("fn main() -> i64:\n    return 0\n");
        assert_eq!(call_i64(&mut cg, "main"), 0);
    }

    #[test]
    fn function_with_locals() {
        let mut cg = compile("fn main() -> i64:\n    let x = 6 * 7\n    return x\n");
        assert_eq!(call_i64(&mut cg, "main"), 42);
    }

    #[test]
    fn struct_construction_and_field_access() {
        let mut cg = compile(
            "struct Point:\n    x: i64\n    y: i64\n\nfn sum_coords(a: i64, b: i64) -> i64:\n    let p = Point(a, b)\n    return p.x + p.y\n",
        );
        assert_eq!(call_i64_2(&mut cg, "sum_coords", 17, 25), 42);
    }

    #[test]
    fn struct_field_mutation() {
        let mut cg = compile(
            "struct Box:\n    val: i64\n\nfn f(n: i64) -> i64:\n    let mut b = Box(n)\n    b.val = b.val * 2\n    return b.val\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 21), 42);
    }

    #[test]
    fn method_dispatch() {
        let mut cg = compile(
            "struct Counter:\n    n: i64\n\nimpl Counter:\n    fn doubled(self) -> i64:\n        return self.n * 2\n\nfn f(x: i64) -> i64:\n    let c = Counter(x)\n    return c.doubled()\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 21), 42);
    }

    #[test]
    fn enum_match_variant_with_payload() {
        let mut cg = compile(
            "enum Opt:\n    Some(i64)\n    None(i64)\n\nfn unwrap_or(v: i64, default: i64) -> i64:\n    let o = Some(v)\n    match o:\n        case Some(x):\n            return x\n        case None(x):\n            return default\n",
        );
        assert_eq!(call_i64_2(&mut cg, "unwrap_or", 42, 0), 42);
        assert_eq!(call_i64_2(&mut cg, "unwrap_or", 7, 0), 7);
    }

    #[test]
    fn generic_identity_int() {
        let mut cg = compile(
            "fn id[T](x: T) -> T:\n    return x\n\nfn f(n: i64) -> i64:\n    return id(n)\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 42), 42);
    }

    #[test]
    fn generic_max() {
        let mut cg = compile(
            "fn max_val[T](a: T, b: T) -> T:\n    if a > b:\n        return a\n    return b\n\nfn f(a: i64, b: i64) -> i64:\n    return max_val(a, b)\n",
        );
        assert_eq!(call_i64_2(&mut cg, "f", 17, 42), 42);
        assert_eq!(call_i64_2(&mut cg, "f", 42, 17), 42);
    }

    #[test]
    fn multiple_functions_independent() {
        let mut cg = compile(
            "fn double(x: i64) -> i64:\n    return x * 2\n\nfn triple(x: i64) -> i64:\n    return x * 3\n\nfn f(x: i64) -> i64:\n    return double(x) + triple(x)\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 6), 30);
    }

    #[test]
    fn deeply_nested_calls() {
        let mut cg = compile(
            "fn inc(x: i64) -> i64:\n    return x + 1\n\nfn f(x: i64) -> i64:\n    return inc(inc(inc(inc(inc(x)))))\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 37), 42);
    }

    #[test]
    fn zero_arg_function() {
        let mut cg = compile("fn the_answer() -> i64:\n    return 42\n");
        assert_eq!(call_i64(&mut cg, "the_answer"), 42);
    }

    #[test]
    fn conditional_early_exit() {
        let mut cg = compile(
            "fn safe_div_10(x: i64) -> i64:\n    if x == 0:\n        return 0\n    return 10 / x\n",
        );
        assert_eq!(call_i64_1(&mut cg, "safe_div_10", 2), 5);
        assert_eq!(call_i64_1(&mut cg, "safe_div_10", 0), 0);
    }

    #[test]
    fn fibonacci_iterative() {
        let mut cg = compile(
            "fn fib_iter(n: i64) -> i64:\n    if n <= 1:\n        return n\n    let mut a = 0\n    let mut b = 1\n    let mut i = 2\n    while i <= n:\n        let c = a + b\n        a = b\n        b = c\n        i = i + 1\n    return b\n",
        );
        assert_eq!(call_i64_1(&mut cg, "fib_iter", 10), 55);
        assert_eq!(call_i64_1(&mut cg, "fib_iter", 20), 6765);
    }

    #[test]
    fn equality_check() {
        let mut cg = compile(
            "fn eq(a: i64, b: i64) -> i64:\n    if a == b:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_2(&mut cg, "eq", 5, 5), 1);
        assert_eq!(call_i64_2(&mut cg, "eq", 5, 6), 0);
    }

    #[test]
    fn inequality_check() {
        let mut cg = compile(
            "fn neq(a: i64, b: i64) -> i64:\n    if a != b:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_2(&mut cg, "neq", 5, 6), 1);
        assert_eq!(call_i64_2(&mut cg, "neq", 5, 5), 0);
    }

    #[test]
    fn less_equal() {
        let mut cg = compile(
            "fn le(a: i64, b: i64) -> i64:\n    if a <= b:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_2(&mut cg, "le", 5, 5), 1);
        assert_eq!(call_i64_2(&mut cg, "le", 6, 5), 0);
    }

    #[test]
    fn isqrt() {
        let mut cg = compile(
            "fn isqrt(n: i64) -> i64:\n    let mut r = 0\n    while (r + 1) * (r + 1) <= n:\n        r = r + 1\n    return r\n",
        );
        assert_eq!(call_i64_1(&mut cg, "isqrt", 144), 12);
        assert_eq!(call_i64_1(&mut cg, "isqrt", 100), 10);
    }

    #[test]
    fn mutual_recursion() {
        let mut cg = compile(
            "fn is_odd(n: i64) -> i64:\n    if n == 0:\n        return 0\n    return is_even(n - 1)\n\nfn is_even(n: i64) -> i64:\n    if n == 0:\n        return 1\n    return is_odd(n - 1)\n",
        );
        assert_eq!(call_i64_1(&mut cg, "is_even", 10), 1);
        assert_eq!(call_i64_1(&mut cg, "is_odd", 7), 1);
    }

    #[test]
    fn collatz_length() {
        let mut cg = compile(
            "fn collatz(n: i64) -> i64:\n    let mut x = n\n    let mut steps = 0\n    while x != 1:\n        if x % 2 == 0:\n            x = x / 2\n        else:\n            x = 3 * x + 1\n        steps = steps + 1\n    return steps\n",
        );
        assert_eq!(call_i64_1(&mut cg, "collatz", 27), 111);
    }
}
