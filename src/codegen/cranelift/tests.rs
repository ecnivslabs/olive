#[cfg(test)]
mod codegen_tests {
    use crate::test_utils::{call_i64, call_i64_1, call_i64_2, compile};

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
            "enum Opt:\n    Some(i64)\n    Nil(i64)\n\nfn unwrap_or(v: i64, default: i64) -> i64:\n    let o = Some(v)\n    match o:\n        case Some(x):\n            return x\n        case Nil(x):\n            return default\n",
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

    #[test]
    fn trait_object_dynamic_dispatch() {
        let code = r#"
trait Animal:
    fn speak(self) -> i64:
        return 0

struct Dog:
    sound: i64

impl Animal for Dog:
    fn speak(self) -> i64:
        return self.sound

fn make_sound(a: Animal) -> i64:
    return a.speak()

fn main() -> i64:
    let d = Dog(42)
    return make_sound(d)
"#;
        let mut cg = compile(code);
        assert_eq!(call_i64(&mut cg, "main"), 42);
    }

    #[test]
    fn dict_get_default_present() {
        let mut cg = compile(
            "fn f() -> int:\n    let d: {str: int} = {\"a\": 5}\n    return d.get(\"a\", 99)\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 5);
    }

    #[test]
    fn dict_get_default_missing() {
        let mut cg = compile(
            "fn f() -> int:\n    let d: {str: int} = {\"a\": 5}\n    return d.get(\"z\", 99)\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 99);
    }

    #[test]
    fn dict_remove_resolves() {
        let mut cg = compile(
            "fn f() -> int:\n    let mut d: {str: int} = {\"a\": 5, \"b\": 6}\n    d.remove(\"a\")\n    return d.get(\"a\", -1)\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), -1);
    }

    #[test]
    fn float_arg_to_builtin_passes() {
        // Float args reach a float-typed builtin (pow) intact now that the call
        // path reads the callee signature instead of a hardcoded name list.
        let mut cg = compile("fn f() -> int:\n    return int(2.0 ** 10.0)\n");
        assert_eq!(call_i64(&mut cg, "f"), 1024);
    }

    #[test]
    fn trait_object_local_and_return_dispatch() {
        // A struct widened into a trait-object local or returned through a
        // trait-typed factory must build a fat pointer and dispatch dynamically.
        let src = "trait T:\n    fn v(self) -> int:\n        0\nstruct A:\n    n: int\nimpl T for A:\n    fn v(self) -> int:\n        self.n\nfn mk(n: int) -> T:\n    A(n)\nfn f() -> int:\n    let p: T = A(7)\n    let q = mk(35)\n    return p.v() + q.v()\n";
        let mut cg = compile(src);
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn indexing_borrowed_list_does_not_consume_elements() {
        // Binding `xs[i]` from a borrowed `&[...]` must be a view, not an owned
        // value; otherwise the binding frees an element the caller still owns and
        // a later read returns garbage. `probe` reads two elements of the borrow,
        // then the caller reads index 1 again and must still see it.
        let src = "fn probe(xs: &[[int]], i: int) -> int:\n    let a: [int] = xs[i]\n    let b: [int] = xs[i + 1]\n    return a[0] + b[0]\nfn f() -> int:\n    let data: [[int]] = [[10], [20], [30]]\n    let p = probe(&data, 0)\n    let second: [int] = data[1]\n    return p + second[0]\n";
        let mut cg = compile(src);
        assert_eq!(call_i64(&mut cg, "f"), 50);
    }

    #[test]
    fn forward_method_call_resolves_union_return() {
        // A method that calls a sibling method defined LATER in the same `impl`
        // must see its real `int | E` return type, so the `match` destructures.
        let src = "enum E:\n    Bad\nstruct S:\n    n: int\nimpl S:\n    fn caller(self) -> int:\n        match self.callee():\n            Bad:\n                return -1\n            v:\n                return v\n    fn callee(self) -> int | E:\n        if self.n == 0:\n            return Bad\n        return self.n\nfn f() -> int:\n    return S(9).caller()\n";
        let mut cg = compile(src);
        assert_eq!(call_i64(&mut cg, "f"), 9);
    }

    #[test]
    fn borrowed_tuple_list_reiterated_in_loop() {
        // Iterating a borrow of a `[(int, int)]` yields views the list still
        // owns; the yielded tuple must not be freed at loop-body exit, so a
        // re-iteration inside a `while` keeps seeing every element.
        let src = "fn build(items: &[(int, int)], loops: int) -> int:\n    let mut count = 0\n    let mut l = 0\n    while l < loops:\n        for a, b in items:\n            count = count + a + b\n        l += 1\n    return count\nfn f() -> int:\n    let segs: [(int, int)] = [(1, 2), (3, 4)]\n    return build(&segs, 3)\n";
        let mut cg = compile(src);
        assert_eq!(call_i64(&mut cg, "f"), 30);
    }

    #[test]
    fn borrowed_tuple_list_destructure_types() {
        // Iterating a borrow of a `[(float, float)]` must bind the tuple parts as
        // `float`, so a `.1f` format spec resolves the float path. Returning the
        // truncated sum proves both parts kept their float type.
        let src = "fn f() -> int:\n    let segs: [(float, float)] = [(1.5, 2.5), (3.0, 4.0)]\n    let mut total = 0.0\n    for s, e in &segs:\n        total = total + s + e\n    return int(total)\n";
        let mut cg = compile(src);
        assert_eq!(call_i64(&mut cg, "f"), 11);
    }

    #[test]
    fn trait_option_return_and_match() {
        // A factory returning `Trait | None` must build a fat pointer for the
        // struct case and a bare `None` otherwise; the match narrows the catch-all
        // to the trait and dispatches, while `None` matches the empty case.
        let src = "trait T:\n    fn v(self) -> int:\n        0\nstruct A:\n    n: int\nimpl T for A:\n    fn v(self) -> int:\n        self.n\nfn get(found: bool) -> T | None:\n    if found:\n        return A(20)\n    return None\nfn pick(found: bool) -> int:\n    match get(found):\n        None:\n            return -1\n        p:\n            return p.v()\nfn f() -> int:\n    return pick(True) + pick(False)\n";
        let mut cg = compile(src);
        assert_eq!(call_i64(&mut cg, "f"), 19);
    }

    #[test]
    fn method_scalar_arg_boxes_into_any_param() {
        // A bool passed to a method's `Any` parameter must box (tag) so it round
        // trips as a bool, not a bare word. `True` boxed reads back truthy.
        let src = "struct H:\n    v: Any\nimpl H:\n    fn __init__(self):\n        self.v = 0\n    fn put(self, x: Any):\n        self.v = x\n    fn truthy(self) -> bool:\n        self.v == True\nfn f() -> int:\n    let mut h = H()\n    h.put(True)\n    if h.truthy():\n        return 1\n    return 0\n";
        let mut cg = compile(src);
        assert_eq!(call_i64(&mut cg, "f"), 1);
    }

    #[test]
    fn struct_or_none_catch_all_binding() {
        // A catch-all binding after a `None` arm over `Struct | None` keeps the
        // full union (a struct member still carries the union tag), so the match
        // lowers without collapsing to a bare struct and faulting.
        let src = "struct R:\n    x: int\nfn make(b: int) -> R | None:\n    if b == 0:\n        return None\n    return R(7)\nfn f() -> int:\n    match make(1):\n        None:\n            return -1\n        _r:\n            return 0\n";
        let mut cg = compile(src);
        assert_eq!(call_i64(&mut cg, "f"), 0);
    }
}
