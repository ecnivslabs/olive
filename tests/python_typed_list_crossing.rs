//! R12: a concretely-typed list argument (`[int]`/`[float]`/`[bool]`/`[str]`)
//! crossing into a Python call converts each element directly by its
//! compiler-known scalar kind (`to_py_typed_list` in
//! `std_lib/src/python/python_writeback.rs`), instead of the generic
//! per-element runtime-guessed dispatch `to_py_deep`/`olive_to_py` used to
//! run for every element regardless of the list's own static type. This file
//! proves every scalar kind still crosses with the exact values intact, that
//! an empty typed list is handled, and that a large list survives the
//! crossing correctly at a scale where a single off-by-one or dropped
//! element would show up in a checksum. Both pipelines.

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

const TLHELPER_PY: &str = r#"
def sum_ints(xs):
    return sum(xs)

def sum_floats(xs):
    return sum(xs)

def all_true(xs):
    return all(xs)

def joined(xs):
    return "-".join(xs)

def count(xs):
    return len(xs)
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_typed_list_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("tlhelper.py"), TLHELPER_PY).unwrap();
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

fn assert_both_succeed(src: &str, expected: &str) {
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
    assert_eq!(
        String::from_utf8_lossy(&jit.stdout),
        expected,
        "pit run stderr: {}",
        String::from_utf8_lossy(&jit.stderr)
    );

    let aot = run_aot(&dir, &liv_path);
    assert!(
        aot.status.success(),
        "AOT failed: {}",
        String::from_utf8_lossy(&aot.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&aot.stdout),
        expected,
        "AOT stderr: {}",
        String::from_utf8_lossy(&aot.stderr)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn typed_int_list_crosses_with_values_intact() {
    assert_both_succeed(
        r#"import py "tlhelper" as h:
    fn sum_ints(xs: [int]) -> int

fn main():
    let xs: [int] = [1, 2, 3, 4, 5]
    print(h.sum_ints(xs))

main()
"#,
        "15\n",
    );
}

#[test]
fn typed_float_list_crosses_with_values_intact() {
    assert_both_succeed(
        r#"import py "tlhelper" as h:
    fn sum_floats(xs: [float]) -> float

fn main():
    let xs: [float] = [1.5, 2.5, 3.0]
    print(h.sum_floats(xs))

main()
"#,
        "7.0\n",
    );
}

#[test]
fn typed_bool_list_crosses_with_values_intact() {
    assert_both_succeed(
        r#"import py "tlhelper" as h:
    fn all_true(xs: [bool]) -> bool

fn main():
    let xs: [bool] = [True, True, True]
    print(h.all_true(xs))

main()
"#,
        "True\n",
    );
}

#[test]
fn typed_str_list_crosses_with_values_intact() {
    assert_both_succeed(
        r#"import py "tlhelper" as h:
    fn joined(xs: [str]) -> str

fn main():
    let xs: [str] = ["a", "b", "c"]
    print(h.joined(xs))

main()
"#,
        "a-b-c\n",
    );
}

#[test]
fn empty_typed_list_crosses_cleanly() {
    assert_both_succeed(
        r#"import py "tlhelper" as h:
    fn count(xs: [int]) -> int

fn main():
    let xs: [int] = []
    print(h.count(xs))

main()
"#,
        "0\n",
    );
}

/// A million-element crossing at a scale where a dropped or corrupted
/// element would show up in the checksum, not just a hand-picked tiny list.
#[test]
fn million_element_int_list_crosses_correctly() {
    assert_both_succeed(
        r#"import py "tlhelper" as h:
    fn sum_ints(xs: [int]) -> int

fn main():
    let mut xs: [int] = []
    let mut i = 0
    while i < 1000000:
        xs.append(i)
        i = i + 1
    print(h.sum_ints(xs))

main()
"#,
        "499999500000\n",
    );
}
