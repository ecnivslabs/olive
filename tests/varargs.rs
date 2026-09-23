#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn olive_variadic_functions_keep_positional_and_keyword_packing() {
    assert_both(
        r#"fn f(a: int, *args: int) -> int:
    print(args)
    return a

fn log(message: str, **metadata: str) -> str:
    print(metadata)
    return message

fn main():
    f(1)
    f(1, 2, 3)
    print(log("x", port="8080"))
    print(1, "a", 2.5, [1, 2], None)
"#,
        "[]\n[2, 3]\n{\"port\": \"8080\"}\nx\n1 a 2.5 [1, 2] None\n",
    );
}

#[test]
fn olive_variadic_methods_pack_arguments() {
    assert_both(
        r#"struct S:
    fn f(self, *args: int):
        print(args)

fn main():
    S().f(1, 2)
"#,
        "[1, 2]\n",
    );
}
