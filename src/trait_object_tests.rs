//! Regression tests for three dynamic-dispatch bugs found while auditing the
//! `polymorphism` benchmark, discovered independently during that work.
//!
//! 1. Checker: a trait-object receiver's method call fell through to
//!    `fresh_var()` (no `TraitObject` arm in `ExprKind::Attr` resolution),
//!    finalizing to `Any`. Harmless for a `Null`-returning method (the docs'
//!    own `Drawable.draw()` example), silently wrong for anything else.
//! 2. Codegen: the raw-function-pointer indirect-call branch in
//!    `translate_call` (vtable dispatch's actual call site) hardcoded an
//!    `i64` return signature. A trait method returning `float` puts its
//!    result in XMM0 under System V; the caller read RAX instead.
//! 3. Ownership: `Drop` on a `TraitObject` local only knew the fat
//!    pointer's own two words, never the concrete struct's field layout
//!    underneath, so it leaked the boxed struct on every drop (confirmed
//!    against the real benchmark: 50M iterations went from an OOM kill to
//!    a clean exit). Fixed by synthesizing a per-struct drop shim
//!    (`build_trait_drop_shim`) whose address is the fat pointer's third
//!    word.
#[cfg(test)]
use crate::test_utils::{call_i64, call_i64_1, compile};

const SHAPES: &str = concat!(
    "trait Shape:\n",
    "    fn area(self) -> float:\n",
    "        return 0.0\n",
    "\n",
    "struct Circle:\n",
    "    radius: float\n",
    "\n",
    "struct Square:\n",
    "    side: float\n",
    "\n",
    "impl Shape for Circle:\n",
    "    fn area(self) -> float:\n",
    "        return 3.14159 * self.radius * self.radius\n",
    "\n",
    "impl Shape for Square:\n",
    "    fn area(self) -> float:\n",
    "        return self.side * self.side\n",
);

#[test]
fn trait_object_param_dispatches_to_correct_area() {
    let mut cg = compile(&format!(
        "{SHAPES}\nfn shape_area(s: Shape) -> float:\n    return s.area()\n\nfn f() -> int:\n    let a = shape_area(Circle(2.0))\n    if a > 12.56 and a < 12.57:\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn trait_object_list_iteration_dispatches_correctly() {
    let mut cg = compile(&format!(
        "{SHAPES}\nfn total(items: [Shape]) -> float:\n    let mut sum = 0.0\n    for item in items:\n        sum = sum + item.area()\n    return sum\n\nfn f() -> int:\n    let t = total([Circle(2.0), Square(3.0)])\n    if t > 21.56 and t < 21.57:\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn trait_object_method_call_types_from_trait_not_any() {
    let mut cg = compile(&format!(
        "{SHAPES}\nfn shape_area(s: Shape) -> float:\n    let a = s.area()\n    if a == 0.0:\n        return -1.0\n    return a\n\nfn f() -> int:\n    let a = shape_area(Square(4.0))\n    if a == 16.0:\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn trait_object_drop_does_not_corrupt_across_many_allocations() {
    // Not a leak detector (that's the benchmark's job), but repeated
    // create-coerce-drop cycles will corrupt the allocator's own metadata
    // fast if the drop shim frees the wrong bytes or double-frees.
    let mut cg = compile(&format!(
        "{SHAPES}\nfn shape_area(s: Shape) -> float:\n    return s.area()\n\nfn f() -> int:\n    let mut total = 0.0\n    let mut i = 0\n    while i < 20000:\n        total = total + shape_area(Circle(2.0))\n        i = i + 1\n    let expected = 12.56636 * 20000.0\n    let diff = total - expected\n    if diff > -1.0 and diff < 1.0:\n        return 1\n    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn branchy_coercion_devirtualizes_to_correct_method() {
    // Two coercion sites feeding one inlined dynamic call: each branch must
    // resolve to its own concrete method, whether devirtualized or not.
    let mut cg = compile(&format!(
        "{SHAPES}\nfn shape_area(s: Shape) -> float:\n    return s.area()\n\nfn f(kind: i64) -> i64:\n    let mut total = 0.0\n    let mut i = 0\n    while i < 100:\n        if kind == 0:\n            total = total + shape_area(Circle(2.0))\n        else:\n            total = total + shape_area(Square(3.0))\n        i = i + 1\n    return int(total)\n",
    ));
    assert_eq!(call_i64_1(&mut cg, "f", 0), 1256);
    assert_eq!(call_i64_1(&mut cg, "f", 1), 900);
}

#[test]
fn escaping_trait_object_keeps_dynamic_dispatch() {
    // The record escapes into a list, so devirtualization must leave it
    // alone and the stored fat pointer must still dispatch and free.
    let mut cg = compile(&format!(
        "{SHAPES}\nfn f() -> i64:\n    let mut shapes: [Shape] = []\n    let mut i = 0\n    while i < 50:\n        shapes = shapes + [Circle(2.0)]\n        shapes = shapes + [Square(3.0)]\n        i = i + 1\n    let mut total = 0.0\n    for s in &shapes:\n        total = total + s.area()\n    return int(total)\n",
    ));
    // 50 circles of area 4*pi plus 50 squares of area 9, truncated.
    assert_eq!(call_i64(&mut cg, "f"), 1078);
}

#[test]
fn drop_hook_struct_keeps_dynamic_drop_path() {
    // A hook-bearing struct must not devirtualize past its shim: the hook
    // still runs on every trait-object drop.
    let src = concat!(
        "trait Noisy:\n",
        "    fn id(self) -> int:\n",
        "        return 0\n",
        "\n",
        "struct Tracker:\n",
        "    n: int\n",
        "    fn __drop__(self):\n",
        "        let t = self.n + 1\n",
        "        t\n",
        "\n",
        "impl Noisy for Tracker:\n",
        "    fn id(self) -> int:\n",
        "        return self.n\n",
        "\n",
        "fn read(x: Noisy) -> int:\n",
        "    return x.id()\n",
        "\n",
        "fn f() -> i64:\n",
        "    let mut sum = 0\n",
        "    let mut i = 0\n",
        "    while i < 1000:\n",
        "        sum = sum + read(Tracker(7))\n",
        "        i = i + 1\n",
        "    return sum\n",
    );
    let mut cg = compile(src);
    assert_eq!(call_i64(&mut cg, "f"), 7000);
}

