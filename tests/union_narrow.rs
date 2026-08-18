#[path = "support/program.rs"]
mod program;
use program::{assert_both, assert_both_with};

#[test]
fn match_sentinel_arm_narrows_catch_all() {
    assert_both(
        r#"struct C:
    x: int

fn f(v: C | int) -> int:
    match v:
        case 0:
            return -1
        case n:
            return n.x

fn main():
    let v: C | int = C(41)
    print(f(v))
    print(f(0))
"#,
        "41\n-1\n",
    );
}

#[test]
fn guard_sentinel_narrows_struct() {
    assert_both(
        r#"struct C:
    x: int

fn main():
    let c: C | int = C(41)
    if c != 0:
        print(c.x)
    else:
        print("zero")
"#,
        "41\n",
    );
}

#[test]
fn narrowed_union_drop_hook_runs_once() {
    assert_both(
        r#"struct C:
    x: int
impl C:
    fn __drop__(self):
        print("bye")

fn main():
    let v: C | int = C(7)
    match v:
        case 0:
            print("zero")
        case n:
            print(n.x)
    print("end")
"#,
        "7\nend\nbye\n",
    );
}

#[test]
fn match_non_sentinel_int_faults_instead_of_misreading() {
    assert_both_with(
        r#"struct C:
    x: int

fn f(v: C | int) -> int:
    match v:
        case 0:
            return -1
        case n:
            return n.x

fn main():
    print(f(5))
"#,
        |status, _, stderr| {
            assert_eq!(status.code(), Some(1), "{status}: {stderr}");
            assert!(stderr.contains("E0715"), "{stderr}");
            assert!(stderr.contains("where a struct was expected"), "{stderr}");
        },
    );
}

#[test]
fn guard_non_sentinel_int_faults_instead_of_misreading() {
    assert_both_with(
        r#"struct C:
    x: int

fn main():
    let c: C | int = 5
    if c != 0:
        print(c.x)
    else:
        print("zero")
"#,
        |status, _, stderr| {
            assert_eq!(status.code(), Some(1), "{status}: {stderr}");
            assert!(stderr.contains("E0715"), "{stderr}");
            assert!(stderr.contains("an integer"), "{stderr}");
        },
    );
}

#[test]
fn narrowed_list_of_structs_reads_exact_values() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn use_it(v: [Res] | int):
    match v:
        case 0:
            print("zero")
        case lst:
            print(lst[0].s)
            print("list arm")

fn main():
    use_it([Res("hello")])
    use_it(0)
    print("done")
"#,
        "hello\nlist arm\nzero\ndone\ndrop hello\n",
    );
}

#[test]
fn narrowed_list_of_ints_reads_exact_values() {
    assert_both(
        r#"fn use_it(v: [int] | int):
    match v:
        case 0:
            print("zero")
        case lst:
            print(lst[0])
            print("list arm")

fn main():
    use_it([42])
    use_it(0)
    print("done")
"#,
        "42\nlist arm\nzero\ndone\n",
    );
}

#[test]
fn narrowed_dict_of_structs_reads_exact_values() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn use_it(v: dict[str, Res] | int):
    match v:
        case 0:
            print("zero")
        case d:
            print(d["k"].s)
            print("dict arm")

fn main():
    use_it({"k": Res("v")})
    use_it(0)
    print("done")
"#,
        "v\ndict arm\nzero\ndone\ndrop v\n",
    );
}

#[test]
fn narrowed_set_of_structs_reads_exact_values() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn use_it(v: set[Res] | int):
    match v:
        case 0:
            print("zero")
        case s:
            print(len(s))
            print("set arm")

fn main():
    use_it({Res("a")})
    use_it(0)
    print("done")
"#,
        "1\nset arm\nzero\ndone\ndrop a\n",
    );
}

#[test]
fn narrowed_nested_list_reads_exact_values() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn use_it(v: [[Res]] | int):
    match v:
        case 0:
            print("zero")
        case lst:
            print(lst[0][0].s)
            print("nested arm")

fn main():
    use_it([[Res("deep")]])
    use_it(0)
    print("done")
"#,
        "deep\nnested arm\nzero\ndone\ndrop deep\n",
    );
}

#[test]
fn narrowed_tuple_with_struct_reads_exact_values() {
    assert_both(
        r#"struct Res:
    s: str
impl Res:
    fn __drop__(self):
        print("drop "+self.s)

fn show(r: Res):
    print("saw "+r.s)

fn use_it(v: (Res, int) | int):
    match v:
        case 0:
            print("zero")
        case t:
            show(t[0])
            print(t[1])
            print("tuple arm")

fn main():
    use_it((Res("a"), 7))
    use_it(0)
    print("done")
"#,
        "saw a\n7\ntuple arm\nzero\ndone\ndrop a\n",
    );
}
