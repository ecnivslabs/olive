#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn native_serializers_stringify_non_string_dict_keys_without_wild_reads() {
    assert_both(
        r#"import yaml
fn main():
    print(yaml.stringify({1: "a"}))
    print(yaml.toml_stringify({1: "a"}))
"#,
        "'1': a\n\n1 = \"a\"\n\n",
    );
}
