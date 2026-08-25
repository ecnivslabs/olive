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
