#[cfg(target_os = "linux")]
#[path = "support/program.rs"]
mod program;

#[cfg(target_os = "linux")]
use program::assert_both;

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
