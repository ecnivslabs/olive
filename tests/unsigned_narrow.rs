#[path = "support/program.rs"]
mod program;
use program::assert_both;

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
