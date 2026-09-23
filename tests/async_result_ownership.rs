#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn async_block_payload_survives_source_drop() {
    assert_both(
        r#"fn main():
    let f = async:
        [str(12345)]
    print(await f)
"#,
        "[\"12345\"]\n",
    );
}

#[test]
fn completed_scalar_result_is_not_treated_as_a_child_pointer() {
    assert_both(
        r#"async fn immediate() -> int:
    return 1

async fn ready() -> int:
    return await immediate()

fn main():
    print(await ready())
"#,
        "1\n",
    );
}

#[test]
fn same_child_future_can_be_awaited_repeatedly() {
    assert_both(
        r#"async fn work() -> int:
    return 7

async fn twice() -> int:
    let f = work()
    let first = await f
    let second = await f
    return first + second

fn main():
    print(await twice())
"#,
        "14\n",
    );
}

#[test]
fn repeated_future_results_are_independent() {
    assert_both(
        r#"import aio

async fn work() -> [str]:
    return [str(12345)]

fn main():
    let f = work()
    let results = await aio.gather([f, f])
    results[0][0] = "changed"
    print(results[1][0])
"#,
        "12345\n",
    );
}
