//! R2b: a collection argument passed into a Python call is synced back into
//! the same Olive allocation after the call returns, on both the success and
//! the exception path, so `xs.sort()`, `random.shuffle(xs)`, `d.update(...)`
//! called from Olive on an Olive collection behave exactly like the
//! equivalent Python code. Both pipelines (JIT `pit run`, AOT release).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

/// A Python module every test can import functions from: real Python
/// mutating methods on the Olive collection passed as an argument. Native
/// Olive `.append`/`.sort`/`.update`/etc dispatch to Olive's own methods, not
/// Python's, so a helper function is the only way to exercise Python-side
/// mutation of a passed-in value.
const WBHELPER_PY: &str = r#"
def just_append(xs, v):
    xs.append(v)

def just_pop(xs):
    return xs.pop()

def do_sort(xs):
    xs.sort()

def do_update(d, extra):
    d.update(extra)

def mutate_then_raise(xs):
    xs.append(999)
    raise ValueError("boom")

def push_wrong_type(xs, v):
    xs[0] = v

def same_list_twice(a, b):
    a.append(1)
    return len(b)

def touch_nested(outer):
    outer[0].append(42)

def flip_set(s):
    s.add(999)
    s.discard(1)

def print_set(s):
    print(sorted(s))

def append_first17(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15, a16):
    a0.append(999)
"#;

/// Writes `wbhelper.py` and a `main.liv` (source `src`) into a fresh temp
/// directory, returning the directory and the `.liv` path.
fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_writeback_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("wbhelper.py"), WBHELPER_PY).unwrap();
    let liv_path = dir.join("main.liv");
    let mut f = std::fs::File::create(&liv_path).unwrap();
    f.write_all(src.as_bytes()).unwrap();
    (dir, liv_path)
}

fn run_jit(dir: &Path, liv_path: &Path) -> Output {
    Command::new(pit_bin())
        .arg("run")
        .arg(liv_path)
        .env("PYTHONPATH", dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run")
}

fn run_aot(dir: &Path, liv_path: &Path) -> Output {
    let out_bin = liv_path.with_extension("bin");
    let build = Command::new(pit_bin())
        .arg("build")
        .arg("--release")
        .arg(liv_path)
        .arg("-o")
        .arg(&out_bin)
        .env("PYTHONPATH", dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit build");
    assert!(
        build.status.success(),
        "AOT build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(&out_bin)
        .env("PYTHONPATH", dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn built binary");
    std::fs::remove_file(&out_bin).ok();
    out
}

/// Runs `src` under both pipelines and asserts each succeeds with stdout
/// exactly `expected`.
fn assert_both_succeed(src: &str, expected: &str) {
    assert_both_succeed_with(src, |stdout, pipeline, stderr| {
        assert_eq!(stdout, expected, "{pipeline} stderr: {stderr}");
    });
}

/// Runs `src` under both pipelines and hands each stdout to `check`. Use this
/// over `assert_both_succeed` when the output embeds a printed dict, whose
/// entry order follows the hash and is not a property worth pinning.
fn assert_both_succeed_with(src: &str, check: impl Fn(&str, &str, &str)) {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(src);

    let jit = run_jit(&dir, &liv_path);
    assert!(
        jit.status.success(),
        "pit run failed: {}",
        String::from_utf8_lossy(&jit.stderr)
    );
    check(
        &String::from_utf8_lossy(&jit.stdout),
        "jit",
        &String::from_utf8_lossy(&jit.stderr),
    );

    let aot = run_aot(&dir, &liv_path);
    assert!(
        aot.status.success(),
        "AOT binary failed: {}",
        String::from_utf8_lossy(&aot.stderr)
    );
    check(
        &String::from_utf8_lossy(&aot.stdout),
        "aot",
        &String::from_utf8_lossy(&aot.stderr),
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Runs `src` under both pipelines and asserts each fails with stderr
/// containing `needle`.
fn assert_both_fail_with(src: &str, needle: &str) {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(src);

    let jit = run_jit(&dir, &liv_path);
    assert!(!jit.status.success(), "pit run unexpectedly succeeded");
    assert!(
        String::from_utf8_lossy(&jit.stderr).contains(needle),
        "jit stderr missing {needle:?}: {}",
        String::from_utf8_lossy(&jit.stderr)
    );

    let aot = run_aot(&dir, &liv_path);
    assert!(!aot.status.success(), "AOT binary unexpectedly succeeded");
    assert!(
        String::from_utf8_lossy(&aot.stderr).contains(needle),
        "aot stderr missing {needle:?}: {}",
        String::from_utf8_lossy(&aot.stderr)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn shuffle_reorders_the_olive_list_in_place() {
    assert_both_succeed(
        r#"import py "random" as random

fn main():
    let mut xs: [int] = [1, 2, 3, 4, 5]
    random.seed(0)
    random.shuffle(xs)
    print(xs)

main()
"#,
        "[3, 2, 1, 5, 4]\n",
    );
}

#[test]
fn python_sort_mutates_the_passed_list() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut xs: [int] = [5, 3, 4, 1, 2]
    h.do_sort(xs)
    print(xs)

main()
"#,
        "[1, 2, 3, 4, 5]\n",
    );
}

#[test]
fn dict_update_mutates_the_passed_dict() {
    assert_both_succeed_with(
        r#"import py "wbhelper" as h

fn main():
    let mut d: {str: int} = {"a": 1, "b": 2}
    let extra: {str: int} = {"c": 3}
    h.do_update(d, extra)
    print(d)

main()
"#,
        |stdout, pipeline, stderr| {
            let printed = stdout.trim_end();
            for entry in ["\"a\": 1", "\"b\": 2", "\"c\": 3"] {
                assert!(
                    printed.contains(entry),
                    "{pipeline}: {printed:?} missing {entry}, stderr: {stderr}"
                );
            }
            assert_eq!(printed.matches(": ").count(), 3, "{pipeline}: {printed:?}");
        },
    );
}

#[test]
fn append_and_pop_grow_and_shrink_the_passed_list() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut xs: [int] = [1, 2, 3]
    h.just_append(xs, 42)
    print(xs)
    let popped = h.just_pop(xs)
    print(xs)
    print(popped)

main()
"#,
        "[1, 2, 3, 42]\n[1, 2, 3]\n42\n",
    );
}

#[test]
fn exception_path_still_syncs_the_partial_mutation() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn call_it(xs: [int]) -> int | Error:
    try h.mutate_then_raise(xs)
    return 0

fn main():
    let mut xs: [int] = [1, 2, 3]
    match call_it(xs):
        Error(_):
            print("caught")
        n:
            print(n)
    print(xs)

main()
"#,
        "caught\n[1, 2, 3, 999]\n",
    );
}

#[test]
fn same_list_passed_twice_aliases_one_python_object() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut xs: [int] = [1, 2, 3]
    let n = h.same_list_twice(xs, xs)
    print(n)
    print(xs)

main()
"#,
        "4\n[1, 2, 3, 1]\n",
    );
}

#[test]
fn wrong_element_type_faults_with_e0714() {
    assert_both_fail_with(
        r#"import py "wbhelper" as h

fn main():
    let mut xs: [int] = [1, 2, 3]
    h.push_wrong_type(xs, "oops")
    print(xs)

main()
"#,
        "E0714",
    );
}

#[test]
fn any_typed_nested_list_syncs_the_inner_mutation() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut outer: [Any] = [[1, 2], [3]]
    h.touch_nested(outer)
    print(outer)

main()
"#,
        "[[1, 2, 42], [3]]\n",
    );
}

/// A concretely nested list (`[[int]]`, not `[Any]`) has no flat 4-bit tag
/// that can express "the inner element is itself a raw int list" (see
/// `py_collection_tag`'s doc comment), so this argument gets no copy-out at
/// all -- the same honest, documented limitation as R2a's pre-writeback
/// behavior, not silent corruption.
#[test]
fn concretely_typed_nested_list_is_a_safe_no_op() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut outer: [[int]] = [[1, 2], [3]]
    h.touch_nested(outer)
    print(outer)

main()
"#,
        "[[1, 2], [3]]\n",
    );
}

#[test]
fn empty_list_and_dict_arguments_are_handled() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut xs: [int] = []
    h.just_append(xs, 7)
    print(xs)
    let mut d: {str: int} = {}
    let extra: {str: int} = {"z": 9}
    h.do_update(d, extra)
    print(d)

main()
"#,
        "[7]\n{\"z\": 9}\n",
    );
}

#[test]
fn set_membership_change_syncs() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut s: set[int] = {1, 2, 4, 6}
    h.flip_set(s)
    h.print_set(s)

main()
"#,
        "[2, 4, 6, 999]\n",
    );
}

#[test]
fn seventeen_argument_call_forces_no_copy_out_without_crashing() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut xs: [int] = [1, 2, 3]
    h.append_first17(xs, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16)
    print(xs)

main()
"#,
        "[1, 2, 3]\n",
    );
}

#[test]
fn repeated_calls_on_the_same_list_do_not_leak() {
    assert_both_succeed(
        r#"import py "sys" as sys
import py "wbhelper" as h

fn main():
    let mut xs: [int] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
    let mut i = 0
    while i < 2000:
        h.do_sort(xs)
        i = i + 1
    let before = sys.getallocatedblocks()
    let mut j = 0
    while j < 2000:
        h.do_sort(xs)
        j = j + 1
    let after = sys.getallocatedblocks()
    print(after - before < 200)

main()
"#,
        "True\n",
    );
}
