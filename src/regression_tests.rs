#[cfg(test)]
use crate::test_utils::{call_i64, call_i64_1, call_i64_2, call_i64_3, compile};

#[test]
fn regression_struct_field_access_through_ref() {
    let mut cg = compile(
        "struct Point:\n    x: i64\n    y: i64\n\nfn f() -> i64:\n    let p = Point(42, 0)\n    return p.x + p.y\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn regression_method_dispatch() {
    let mut cg = compile(
        "struct Counter:\n    n: i64\n\nimpl Counter:\n    fn get(self) -> i64:\n        return self.n\n\nfn f(c: Counter) -> i64:\n    return c.get()\n\nfn make() -> i64:\n    let c = Counter(42)\n    return f(c)\n",
    );
    assert_eq!(call_i64(&mut cg, "make"), 42);
}

#[test]
fn regression_global_dedup() {
    let mut cg =
        compile("const X = 42\nfn f() -> i64:\n    return X\nfn g() -> i64:\n    return X\n");
    assert_eq!(call_i64(&mut cg, "f"), 42);
    assert_eq!(call_i64(&mut cg, "g"), 42);
}

#[test]
fn regression_const_in_impl() {
    let mut cg = compile(
        "struct Foo:\n    x: i64\n\nimpl Foo:\n    const ZERO = 0\n\nfn f() -> i64:\n    return Foo::ZERO\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 0);
}

#[test]
fn regression_literal_type_coercion() {
    let mut cg = compile("fn f(x: i64) -> i64:\n    return x + 0\n");
    assert_eq!(call_i64_1(&mut cg, "f", 42), 42);
}

#[test]
fn regression_ptr_load_f32() {
    let mut cg = compile(
        "struct FBuf:\n    a: f32\n    b: f32\n\nfn f() -> i64:\n    let buf = FBuf(1.5, 2.5)\n    if buf.a + buf.b > 3.0:\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn regression_odd_integer_disambiguation() {
    let mut cg = compile("fn f(n: i64) -> i64:\n    let mut x = n\n    return x\n");
    assert_eq!(call_i64_1(&mut cg, "f", 1), 1);
    assert_eq!(call_i64_1(&mut cg, "f", 3), 3);
    assert_eq!(call_i64_1(&mut cg, "f", 65535), 65535);
    assert_eq!(call_i64_1(&mut cg, "f", 65537), 65537);
    assert_eq!(call_i64_1(&mut cg, "f", 0), 0);
}

#[test]
fn regression_struct_allocation() {
    let mut cg = compile(
        "struct Point:\n    x: i64\n    y: i64\n\nfn f(x: i64) -> i64:\n    let p = Point(x, x * 2)\n    return p.x + p.y\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 14), 42);
}

#[test]
fn regression_generic_method() {
    let mut cg = compile(
        "struct Box[T]:\n    val: T\n\nimpl[T] Box[T]:\n    fn get(self) -> T:\n        return self.val\n\nfn f() -> i64:\n    let b: Box[i64] = Box(42)\n    return b.get()\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn regression_nested_generic() {
    let mut cg = compile(
        "fn id[T](x: T) -> T:\n    return x\n\nfn wrap[T](x: T) -> [T]:\n    return [x]\n\nfn f() -> i64:\n    let a = id(42)\n    let b = wrap(a)\n    return b[0]\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn regression_for_loop_list() {
    let mut cg = compile(
        "fn f() -> i64:\n    let mut s = 0\n    for x in [1, 2, 3, 4]:\n        s = s + x\n    return s\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 10);
}

#[test]
fn regression_enum_single_variant() {
    let mut cg = compile(
        "enum Wrap:\n    Val(i64)\n\nfn f(n: i64) -> i64:\n    let w = Val(n)\n    match w:\n        case Val(v):\n            return v\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 42), 42);
}

#[test]
fn regression_infinite_loop_break() {
    let mut cg = compile(
        "fn f(n: i64) -> i64:\n    let mut i = 0\n    while True:\n        if i >= n:\n            break\n        i = i + 1\n    return i\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 10), 10);
}

#[test]
fn regression_generic_recursive_call() {
    let mut cg = compile(
        "fn double(x: i64) -> i64:\n    return x * 2\n\nfn f() -> i64:\n    return double(21)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn regression_complex_condition() {
    let mut cg = compile(
        "fn f(a: i64, b: i64, c: i64) -> i64:\n    if a > 0 and b > 0 or c > 0:\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64_3(&mut cg, "f", 1, 0, 1), 1);
    assert_eq!(call_i64_3(&mut cg, "f", 0, 0, 0), 0);
}

#[test]
fn regression_nested_struct_mutation() {
    let mut cg = compile(
        "struct Inner:\n    v: i64\nstruct Outer:\n    inner: Inner\n\nfn f() -> i64:\n    let mut o = Outer(Inner(0))\n    o.inner.v = 42\n    return o.inner.v\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn regression_scoped_let_shadowing() {
    let mut cg = compile(
        "fn f() -> i64:\n    let x = 1\n    if True:\n        let x = 42\n        return x\n    return x\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn regression_nested_if_else() {
    let mut cg = compile(
        "fn f(a: i64, b: i64) -> i64:\n    if a > 0:\n        if b > 0:\n            return a + b\n        else:\n            return a\n    return 0\n",
    );
    assert_eq!(call_i64_2(&mut cg, "f", 10, 5), 15);
    assert_eq!(call_i64_2(&mut cg, "f", 10, -1), 10);
    assert_eq!(call_i64_2(&mut cg, "f", -1, 5), 0);
}

#[test]
fn regression_while_loop_mutation() {
    let mut cg = compile(
        "fn f(n: i64) -> i64:\n    let mut x = 0\n    let mut i = 1\n    while i <= n:\n        x = x + i\n        i = i + 1\n    return x\n",
    );
    assert_eq!(call_i64_1(&mut cg, "f", 10), 55);
}
