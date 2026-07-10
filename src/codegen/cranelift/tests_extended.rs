#[cfg(test)]
mod codegen_tests_extended {
    use crate::test_utils::{call_i64, call_i64_1, call_i64_2, call_i64_3, compile};

    #[test]
    fn multiplication_by_negative() {
        let mut cg = compile("fn f(x: i64) -> i64:\n    return x * -1\n");
        assert_eq!(call_i64_1(&mut cg, "f", 42), -42);
        assert_eq!(call_i64_1(&mut cg, "f", -10), 10);
    }

    #[test]
    fn composite_arithmetic() {
        let mut cg = compile("fn f(a: i64, b: i64, c: i64) -> i64:\n    return a * b + c\n");
        assert_eq!(call_i64_3(&mut cg, "f", 5, 6, 12), 42);
    }

    #[test]
    fn nested_arithmetic_parens() {
        let mut cg = compile("fn f(a: i64, b: i64, c: i64) -> i64:\n    return (a + b) * c\n");
        assert_eq!(call_i64_3(&mut cg, "f", 3, 4, 6), 42);
    }

    #[test]
    fn modulo_edge_cases() {
        let mut cg = compile("fn f(a: i64, b: i64) -> i64:\n    return a % b\n");
        assert_eq!(call_i64_2(&mut cg, "f", 10, 3), 1);
        assert_eq!(call_i64_2(&mut cg, "f", 7, 7), 0);
        assert_eq!(call_i64_2(&mut cg, "f", 0, 5), 0);
    }

    #[test]
    fn while_loop_edge_empty_body() {
        let mut cg = compile(
            "fn f(n: i64) -> i64:\n    let mut i = 0\n    while i < n:\n        i = i + 1\n    return i\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 0), 0);
        assert_eq!(call_i64_1(&mut cg, "f", 1), 1);
        assert_eq!(call_i64_1(&mut cg, "f", 10), 10);
    }

    #[test]
    fn nested_while_loops() {
        let mut cg = compile(
            "fn f(n: i64) -> i64:\n    let mut r = 0\n    let mut i = 0\n    while i < n:\n        let mut j = 0\n        while j < n:\n            r = r + 1\n            j = j + 1\n        i = i + 1\n    return r\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 0), 0);
        assert_eq!(call_i64_1(&mut cg, "f", 3), 9);
        assert_eq!(call_i64_1(&mut cg, "f", 10), 100);
    }

    #[test]
    fn multiple_elif_branches() {
        let mut cg = compile(
            "fn f(x: i64) -> i64:\n    if x > 0:\n        return 1\n    elif x < 0:\n        return -1\n    return 0\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 5), 1);
        assert_eq!(call_i64_1(&mut cg, "f", -5), -1);
        assert_eq!(call_i64_1(&mut cg, "f", 0), 0);
    }

    #[test]
    fn nested_if_inside_while() {
        let mut cg = compile(
            "fn f(n: i64) -> i64:\n    let mut r = 0\n    let mut i = 0\n    while i < n:\n        if i % 2 == 0:\n            r = r + i\n        i = i + 1\n    return r\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 10), 20);
    }

    #[test]
    fn and_operator_truth_table() {
        let mut cg = compile(
            "fn f(a: i64, b: i64) -> i64:\n    if a != 0 and b != 0:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_2(&mut cg, "f", 0, 0), 0);
        assert_eq!(call_i64_2(&mut cg, "f", 1, 0), 0);
        assert_eq!(call_i64_2(&mut cg, "f", 0, 1), 0);
        assert_eq!(call_i64_2(&mut cg, "f", 1, 1), 1);
    }

    #[test]
    fn or_operator_truth_table() {
        let mut cg = compile(
            "fn f(a: i64, b: i64) -> i64:\n    if a != 0 or b != 0:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_2(&mut cg, "f", 0, 0), 0);
        assert_eq!(call_i64_2(&mut cg, "f", 1, 0), 1);
        assert_eq!(call_i64_2(&mut cg, "f", 0, 1), 1);
        assert_eq!(call_i64_2(&mut cg, "f", 1, 1), 1);
    }

    #[test]
    fn not_operator() {
        let mut cg =
            compile("fn f(x: bool) -> i64:\n    if not x:\n        return 1\n    return 0\n");
        assert_eq!(call_i64_1(&mut cg, "f", 0), 1);
        assert_eq!(call_i64_1(&mut cg, "f", 1), 0);
    }

    #[test]
    fn bitwise_and_or_xor() {
        let mut cg = compile("fn f(a: i64, b: i64) -> i64:\n    return (a & b) | (a ^ b)\n");
        assert_eq!(call_i64_2(&mut cg, "f", 0b1010, 0b1100), 0b1110);
        assert_eq!(call_i64_2(&mut cg, "f", 0xFF, 0xFF), 0xFF);
        assert_eq!(call_i64_2(&mut cg, "f", 0, 0), 0);
    }

    #[test]
    fn bitwise_not() {
        let mut cg = compile("fn f(x: i64) -> i64:\n    return ~x\n");
        assert_eq!(call_i64_1(&mut cg, "f", 0), -1);
        assert_eq!(call_i64_1(&mut cg, "f", -1), 0);
        assert_eq!(call_i64_1(&mut cg, "f", 42), !42i64);
    }

    #[test]
    fn const_bool_in_if() {
        let mut cg = compile("fn f() -> i64:\n    if True:\n        return 42\n    return 0\n");
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn short_circuit_and() {
        let mut cg = compile(
            "fn f() -> i64:\n    if False and (1 / 0) != 0:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 0);
    }

    #[test]
    fn short_circuit_or() {
        let mut cg = compile(
            "fn f() -> i64:\n    if True or (1 / 0) != 0:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 1);
    }

    #[test]
    fn empty_return() {
        let mut cg = compile("fn f():\n    return\n");
        let ptr = cg.get_function("f").unwrap();
        let f: extern "C" fn() = unsafe { std::mem::transmute(ptr) };
        f();
    }

    #[test]
    fn multiple_returns() {
        let mut cg = compile(
            "fn f(x: i64) -> i64:\n    if x == 0:\n        return 0\n    elif x == 1:\n        return 1\n    elif x == 2:\n        return 2\n    return -1\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 0), 0);
        assert_eq!(call_i64_1(&mut cg, "f", 1), 1);
        assert_eq!(call_i64_1(&mut cg, "f", 2), 2);
        assert_eq!(call_i64_1(&mut cg, "f", 3), -1);
    }

    #[test]
    fn if_else_as_expression() {
        let mut cg =
            compile("fn abs(x: i64) -> i64:\n    if x >= 0:\n        return x\n    return 0 - x\n");
        assert_eq!(call_i64_1(&mut cg, "abs", -7), 7);
        assert_eq!(call_i64_1(&mut cg, "abs", 7), 7);
        assert_eq!(call_i64_1(&mut cg, "abs", 0), 0);
    }

    #[test]
    fn scoped_shadowing() {
        let mut cg = compile(
            "fn f(x: i64) -> i64:\n    if True:\n        let x = 99\n        return x\n    return x\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 42), 99);
    }

    #[test]
    fn multi_let_destructuring() {
        let mut cg = compile("fn f() -> i64:\n    let a, b = 10, 32\n    return a + b\n");
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn nested_structs_and_methods() {
        let mut cg = compile(
            "struct Point:\n    x: i64\n    y: i64\n\nstruct Rect:\n    min: Point\n    max: Point\n\nimpl Rect:\n    fn area(self) -> i64:\n        let w = self.max.x - self.min.x\n        let h = self.max.y - self.min.y\n        return w * h\n\nfn f() -> i64:\n    let r = Rect(Point(0, 0), Point(3, 4))\n    return r.area()\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 12);
    }

    #[test]
    fn enum_with_multiple_variants() {
        let mut cg = compile(
            "enum Shape:\n    Circle(i64)\n    Rect(i64, i64)\n\nfn area(s: Shape) -> i64:\n    match s:\n        case Circle(r):\n            return r * r\n        case Rect(w, h):\n            return w * h\n\nfn f() -> i64:\n    let c = Circle(7)\n    let r = Rect(3, 4)\n    return area(c) + area(r)\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 49 + 12);
    }

    #[test]
    fn generic_identity_struct() {
        let mut cg = compile(
            "struct Wrapper[T]:\n    val: T\n\nfn id(x: i64) -> Wrapper[i64]:\n    return Wrapper(x)\n\nfn f() -> i64:\n    let w = id(42)\n    return w.val\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn generic_identity() {
        let mut cg =
            compile("fn id[T](x: T) -> T:\n    return x\n\nfn f() -> i64:\n    return id(42)\n");
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn generic_pair_swap() {
        let mut cg = compile(
            "struct Pair[A, B]:\n    first: A\n    second: B\n\nfn swap(p: Pair[i64, i64]) -> Pair[i64, i64]:\n    return Pair(p.second, p.first)\n\nfn f() -> i64:\n    let p = Pair(10, 32)\n    let q = swap(p)\n    return q.first + q.second\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn enum_match_wildcard() {
        let mut cg = compile(
            "enum Opt:\n    Some(i64)\n    Nil\n\nfn f(n: i64) -> i64:\n    let o = Some(n)\n    match o:\n        case Some(v):\n            return v\n        case _:\n            return 0\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 42), 42);
    }

    #[test]
    fn enum_match_some() {
        let mut cg = compile(
            "enum Opt:\n    Some(i64)\n    Nil\n\nfn f(n: i64) -> i64:\n    let o = Some(n)\n    match o:\n        case Some(v):\n            return v\n        case Nil:\n            return 0\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 42), 42);
    }

    #[test]
    fn multiple_generic_functions() {
        let mut cg = compile(
            "fn id[T](x: T) -> T:\n    return x\n\nfn f() -> i64:\n    return id(id(42))\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn large_constant_int() {
        let mut cg = compile("fn f() -> i64:\n    return 2147483647\n");
        assert_eq!(call_i64(&mut cg, "f"), 2147483647);
    }

    #[test]
    fn negative_constant() {
        let mut cg = compile("fn f() -> i64:\n    return -42\n");
        assert_eq!(call_i64(&mut cg, "f"), -42);
    }

    #[test]
    fn zero_constant() {
        let mut cg = compile("fn f() -> i64:\n    return 0\n");
        assert_eq!(call_i64(&mut cg, "f"), 0);
    }

    #[test]
    fn large_loop_count() {
        let mut cg = compile(
            "fn f(n: i64) -> i64:\n    let mut s = 0\n    let mut i = 0\n    while i < n:\n        s = s + 1\n        i = i + 1\n    return s\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 1000), 1000);
        assert_eq!(call_i64_1(&mut cg, "f", 10000), 10000);
    }

    #[test]
    fn recursive_sum() {
        let mut cg = compile(
            "fn sum(n: i64) -> i64:\n    if n <= 0:\n        return 0\n    return n + sum(n - 1)\n",
        );
        assert_eq!(call_i64_1(&mut cg, "sum", 10), 55);
        assert_eq!(call_i64_1(&mut cg, "sum", 100), 5050);
    }

    #[test]
    fn recursive_sum_not_tail() {
        let mut cg = compile(
            "fn f(n: i64) -> i64:\n    if n <= 0:\n        return 0\n    return n + f(n - 1)\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 3), 6);
    }

    #[test]
    fn gcd_recursive() {
        let mut cg = compile(
            "fn gcd(a: i64, b: i64) -> i64:\n    let mut x = a\n    let mut y = b\n    while y != 0:\n        let t = y\n        y = x % y\n        x = t\n    return x\n",
        );
        assert_eq!(call_i64_2(&mut cg, "gcd", 48, 18), 6);
    }

    #[test]
    fn prime_check() {
        let mut cg = compile(
            "fn is_prime(n: i64) -> i64:\n    if n <= 1:\n        return 0\n    let mut i = 2\n    while i * i <= n:\n        if n % i == 0:\n            return 0\n        i = i + 1\n    return 1\n",
        );
        assert_eq!(call_i64_1(&mut cg, "is_prime", 2), 1);
        assert_eq!(call_i64_1(&mut cg, "is_prime", 17), 1);
        assert_eq!(call_i64_1(&mut cg, "is_prime", 25), 0);
        assert_eq!(call_i64_1(&mut cg, "is_prime", 97), 1);
    }

    #[test]
    fn for_loop_over_list() {
        let mut cg = compile(
            "fn f() -> i64:\n    let mut s = 0\n    for x in [1, 2, 3, 4]:\n        s = s + x\n    return s\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 10);
    }

    #[test]
    fn for_loop_borrows_iterable() {
        let mut cg = compile(
            "fn f() -> i64:\n    let xs = [1, 2, 3]\n    let mut s = 0\n    for x in xs:\n        s = s + x\n    return s + len(xs)\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 9);
    }

    #[test]
    fn for_loop_borrows_nested_tuple() {
        let mut cg = compile(
            "fn f() -> i64:\n    let segs = [(1, 2), (3, 4)]\n    let mut total = 0\n    for a, b in segs:\n        total = total + a + b\n    total + len(segs)\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 12);
    }

    #[test]
    fn integer_comparison_chain() {
        let mut cg = compile(
            "fn f(a: i64, b: i64, c: i64) -> i64:\n    if a < b:\n        if b < c:\n            return 42\n    return 0\n",
        );
        assert_eq!(call_i64_3(&mut cg, "f", 1, 2, 3), 42);
        assert_eq!(call_i64_3(&mut cg, "f", 3, 2, 1), 0);
    }

    #[test]
    fn struct_method_mutation() {
        let mut cg = compile(
            "struct Counter:\n    n: i64\n\nimpl Counter:\n    fn inc(self) -> Counter:\n        return Counter(self.n + 1)\n\nfn f(n: i64) -> i64:\n    let c = Counter(n)\n    let c2 = c.inc()\n    return c2.n\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 41), 42);
    }

    #[test]
    fn boolean_arithmetic() {
        let mut cg = compile(
            "fn f(a: bool, b: bool) -> i64:\n    if a and b:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_2(&mut cg, "f", 1, 1), 1);
        assert_eq!(call_i64_2(&mut cg, "f", 1, 0), 0);
    }
}
