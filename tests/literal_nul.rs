#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn source_literal_preserves_embedded_nul() {
    assert_both(
        r#"fn main():
    let s = "a\0b"
    assert len(s) == 3
    assert s[1] == "\0"
    print(len(s))
"#,
        "3\n",
    );
}
