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
fn update_from_any_uses_receiver_descriptor() {
    assert_both(
        r#"fn apply(src: Any) -> int:
    let dst: {int: int} = {}
    dst.update(src)
    return dst[1]

fn main():
    let src: {Any: Any} = {1: 42}
    print(apply(src))

main()
"#,
        "42\n",
    );
}

#[test]
fn update_from_any_converts_structural_keys_and_nested_values() {
    assert_both(
        r#"fn apply(src: Any) -> int:
    let dst: {(int, int): [int]} = {}
    dst.update(src)
    return len(dst[(1, 2)])

fn main():
    let src: {Any: Any} = {(1, 2): [3, 4]}
    print(apply(src))

main()
"#,
        "2\n",
    );
}

#[test]
fn erased_u64_formats_as_unsigned_value() {
    assert_both(
        r#"fn main():
    let one: u64 = 1
    let high: u64 = one << 63
    let value: Any = high
    print(str(value))
"#,
        "9223372036854775808\n",
    );
}

#[test]
fn concrete_dict_get_boxes_hits_with_the_stored_value_type() {
    assert_both(
        r#"fn main():
    let floats: {int: float} = {1: -1.5}
    let f: Any = floats.get(1, None)
    print(f)
    print(type(f))

    let one: u64 = 1
    let high: u64 = one << 63
    let unsigned: {int: u64} = {1: high}
    let u: Any = unsigned.get(1, None)
    print(u)
    print(type(u))

    let bools: {int: bool} = {1: True}
    let b: Any = bools.get(1, 0)
    print(b)
    print(type(b))

    let ints: {int: int} = {1: 2}
    let i: Any = ints.get(1, None)
    print(i)
    print(type(i))

    let f32s: {int: f32} = {1: 1.5}
    let small: Any = f32s.get(1, None)
    print(small)
    print(type(small))
    let missing: Any = f32s.get(2, None)
    print(missing)
"#,
        "-1.5\nfloat\n9223372036854775808\nu64\nTrue\nbool\n2\nint\n1.5\nfloat\nNone\n",
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

#[test]
fn interned_char_key_matches_heap_spelling() {
    assert_both(
        r#"fn main():
    let s = "abc"
    let c = s[0]
    let d: dict[Any, int] = {}
    d[c] = 7
    print(d["a"])
    let e: dict[Any, int] = {"a": 42}
    print(e[c])
"#,
        "7\n42\n",
    );
}

#[test]
fn erased_struct_set_keeps_values_and_hooks_once() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn main():
    let st = {Res("a")}
    let a: Any = st
    st.add(Res("b"))
    print(len(a))
    print(len(st))
    print("done")
"#,
        "1\n2\ndone\ndrop a\ndrop b\n",
    );
}

#[test]
fn erased_struct_set_remove_through_any() {
    assert_both(
        r#"struct P:
    x: int
fn main():
    let s = {P(1), P(2)}
    let a: Any = s
    print(len(a))
    a.remove(P(1))
    print(len(a))
    print("done")
"#,
        "2\n1\ndone\n",
    );
}

#[test]
fn erased_struct_dict_values_read_back() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn show(r: Res):
    print("saw "+r.s)

fn main():
    let d = {"k": Res("v")}
    let a: Any = d
    show(a["k"])
    print("done")
"#,
        "saw v\ndone\ndrop v\n",
    );
}

#[test]
fn erased_struct_tuple_reads_back() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn show(r: Res):
    print("saw "+r.s)

fn main():
    let t: Any = (Res("a"), 1)
    show(t[0])
    print(t[1])
    print("done")
"#,
        "saw a\n1\ndone\ndrop a\n",
    );
}

#[test]
fn erased_nullable_union_both_arms() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn maybe(b: bool) -> Res | None:
    if b:
        return Res("x")
    return None

fn show(r: Res):
    print("saw "+r.s)

fn main():
    let hit: Any = maybe(True)
    show(hit)
    let miss: Any = maybe(False)
    print(miss)
    print("done")
"#,
        "saw x\n0\ndone\ndrop x\n",
    );
}

#[test]
fn erased_struct_keyed_dict_iterates() {
    assert_both(
        r#"struct Key:
    k: str
impl Key:
    fn __drop__(self):
        print("dropk "+self.k)

fn main():
    let d = {(Key("a")): 1}
    let a: Any = d
    for k in a:
        print("iter done")
    print("done")
"#,
        "iter done\ndone\ndropk a\n",
    );
}

#[test]
fn erased_struct_keyed_dict_looks_up() {
    assert_both(
        r#"struct Key:
    k: str
fn get(d: Any, k: Any):
    return d[k]
fn main():
    let d = {(Key("a")): 1, (Key("b")): 2}
    let a: Any = d
    print(a[Key("a")])
    print(a[Key("b")])
    print(a.get(Key("a"), -1))
    let k: Any = Key("b")
    print(get(a, k))
    a[Key("c")] = 3
    print(a[Key("c")])
    print("done")
"#,
        "1\n2\n1\n2\n3\ndone\n",
    );
}

#[test]
fn erased_struct_keyed_dict_drops_exactly() {
    assert_both(
        r#"struct Key:
    k: str
impl Key:
    fn __drop__(self):
        print("dropk "+self.k)
fn main():
    let d = {(Key("a")): 1}
    let a: Any = d
    print(a[Key("a")])
    print("done")
"#,
        "1\ndone\ndropk a\ndropk a\n",
    );
}

#[test]
fn erased_enum_keyed_dict_looks_up() {
    assert_both(
        r#"enum Shape:
    Circle(int)
    Square(int)
fn get(d: Any, k: Any):
    return d[k]
fn main():
    let d = {(Circle(5)): 1, (Square(2)): 2}
    let a: Any = d
    print(a[Circle(5)])
    print(a[Square(2)])
    print(a.get(Circle(9), -1))
    let k: Any = Square(2)
    print(get(a, k))
    print("done")
"#,
        "1\n2\n-1\n2\ndone\n",
    );
}

#[test]
fn erased_tuple_keyed_dict_looks_up() {
    assert_both(
        r#"fn get(d: Any, k: Any):
    return d[k]
fn main():
    let d = {((1, 2)): 10, ((3, 4)): 20}
    let a: Any = d
    print(a[(1, 2)])
    print(a[(3, 4)])
    let k: Any = (3, 4)
    print(get(a, k))
    print("done")
"#,
        "10\n20\n20\ndone\n",
    );
}

#[test]
fn erased_set_and_dict_keyed_dict_looks_up() {
    assert_both(
        r#"fn main():
    let s = {({1, 2}): 10}
    let a: Any = s
    print(a[{1, 2}])
    print(a.get(({2, 1}), -1))
    print("done")
"#,
        "10\n10\ndone\n",
    );
}

#[test]
fn struct_keys_snapshot_and_iterate() {
    assert_both(
        r#"struct Key:
    k: str
impl Key:
    fn __drop__(self):
        print("dropk "+self.k)

fn show(k: Key):
    print("saw "+k.k)

fn main():
    let d = {(Key("a")): 1}
    let ks = d.keys()
    show(ks[0])
    for k in d:
        show(k)
    print("done")
"#,
        "saw a\nsaw a\ndone\ndropk a\n",
    );
}

#[test]
fn erased_tuple_in_wide_union_reads_back() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn make(b: bool) -> (Res, int) | int:
    if b:
        return (Res("a"), 1)
    return 0

fn main():
    let a: Any = make(True)
    print(a)
    print("done")
"#,
        "[Res(s=\"a\"), 1]\ndone\ndrop a\n",
    );
}
