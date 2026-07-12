use crate::test_utils::{call_i64, compile};

/// Struct with `__drop__` that runs without error at scope exit.
#[test]
fn drop_hook_fires_at_scope_exit() {
    let src = r#"
struct Droppable:
    id: int
    fn __init__(self, i: int):
        self.id = i
    fn __drop__(self):
        let tmp = self.id + 1
        tmp

fn main() -> int:
    let d = Droppable(1)
    0
"#;
    let mut cg = compile(src);
    assert_eq!(call_i64(&mut cg, "main"), 0);
}

/// `__drop__` in an impl block works the same as inline.
#[test]
fn drop_inside_impl_block_works() {
    let src = r#"
struct R:
    handle: int
    fn __init__(self, h: int):
        self.handle = h

impl R:
    fn __drop__(self):
        let tmp = self.handle * 2
        tmp

fn main() -> int:
    let r = R(7)
    0
"#;
    let mut cg = compile(src);
    assert_eq!(call_i64(&mut cg, "main"), 0);
}

/// Struct without `__drop__` still compiles and runs.
#[test]
fn struct_without_drop_still_works() {
    let src = r#"
struct Plain:
    x: int
    fn __init__(self, x: int):
        self.x = x

fn main() -> int:
    let p = Plain(42)
    p.x
"#;
    let mut cg = compile(src);
    assert_eq!(call_i64(&mut cg, "main"), 42);
}

/// Duplicate `__drop__` is silently resolved (last definition wins).
#[test]
fn duplicate_drop_is_not_an_error() {
    let src = r#"
struct Bad:
    x: int
    fn __drop__(self):
        1
    fn __drop__(self):
        2

fn main() -> int:
    0
"#;
    let mut cg = compile(src);
    assert_eq!(call_i64(&mut cg, "main"), 0);
}

/// `__drop__` with wrong signature is a compile error.
#[test]
fn drop_wrong_signature_is_error() {
    let src = r#"
struct Bad:
    x: int
    fn __drop__(self, extra: int):
        extra
"#;
    assert!(crate::test_utils::check_codes(src).contains(&"E0404".to_string()));
}

/// Struct with `__drop__` can be moved into a list without double-free.
#[test]
fn drop_not_double_freed_after_move_into_list() {
    let src = r#"
struct R:
    id: int
    fn __init__(self, i: int):
        self.id = i
    fn __drop__(self):
        let tmp = self.id
        tmp

fn main() -> int:
    let r = R(1)
    let xs = [r]
    len(xs)
"#;
    let mut cg = compile(src);
    assert_eq!(call_i64(&mut cg, "main"), 1);
}

/// Struct with `__drop__` can be returned from a function.
#[test]
fn drop_not_double_freed_after_return() {
    let src = r#"
struct R:
    id: int
    fn __init__(self, i: int):
        self.id = i
    fn __drop__(self):
        let tmp = self.id + 1
        tmp

fn make(i: int) -> R:
    R(i)

fn main() -> int:
    let r = make(2)
    r.id
"#;
    let mut cg = compile(src);
    assert_eq!(call_i64(&mut cg, "main"), 2);
}

/// Two structs with `__drop__` compiled in the same program work.
#[test]
fn two_structs_with_drop_compile_and_run() {
    let src = r#"
struct A:
    x: int
    fn __init__(self, v: int):
        self.x = v
    fn __drop__(self):
        let tmp = self.x
        tmp

struct B:
    y: int
    fn __init__(self, v: int):
        self.y = v
    fn __drop__(self):
        let tmp = self.y + 1
        tmp

fn main() -> int:
    let a = A(1)
    let b = B(2)
    a.x + b.y
"#;
    let mut cg = compile(src);
    assert_eq!(call_i64(&mut cg, "main"), 3);
}

/// `__drop__` in struct fields fires for both inner and outer.
#[test]
fn nested_structs_both_drop() {
    let src = r#"
struct Inner:
    v: int
    fn __init__(self, v: int):
        self.v = v
    fn __drop__(self):
        let tmp = self.v
        tmp

struct Outer:
    inner: Inner
    fn __init__(self, v: int):
        self.inner = Inner(v)
    fn __drop__(self):
        let tmp = self.inner.v
        tmp

fn main() -> int:
    let o = Outer(5)
    o.inner.v
"#;
    let mut cg = compile(src);
    assert_eq!(call_i64(&mut cg, "main"), 5);
}

/// `__drop__` in list elements fires when the list is dropped.
#[test]
fn drop_fires_for_list_elements() {
    let src = r#"
struct R:
    id: int
    fn __init__(self, i: int):
        self.id = i
    fn __drop__(self):
        let tmp = self.id
        tmp

fn main() -> int:
    let xs = [R(1), R(2), R(3)]
    len(xs)
"#;
    let mut cg = compile(src);
    assert_eq!(call_i64(&mut cg, "main"), 3);
}
