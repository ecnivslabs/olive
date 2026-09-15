#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn bytes_for_loop_yields_each_byte() {
    assert_both(
        r#"fn main():
    let bytes = bytes_new(0)
    bytes_push(bytes, 65)
    bytes_push(bytes, 66)
    bytes_push(bytes, 67)
    for value in bytes:
        print(value)
"#,
        "65\n66\n67\n",
    );
}
