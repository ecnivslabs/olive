#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn interned_indexed_char_is_reflected_as_string() {
    assert_both(
        r#"import reflect

fn main():
    let c = "abc"[0]
    print(reflect.is_str(c))
    print(reflect.typeof(c))
    print(reflect.is_str(200001))
    print(reflect.typeof(200001))
    print(reflect.is_null(None))
"#,
        "True\nstr\nFalse\nint\nTrue\n",
    );
}

#[test]
fn string_indexes_and_global_slice_use_scalar_positions() {
    assert_both(
        r#"fn main():
    let s = "é😀"
    print(s[0])
    print(s[1])
    print(s[-1])
    print(s[-2])
    print(s[0:2])
    print(slice(s, 0, 2))
"#,
        "é\n😀\n😀\né\né😀\né😀\n",
    );
}
