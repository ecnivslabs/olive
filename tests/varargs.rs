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

#[test]
fn higher_order_fixed_container_parameters_are_not_variadic() {
    assert_both(
        r#"fn list_value(xs: [int]) -> int:
    return xs[0]

fn dict_value(d: {str: int}) -> int:
    return d["key"]

fn apply_list(f: fn([int]) -> int, xs: [int]) -> int:
    return f(xs)

fn apply_dict(f: fn({str: int}) -> int, d: {str: int}) -> int:
    return f(d)

fn main():
    print(apply_list(list_value, [7]))
    print(apply_dict(dict_value, {"key": 9}))
"#,
        "7\n9\n",
    );
}
