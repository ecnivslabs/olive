#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn spawned_float_arguments_and_result() {
    assert_both(
        "async fn add(a: float, b: float) -> float:\n    return a + b\n\nfn main():\n    print(await add(1.25, 2.5))\n",
        "3.75\n",
    );
}

#[test]
fn spawned_many_mixed_arguments() {
    assert_both(
        "async fn sum(a: int, b: float, c: int, d: float, e: int, f: float, g: int, h: float, i: int, j: float, k: int, l: float) -> float:\n    return (a as float) + b + (c as float) + d + (e as float) + f + (g as float) + h + (i as float) + j + (k as float) + l\n\nfn main():\n    assert (await sum(1, 2.0, 3, 4.0, 5, 6.0, 7, 8.0, 9, 10.0, 11, 12.0)) == 78.0\n    print(\"ok\")\n",
        "ok\n",
    );
}

#[test]
fn state_machine_float_await_and_return() {
    assert_both(
        "async fn value() -> float:\n    return 2.5\n\nasync fn add(a: float) -> float:\n    let b = await value()\n    return a + b\n\nfn main():\n    print(await add(1.25))\n",
        "3.75\n",
    );
}

#[test]
fn narrow_scalar_await_and_return() {
    assert_both(
        "async fn value(a: i8) -> i8:\n    return a\n\nasync fn forward(a: i8) -> i8:\n    return await value(a)\n\nasync fn flag(a: bool) -> bool:\n    return a\n\nasync fn forward_flag(a: bool) -> bool:\n    return await flag(a)\n\nfn main():\n    assert (await forward(-7 as i8)) == (-7 as i8)\n    assert await forward_flag(True)\n    print(\"ok\")\n",
        "ok\n",
    );
}

#[test]
fn f32_await_and_return() {
    assert_both(
        "async fn value(a: f32) -> f32:\n    return a\n\nasync fn forward(a: f32) -> f32:\n    let b = await value(a)\n    return b\n\nfn main():\n    let a: f32 = 1.25\n    assert (await forward(a)) == a\n    print(\"ok\")\n",
        "ok\n",
    );
}

#[test]
fn negative_zero_is_a_ready_result() {
    assert_both(
        "async fn value(a: float) -> float:\n    return a\n\nasync fn forward(a: float) -> float:\n    return await value(a)\n\nfn main():\n    print(await forward(-0.0))\n",
        "-0.0\n",
    );
}

#[test]
fn minimum_integer_is_a_ready_result() {
    assert_both(
        "async fn value(a: int) -> int:\n    return a\n\nasync fn forward(a: int) -> int:\n    return await value(a)\n\nfn main():\n    print(await forward(-9223372036854775807 - 1))\n",
        "-9223372036854775808\n",
    );
}

#[test]
fn state_machine_struct_result_retains_its_fields() {
    assert_both(
        "struct Record:\n    first: int\n    second: int\n\nasync fn value() -> int:\n    return 1\n\nasync fn record() -> Record:\n    let first = await value()\n    return Record(first, 42)\n\nfn main():\n    let r = await record()\n    print(r.first)\n    print(r.second)\n",
        "1\n42\n",
    );
}

#[test]
fn spawned_arguments_survive_the_launching_task() {
    assert_both(
        r#"async fn consume(xs: [int], marker: int) -> int:
    while __olive_struct_gen_of(marker) != 0:
        __olive_time_sleep(0.001)
    return xs[0]

async fn launch() -> Future[int]:
    let marker: int = __olive_struct_alloc(0)
    let xs = [42]
    return consume(xs, marker)

fn main():
    let child = await launch()
    print(await child)
"#,
        "42\n",
    );
}

#[test]
fn suspended_arguments_survive_the_launching_task() {
    assert_both(
        r#"async fn ready() -> int:
    return 1

async fn consume(xs: [int]) -> int:
    assert (await ready()) == 1
    return xs[0]

async fn launch() -> Future[int]:
    let xs = [42]
    return consume(xs)

fn main():
    let child = await launch()
    print(await child)
"#,
        "42\n",
    );
}

#[test]
fn async_argument_copies_are_released() {
    assert_both(
        r#"async fn consume(xs: [int]) -> int:
    return __olive_unbox_int(xs)

async fn suspended(xs: [int]) -> int:
    assert (await consume([0])) != 0
    return __olive_unbox_int(xs)

fn main():
    let xs = [42]
    let first = await consume(xs)
    assert (__olive_struct_gen_of(first) & 1) == 0
    let second = await suspended(xs)
    assert (__olive_struct_gen_of(second) & 1) == 0
    assert xs[0] == 42
    print("ok")
"#,
        "ok\n",
    );
}

#[test]
fn async_arguments_can_escape_and_return_without_consuming_the_caller() {
    assert_both(
        r#"async fn wrap(xs: [int]) -> [[int]]:
    return [xs]

async fn forward(xs: [int]) -> [int]:
    let nested = await wrap(xs)
    assert nested[0][0] == 42
    return xs

fn main():
    let xs = [42]
    let ys = await forward(xs)
    assert ys[0] == 42
    assert xs[0] == 42
    let zs = await wrap(xs)
    assert zs[0][0] == 42
    assert xs[0] == 42
    print("ok")
"#,
        "ok\n",
    );
}

#[test]
fn async_nested_function_captures_survive_the_launcher() {
    assert_both(
        r#"async fn launch() -> Future[int]:
    let xs = [42]
    async fn consume() -> int:
        return xs[0]
    return consume()

fn main():
    print(await (await launch()))
"#,
        "42\n",
    );
}

#[test]
fn async_any_arguments_survive_the_launcher() {
    assert_both(
        r#"async fn ready() -> int:
    return 1

async fn consume(value: Any) -> Any:
    assert (await ready()) == 1
    return value

async fn launch() -> Future[Any]:
    let xs: [Any] = [42]
    return consume(xs)

fn main():
    print(await (await launch()))
"#,
        "[42]\n",
    );
}

#[test]
fn async_reassigned_parameters_release_the_initial_capture() {
    assert_both(
        r#"async fn ready() -> int:
    return 1

async fn consume(mut xs: [int]) -> int:
    let old: int = __olive_unbox_int(xs)
    let generation: int = __olive_struct_gen_of(old)
    xs = [7]
    assert __olive_struct_gen_stale(old, generation) != 0
    return xs[0]

async fn suspended(mut xs: [int]) -> int:
    let old: int = __olive_unbox_int(xs)
    let generation: int = __olive_struct_gen_of(old)
    assert (await ready()) == 1
    xs = [7]
    assert __olive_struct_gen_stale(old, generation) != 0
    return xs[0]

fn main():
    let xs = [42]
    assert (await consume(xs)) == 7
    assert (await suspended(xs)) == 7
    assert xs[0] == 42
    print("ok")
"#,
        "ok\n",
    );
}

#[test]
fn async_closure_arguments_keep_their_captured_storage() {
    assert_both(
        r#"async fn ready() -> int:
    return 1

async fn consume(f: fn(int) -> int) -> int:
    assert (await ready()) == 1
    return f(2)

async fn launch() -> Future[int]:
    let xs = [40]
    fn add(n: int) -> int:
        return xs[0] + n
    return consume(add)

fn main():
    print(await (await launch()))
"#,
        "42\n",
    );
}

#[test]
fn async_parameter_can_switch_from_owner_to_borrow() {
    assert_both(
        r#"async fn choose(mut xs: [int], ys: [int], change: bool) -> int:
    let old: int = __olive_unbox_int(xs)
    let generation: int = __olive_struct_gen_of(old)
    if change:
        xs = ys
        assert __olive_struct_gen_stale(old, generation) != 0
    else:
        assert __olive_struct_gen_stale(old, generation) == 0
    assert ys[0] == 7
    return xs[0]

fn main():
    let xs = [42]
    let ys = [7]
    assert (await choose(xs, ys, True)) == 7
    assert (await choose(xs, ys, False)) == 42
    assert xs[0] == 42
    assert ys[0] == 7
    print("ok")
"#,
        "ok\n",
    );
}
