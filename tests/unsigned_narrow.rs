#[path = "support/program.rs"]
mod program;
use program::{assert_both, assert_fault_both};

#[test]
fn narrow_unsigned_division_and_remainder_use_unsigned_codegen() {
    assert_both(
        r#"fn div(x: u32) -> u32:
    return x / 8

fn rem(x: u32) -> u32:
    return x % 8

fn div8(x: u8) -> u8:
    return x / 8

fn rem8(x: u8) -> u8:
    return x % 8

fn div16(x: u16) -> u16:
    return x / 8

fn rem16(x: u16) -> u16:
    return x % 8

fn main():
    print(div(4294967295))
    print(rem(4294967295))
    print(div8(255))
    print(rem8(255))
    print(div16(65535))
    print(rem16(65535))
"#,
        "536870911\n7\n31\n7\n8191\n7\n",
    );
}

#[test]
fn narrow_signed_operations_keep_their_width() {
    assert_both(
        r#"fn add(a: i8, b: i8) -> i8:
    return a + b

fn div(a: i8, b: i8) -> i8:
    return a / b

fn main():
    print(add(12 as i8, 20 as i8))
    print(div(12 as i8, 3 as i8))
"#,
        "32\n4\n",
    );
}

#[test]
fn narrow_signed_division_overflow_is_checked() {
    assert_fault_both(
        r#"fn div(a: i8, b: i8) -> i8:
    return a / b

fn main():
    print(div(-128 as i8, -1 as i8))
"#,
        "[E0713]",
        "integer overflow",
    );
}

#[test]
fn float_and_unsigned_casts_use_numeric_signedness() {
    assert_both(
        r#"fn neg(x: f32) -> f32:
    return -x

fn main():
    let x: f32 = 1.5
    print(neg(x))
    let y: u64 = 9223372036854775807 as u64
    print(y as f64)
    let z: u32 = 4000000000 as u32
    print(z as f64)
    let f: f64 = 4000000000.5
    print(f as u32)
    let n: u8 = 255 as u8
    print(n < 128)
"#,
        "-1.5\n9223372036854776000.0\n4000000000.0\n4000000000\nFalse\n",
    );
}

#[test]
fn unsigned_index_does_not_wrap_into_a_valid_position() {
    assert_fault_both(
        r#"fn main():
    let xs = [7]
    let zero: u64 = 0
    let index: u64 = ~zero
    print(xs[index])
"#,
        "[E0701]",
        "index",
    );
}
