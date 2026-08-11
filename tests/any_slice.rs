//! Slicing through `Any`-typed values and enum payload reads.
//!
//! Slicing a statically-`Any` object used to emit the Python slicer
//! unconditionally, segfaulting on native values (`a[0:5]` with
//! `a: Any = "hello"` died in `PyObject_GetItem`). `Any` slices now
//! dispatch at runtime by the value's own representation. Enum payload
//! reads through a single-variant enum type precisely, so chained member
//! access and annotation-guided reads stay on typed paths.

#[path = "support/program.rs"]
mod program;
use program::{assert_both, assert_both_with};

#[test]
fn any_held_string_slices() {
    assert_both(
        r#"fn main():
    let a: Any = "hello-world"
    print(a[0:5])
"#,
        "\"hello\"\n",
    );
}

#[test]
fn any_held_list_slices() {
    assert_both(
        r#"fn main():
    let l: Any = [10, 20, 30, 40]
    print(l[1:3])
"#,
        "[20, 30]\n",
    );
}

#[test]
fn enum_payload_slices() {
    assert_both(
        r#"enum E:
    V(str)

fn main():
    let e = V("hello-world")
    let s: str = e[0]
    print(s[0:5])
"#,
        "hello\n",
    );
}

#[test]
fn single_variant_struct_payload_chains() {
    assert_both(
        r#"struct P:
    s: str

enum F:
    Hold(P)

fn main():
    let f = Hold(P("boxed"))
    print(f[0].s)
    let p: P = f[0]
    print(p.s)
"#,
        "boxed\nboxed\n",
    );
}

#[test]
fn slicing_an_int_faults_cleanly() {
    assert_both_with(
        r#"fn main():
    let n: Any = 42
    print(n[0:2])
"#,
        |status, _, stderr| {
            assert_eq!(status.code(), Some(1), "{status}: {stderr}");
            assert!(stderr.contains("does not support slicing"), "{stderr}");
        },
    );
}
