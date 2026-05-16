#[cfg(test)]
use crate::test_utils::{call_i64, call_i64_1, call_i64_2, call_i64_3, compile, compile_minimal};

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

#[test]
fn regression_any_list_large_odd_int_arithmetic() {
    // A large odd int read from an `[Any]` slot once collided with the string
    // pointer heuristic and segfaulted when added. Boxing keeps it a sound int.
    let mut cg = compile(
        "fn f() -> i64:\n    let mut xs: [Any] = [0, 0, 0]\n    let mut i = 0\n    while i < 3:\n        xs[i] = i * 200000000 + 1\n        i = i + 1\n    let mut s = 0\n    let mut k = 0\n    while k < 3:\n        s = s + xs[k]\n        k = k + 1\n    return int(s)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 600000003);
}

#[test]
fn regression_any_scalar_large_odd_int_arithmetic() {
    // The same soundness for a scalar `Any` value, not just a container slot.
    let mut cg = compile(
        "fn box_it(i: i64) -> Any:\n    return i * 200000000 + 1\n\nfn f() -> i64:\n    let mut s = 0\n    let mut k = 0\n    while k < 3:\n        let a = box_it(k)\n        s = s + a\n        k = k + 1\n    return int(s)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 600000003);
}

#[test]
fn regression_any_float_in_container_roundtrips() {
    // A float boxed into an `Any` slot keeps its value through arithmetic
    // rather than reading its raw bit pattern as an int.
    let mut cg = compile(
        "fn f() -> i64:\n    let mut xs: [Any] = [0.0, 0.0]\n    let mut i = 0\n    while i < 2:\n        xs[i] = 1.5 + float(i)\n        i = i + 1\n    return int(float(xs[0]) + float(xs[1]))\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 4);
}

#[test]
fn regression_int_of_float_does_not_box_arg() {
    // `int(float)` dispatches to a native float->int conversion on the concrete
    // argument type. The argument must not be boxed into `int`'s nominal `Any`
    // parameter (that would pick `unbox_int` and, in a loop, allocate a boxed
    // scalar per call), so the value must round-trip correctly.
    let mut cg = compile(
        "fn f(n: i64) -> i64:\n    let mut s = 0\n    let mut k = 0\n    while k < n:\n        s = s + int(7.0 * float(k) + 0.5)\n        k = k + 1\n    return s\n",
    );
    // int(7.0*k + 0.5) for k=0..5: 0,7,14,21,28 -> 70
    assert_eq!(call_i64_1(&mut cg, "f", 5), 70);
}

#[test]
fn regression_int_keyed_dict_literal_scalarized() {
    // Scalarization mapped int-keyed dict reads but skipped the literal's
    // writes, so a lookup read an uninitialized slot.
    let mut cg = compile("fn f() -> i64:\n    let d = {1: 10, 2: 20}\n    return d[1] + d[2]\n");
    assert_eq!(call_i64(&mut cg, "f"), 30);
}

#[test]
fn regression_scalarized_list_feeds_recursion() {
    // A local list scalarized to a field must carry its element type, not a
    // blanket `Any`. A wrong `Any` routed the recursion-bound arithmetic through
    // the boxing `any_*` helpers, so the recursive call saw a boxed pointer and
    // never terminated.
    let mut cg = compile(
        "fn hf(n: i64) -> i64:\n    let data = [n, n * 2]\n    let v = data[0]\n    if v <= 1:\n        return v\n    return hf(v - 1) + hf(n - 2)\n\nfn f() -> i64:\n    return hf(6)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 8);
}

#[test]
fn regression_scalarized_dict_feeds_recursion() {
    // The same soundness for a scalarized local dict driving the recursion.
    let mut cg = compile(
        "fn hf(n: i64) -> i64:\n    let info = {\"val\": n, \"next\": n + 1}\n    let v = info[\"val\"]\n    if v <= 1:\n        return v\n    return hf(v - 1) + hf(n - 2)\n\nfn f() -> i64:\n    return hf(6)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 8);
}

#[test]
fn regression_inferred_return_unboxes_for_caller() {
    // An un-annotated function returns a nested list element; the caller reads
    // it as a concrete int, so the `_return` slot must match the inferred type
    // rather than boxing into `Any`.
    let mut cg = compile(
        "fn get(n: i64):\n    let mut c = list_new(2)\n    c[0] = list_new(2)\n    let mut row = c[0]\n    row[0] = 5\n    return c[0][0]\n\nfn f() -> i64:\n    return get(2)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 5);
}

#[test]
fn regression_any_keyed_dict_int_lookup() {
    // An `Any`-typed int key hashes by value, so a separately built lookup key
    // still finds the entry.
    let mut cg = compile(
        "fn f() -> i64:\n    let mut d: {Any: i64} = {}\n    d[1] = 10\n    d[2] = 20\n    return d[1] + d[2]\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 30);
}

#[test]
fn regression_append_boxes_large_odd_into_any() {
    // A scalar pushed onto an `[Any]` by `append` must be tagged like a literal
    // element. A large odd int left bare is bit-identical to a tagged string
    // pointer, so reading it back through an `Any` add once dereferenced garbage
    // and crashed; the element must survive the round trip intact.
    let mut cg = compile(
        "fn f() -> i64:\n    let mut xs: [Any] = []\n    xs.append(200000001)\n    let mut t: Any = 0\n    t = t + xs[0]\n    return int(t)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 200000001);
}

#[test]
fn regression_membership_in_any_list_matches_scalar() {
    // The needle of an `in` test against an `[Any]` is tagged the same way the
    // stored elements are, so equal inline scalars share one word and compare
    // equal even past the string-pointer threshold.
    let mut cg = compile(
        "fn f() -> i64:\n    let mut xs: [Any] = []\n    xs.append(100000003)\n    if 100000003 in xs:\n        return 1\n    return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn regression_tuple_constant_index_scalarized() {
    // A non-escaping tuple read by constant index never reaches the heap; the
    // scalar-replacement must preserve every element.
    let mut cg =
        compile("fn f() -> i64:\n    let t = (10, 20, 30)\n    return t[0] + t[1] + t[2]\n");
    assert_eq!(call_i64(&mut cg, "f"), 60);
}

#[test]
fn regression_tuple_destructure_scalarized() {
    // Multi-assignment from a non-escaping tuple lowers to per-element constant
    // index reads, so the aggregate is replaced by scalars yet still adds up.
    let mut cg = compile(
        "fn f() -> i64:\n    let mut a = 0\n    let mut b = 0\n    let mut c = 0\n    a, b, c = (10, 12, 20)\n    return a + b + c\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn regression_tuple_returned_survives_escape_guard() {
    // The tuple escapes through the return slot, so it must stay allocated and
    // come back intact at the call site.
    let mut cg = compile(
        "fn make() -> (i64, i64):\n    let t = (40, 2)\n    return t\n\nfn f() -> i64:\n    let p = make()\n    return p[0] + p[1]\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn regression_tuple_indexed_then_returned() {
    // Reading a field locally and also returning the tuple: the local read may
    // be served from a scalar, but the returned aggregate must still be whole.
    let mut cg = compile(
        "fn make() -> (i64, i64, i64):\n    let t = (10, 20, 30)\n    let _z = t[0]\n    return t\n\nfn f() -> i64:\n    let p = make()\n    return p[0] + p[1] + p[2]\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 60);
}

#[test]
fn regression_tuple_rebuilt_in_loop() {
    // A fresh tuple each iteration is the pattern stack promotion targets: the
    // running pair must advance correctly with no per-iteration allocation.
    let mut cg = compile(
        "fn f() -> i64:\n    let mut a = 0\n    let mut b = 1\n    let mut i = 0\n    while i < 5:\n        let t = (b, a + b)\n        a = t[0]\n        b = t[1]\n        i = i + 1\n    return b\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 8);
}

#[test]
fn regression_bce_const_index_value() {
    // A constant index into a fixed-length list elides its bounds check; the
    // value read must still be the right element.
    let mut cg =
        compile("fn f() -> i64:\n    let xs = [10, 20, 30, 40]\n    return xs[0] + xs[3]\n");
    assert_eq!(call_i64(&mut cg, "f"), 50);
}

#[test]
fn regression_bce_len_bounded_loop_sum() {
    // `while i < len(xs)` indexed by the induction variable: every access is
    // unchecked, and the sum must match.
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [1, 2, 3, 4, 5]\n    let mut acc = 0\n    let mut i = 0\n    while i < len(xs):\n        acc = acc + xs[i]\n        i = i + 1\n    return acc\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 15);
}

#[test]
fn regression_bce_len_bounded_loop_write() {
    // An indexed write inside a len-bounded loop is unchecked yet must store
    // into the right slot; reading back confirms the element-preserving path.
    let mut cg = compile(
        "fn f() -> i64:\n    let mut xs = [1, 2, 3, 4]\n    let mut i = 0\n    while i < len(xs):\n        xs[i] = xs[i] * 10\n        i = i + 1\n    return xs[0] + xs[1] + xs[2] + xs[3]\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 100);
}

#[test]
fn regression_bce_repeated_dynamic_index() {
    // Two reads of the same dynamic index in a block: the second is redundant
    // and must read the same value as the first.
    let mut cg =
        compile("fn f(i: i64) -> i64:\n    let xs = [7, 8, 9]\n    return xs[i] + xs[i]\n");
    assert_eq!(call_i64_1(&mut cg, "f", 1), 16);
}

#[test]
fn regression_closure_captures_scalar() {
    let mut cg = compile(
        "fn f() -> i64:\n    let n = 10\n    fn add(x: i64) -> i64:\n        return x + n\n    return add(5)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 15);
}

#[test]
fn regression_closure_multi_capture_and_reuse() {
    // Two captures, called twice, and the captured locals stay usable after.
    let mut cg = compile(
        "fn f() -> i64:\n    let a = 10\n    let b = 20\n    fn g(x: i64) -> i64:\n        return x + a + b\n    return g(1) + g(2) + a + b\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 31 + 32 + 30);
}

#[test]
fn regression_closure_captures_heap_read() {
    let mut cg = compile(
        "fn f() -> i64:\n    let xs = [1, 2, 3]\n    fn total() -> i64:\n        return xs[0] + xs[1] + xs[2]\n    return total() + xs[0]\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 7);
}

#[test]
fn regression_closure_heap_mutation_through_alias() {
    // Captured heap value is aliased: mutation is visible outside, no double-free.
    let mut cg = compile(
        "fn f() -> i64:\n    let mut xs = [1, 2, 3]\n    fn push():\n        xs.append(99)\n    push()\n    return len(xs) + xs[3]\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 4 + 99);
}

#[test]
fn regression_closure_transitive_capture() {
    // Middle fn captures `n` as a param, making it visible to the grandchild.
    let mut cg = compile(
        "fn f() -> i64:\n    let n = 5\n    fn outer(x: i64) -> i64:\n        fn inner(y: i64) -> i64:\n            return y + n\n        return inner(x) + n\n    return outer(1)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 11);
}

#[test]
fn regression_pyobject_none_compare_compiles() {
    // `pyobj != None` boxes the compared 0 in codegen; the conversion helper
    // must be imported or codegen panics on a missing runtime fn.
    let _cg =
        compile("struct B:\n    v: PyObject\nfn check(b: &B) -> bool:\n    return b.v != None\n");
}

#[test]
fn regression_pyobject_field_into_aggregate_compiles() {
    // A borrowed PyObject field placed in a list/tuple/set needs an owned ref
    // (`__olive_py_copy_ref`), else container drop frees it and the field dangles.
    let _cg = compile(
        "struct M:\n    a: PyObject\n    b: PyObject\nimpl M:\n    fn build(self):\n        let xs = [self.a, self.b]\n",
    );
}

#[test]
fn regression_pyobject_field_into_setindex_compiles() {
    // SetIndex of a borrowed PyObject field needs an owned ref too, else the
    // list decrefs it on drop.
    let _cg = compile(
        "struct M:\n    a: PyObject\nimpl M:\n    fn build(self, xs: &mut [PyObject]):\n        xs[0] = self.a\n",
    );
}

#[test]
fn regression_async_block_in_loop_compiles() {
    // Async state machine zero-inits locals; a sub-i64 (bool) local must be
    // zeroed with its own cranelift type or the SSA builder rejects the mismatch.
    let _cg = compile(
        "async fn handle():\n    print(1)\nasync fn run():\n    let mut i = 0\n    while i < 3:\n        i = i + 1\n        async:\n            await handle()\nfn main():\n    async:\n        await run()\n",
    );
}

#[test]
fn regression_default_param_omitted() {
    let mut cg = compile(
        "fn add(x: i64, y: i64 = 5) -> i64:\n    return x + y\nfn f() -> i64:\n    return add(1)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 6);
}

#[test]
fn regression_default_param_supplied() {
    let mut cg = compile(
        "fn add(x: i64, y: i64 = 5) -> i64:\n    return x + y\nfn f() -> i64:\n    return add(1, 2)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 3);
}

#[test]
fn regression_method_default_param_omitted() {
    let mut cg = compile(
        "struct C:\n    v: i64\nimpl C:\n    fn add(self, x: i64, y: i64 = 10) -> i64:\n        return self.v + x + y\nfn f() -> i64:\n    let c = C(1)\n    return c.add(2)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 13);
}

#[test]
fn regression_generic_method_minimal_opt() {
    // Monomorphized method receiver needs its field layout registered or field
    // access derefs a raw struct pointer. Full opt scalarizes it away; lean only.
    let mut cg = compile_minimal(
        "struct Box[T]:\n    v: T\nimpl[T] Box[T]:\n    fn get(self) -> T:\n        return self.v\nfn f() -> i64:\n    let b = Box(42)\n    return b.get()\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn regression_generic_struct_field_minimal_opt() {
    let mut cg = compile_minimal(
        "struct Box[T]:\n    v: T\nfn f() -> i64:\n    let b = Box(7)\n    return b.v\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 7);
}

#[test]
fn regression_generic_method_default_minimal_opt() {
    let mut cg = compile_minimal(
        "struct Box[T]:\n    v: T\nimpl[T] Box[T]:\n    fn show(self, p: i64 = 9) -> i64:\n        return p\nfn f() -> i64:\n    let b = Box(5)\n    return b.show()\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 9);
}

#[test]
fn regression_tail_self_recursion_terminates() {
    // Self tail call becomes a loop; back-edge must target a header, not the
    // sealed entry, or SSA construction panics.
    let mut cg = compile(
        "fn down(n: i64, base: i64) -> i64:\n    if n <= 0:\n        return base\n    return down(n - 1, base)\nfn f() -> i64:\n    return down(100000, 7)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 7);
}

#[test]
fn regression_tail_recursion_accumulator() {
    // Args staged through temporaries so each sees its pre-update param value.
    let mut cg = compile(
        "fn sum(n: i64, acc: i64) -> i64:\n    if n <= 0:\n        return acc\n    return sum(n - 1, acc + n)\nfn f() -> i64:\n    return sum(5, 0)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 15);
}

#[test]
fn regression_closure_tail_recursion() {
    let mut cg = compile(
        "fn f() -> i64:\n    let base = 100\n    fn down(n: i64) -> i64:\n        if n <= 0:\n            return base\n        return down(n - 1)\n    return down(50000)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 100);
}

#[test]
fn regression_closure_recursion() {
    // Non-tail recursion (keeps the tail-call pass out): a capturing closure
    // that calls itself supplies its captures each frame.
    let mut cg = compile(
        "fn f() -> i64:\n    let base = 100\n    fn sum(n: i64) -> i64:\n        if n <= 0:\n            return base\n        return n + sum(n - 1)\n    return sum(3)\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 100 + 3 + 2 + 1);
}

#[test]
fn regression_nested_fn_name_no_collision() {
    // A same-named helper in two functions must stay distinct after lifting.
    let mut cg = compile(
        "fn a() -> i64:\n    fn helper() -> i64:\n        return 1\n    return helper()\nfn b() -> i64:\n    fn helper() -> i64:\n        return 2\n    return helper()\nfn f() -> i64:\n    return a() * 10 + b()\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 12);
}

#[test]
fn regression_closure_capture_in_loop() {
    let mut cg = compile(
        "fn f() -> i64:\n    let step = 3\n    let mut total = 0\n    let mut i = 0\n    while i < 4:\n        fn bump(v: i64) -> i64:\n            return v + step\n        total = bump(total)\n        i = i + 1\n    return total\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 12);
}

#[test]
fn regression_py_coerce_to_native_let_compiles() {
    // A PyObject read into a native-annotated binding unboxes via the runtime
    // converter rather than parking the raw pointer in a typed slot.
    let _cg = compile("fn f(p: PyObject) -> i64:\n    let n: i64 = p\n    return n\n");
    let _cg = compile("fn g(p: PyObject) -> f64:\n    let x: f64 = p\n    return x\n");
}

#[test]
fn regression_py_coerce_to_str_compiles() {
    // PyObject -> str must route through __olive_py_to_str; the str cast path was
    // previously unwired and silently produced an empty value.
    let _cg = compile("fn f(p: PyObject) -> str:\n    let s: str = p\n    return s\n");
}

#[test]
fn regression_py_coerce_return_and_arg_compiles() {
    let _cg = compile(
        "fn takes(x: f64) -> f64:\n    return x + x\nfn f(p: PyObject) -> f64:\n    return takes(p)\n",
    );
}

#[test]
fn regression_py_coerce_collection_elements_compiles() {
    // A PyObject stored into a concretely-typed collection slot unboxes instead
    // of leaving a raw pointer that later reads as garbage.
    let _cg = compile("fn f(p: PyObject) -> [f64]:\n    return [p]\n");
    let _cg = compile("fn g(p: PyObject) -> {str: i64}:\n    return {\"a\": p}\n");
}

#[test]
fn regression_py_coerce_struct_field_compiles() {
    // Constructing a native-field struct from a PyObject must unbox per field;
    // otherwise a f64 field read off a pointer-sized slot fails register
    // allocation with a type mismatch.
    let _cg = compile(
        "struct Pt:\n    x: f64\n    name: str\nfn f(p: PyObject) -> Pt:\n    return Pt(p, p)\n",
    );
}

#[test]
fn regression_subscript_any_boxing() {
    let mut cg = compile(
        "fn f() -> Any:\n    let mut xs: [Any] = list_new(2)\n    xs[0] = 200000001\n    return xs[0]\n",
    );
    let val = call_i64(&mut cg, "f");
    // If it is boxed correctly, the low bits will have TAG_INT (2).
    // If it is raw, it will be 200000001.
    assert_eq!(val, (200000001 << 3) | 2);
}

#[test]
fn regression_subscript_any_boxing_inferred() {
    let mut cg = compile(
        "fn f() -> Any:\n    let mut xs = list_new(2)\n    xs[0] = 200000001\n    let mut ys: [Any] = xs\n    return ys[0]\n",
    );
    let val = call_i64(&mut cg, "f");
    assert_eq!(val, (200000001 << 3) | 2);
}

#[test]
fn regression_subscript_any_boxing_return() {
    let mut cg = compile(
        "fn f() -> [Any]:\n    let mut xs = list_new(2)\n    xs[0] = 200000001\n    return xs\nfn g() -> Any:\n    let xs = f()\n    return xs[0]\n",
    );
    let val = call_i64(&mut cg, "g");
    assert_eq!(val, (200000001 << 3) | 2);
}

#[test]
fn regression_list_new_kind_any_list() {
    // list_new must produce KIND_ANY_LIST (15) so subscript reads round-trip
    // boxed Any values correctly through function boundaries.
    let mut cg = compile(
        "fn make() -> [Any]:\n    let mut xs = list_new(3)\n    xs[0] = 42\n    xs[1] = 99\n    return xs\nfn f() -> Any:\n    let xs = make()\n    return xs[1]\n",
    );
    let val = call_i64(&mut cg, "f");
    assert_eq!(val, (99 << 3) | 2);
}
