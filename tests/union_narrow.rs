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
