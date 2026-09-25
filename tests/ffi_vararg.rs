#[cfg(target_os = "linux")]
#[path = "support/program.rs"]
mod program;

#[cfg(target_os = "linux")]
use program::{assert_both, assert_compile_fails};

#[cfg(target_os = "linux")]
#[test]
fn c_variadic_call_requires_fixed_arguments() {
    assert_compile_fails(
        r#"import "/usr/lib/libc.so.6" as c:
    fn printf(fmt: str, ...) -> int

fn main():
    unsafe:
        c.printf()
"#,
        "variadic call expects at least 1 arguments, got 0",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn c_variadic_call_checks_fixed_argument_types() {
    assert_compile_fails(
        r#"import "/usr/lib/libc.so.6" as c:
    fn printf(fmt: str, ...) -> int

fn main():
    unsafe:
        c.printf(1)
"#,
        "mismatched types",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn c_variadic_call_rejects_managed_values() {
    assert_compile_fails(
        r#"import "/usr/lib/libc.so.6" as c:
    fn printf(fmt: str, ...) -> int

fn main():
    let values = [1, 2]
    unsafe:
        c.printf("%p\n", values)
"#,
        "which cannot cross the C variadic boundary",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn c_variadic_call_rejects_any_values() {
    assert_compile_fails(
        r#"import "/usr/lib/libc.so.6" as c:
    fn printf(fmt: str, ...) -> int

fn main():
    let value: Any = 7
    unsafe:
        c.printf("%d\n", value)
"#,
        "which cannot cross the C variadic boundary",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn c_variadic_declaration_rejects_aggregate_parameters() {
    assert_compile_fails(
        r#"import "/usr/lib/libc.so.6" as c:
    struct Pair:
        a: i64
        b: i64
    fn take_pair(pair: Pair, ...) -> int

fn main():
    pass
"#,
        "has no verified C variadic ABI",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn c_variadic_declaration_rejects_aggregate_returns() {
    assert_compile_fails(
        r#"import "/usr/lib/libc.so.6" as c:
    struct Pair:
        a: i64
        b: i64
    fn make_pair(...) -> Pair

fn main():
    pass
"#,
        "has no verified variadic return ABI",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn fixed_ffi_string_arguments_strip_all_olive_tags() {
    assert_both(
        r#"import "/usr/lib/libc.so.6" as c:
    fn strlen(s: str) -> int

fn forward(s: Any) -> int:
    unsafe:
        return c.strlen(s)

fn main():
    let value = "".join(["ab", "cd"])
    print(forward(value))
"#,
        "4\n",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn variadic_ffi_string_arguments_strip_all_olive_tags() {
    assert_both(
        r#"import "/usr/lib/libc.so.6" as c:
    fn printf(fmt: str, ...) -> int

fn main():
    let value = "".join(["ab", "cd"])
    unsafe:
        c.printf("%s\n", value)
"#,
        "abcd\n",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn fixed_ffi_raw_pointer_preserves_low_address_bits() {
    assert_both(
        r#"import "/usr/lib/libc.so.6" as c:
    fn strchr(s: str, ch: int) -> *void
    fn memcmp(a: *void, b: str, n: int) -> int

fn main():
    unsafe:
        let p = c.strchr("abcdef", 99)
        print(c.memcmp(p, "cdef", 4))
"#,
        "0\n",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn variadic_ffi_raw_pointer_preserves_low_address_bits() {
    assert_both(
        r#"import "/usr/lib/libc.so.6" as c:
    fn strchr(s: str, ch: int) -> *void
    fn printf(fmt: str, ...) -> int

fn main():
    unsafe:
        let p = c.strchr("abcdef", 99)
        c.printf("%s\n", p)
"#,
        "cdef\n",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn c_aggregate_rejects_qualified_nested_fields() {
    assert_compile_fails(
        r#"import "/usr/lib/libc.so.6" as c:
    struct Inner:
        x: int
    struct Outer:
        inner: c.Inner
        y: int
    fn outer_sum(o: Outer) -> int

fn main():
    print("unreached")
"#,
        "has an unsupported type",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn c_aggregate_rejects_fixed_array_fields() {
    assert_compile_fails(
        r#"import "/usr/lib/libc.so.6" as c:
    struct Arr:
        xs: [int; 3]
    fn arr_sum(a: Arr) -> int

fn main():
    print("unreached")
"#,
        "has an unsupported type",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn c_void_return_is_accepted() {
    assert_both(
        r#"import "/usr/lib/libc.so.6" as c:
    fn malloc(size: int) -> *void
    fn free(p: *void)

fn main():
    unsafe:
        let p = c.malloc(1)
        c.free(p)
"#,
        "",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn c_varargs_promote_narrow_int_and_f32() {
    assert_both(
        r#"import "/usr/lib/libc.so.6" as c:
    @cdecl
    fn printf(fmt: str, ...) -> int

fn main():
    let n: i8 = 7
    let x: f32 = 1.5
    unsafe:
        c.printf("%d %f\n", n, x)
"#,
        "7 1.500000\n",
    );
}
