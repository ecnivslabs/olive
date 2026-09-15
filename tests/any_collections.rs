#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn native_lists_erased_into_any_keep_their_scalar_values() {
    assert_both(
        r#"fn show(value: Any):
    print(value)

fn main():
    let ints = [42, -7, 0, 2, 3, 4]
    let floats = [1.25, -0.0, 2.5]
    let bools = [True, False]
    show(ints)
    show(floats)
    show(bools)
    print(ints)
"#,
        "[42, -7, 0, 2, 3, 4]\n[1.25, -0.0, 2.5]\n[True, False]\n[42, -7, 0, 2, 3, 4]\n",
    );
}

#[test]
fn erased_native_lists_support_dynamic_indexing() {
    assert_both(
        r#"fn first(value: Any) -> Any:
    return value[0]

fn main():
    let xs = [42]
    print(first(xs))
    let ys = [[1.25, 2.5]]
    print(first(first(ys)))
"#,
        "42\n1.25\n",
    );
}

#[test]
fn erased_nested_lists_survive_an_async_handoff() {
    assert_both(
        r#"async fn forward(value: Any) -> Any:
    return value

async fn launch() -> Future[Any]:
    let rows = [[42, -7], [2, 3]]
    return forward(rows)

fn main():
    print(await (await launch()))
"#,
        "[[42, -7], [2, 3]]\n",
    );
}

#[test]
fn scalar_any_indexing_reports_a_fault_instead_of_dereferencing_the_scalar() {
    program::assert_both_with(
        r#"fn first(value: Any) -> Any:
    return value[0]

fn main():
    print(first(42))
"#,
        |status, _, stderr| {
            assert_eq!(status.code(), Some(1), "{status}: {stderr}");
            assert!(stderr.contains("does not support indexing"), "{stderr}");
        },
    );
}
