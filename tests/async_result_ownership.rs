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
