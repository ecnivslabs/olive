//! A positional-only, tagged-fast-path call with 0-4 arguments routes
//! through `olive_py_call0..4`, which pass every argument straight in a call
//! register -- no `args_list` aggregate, no Olive list allocation for the
//! call site at all. Arity 5 (and up) keeps the list-based `olive_py_call_t`
//! path. This file proves both shapes produce correct, identical-looking
//! output from real Olive source across both pipelines (JIT `pit run`, AOT
//! release), and that a collection argument at a specialized arity still
//! round-trips its copy-out correctly.

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

const ARITYHELPER_PY: &str = r#"
def h0():
    return 111

def h1(a):
    return a + 1

def h2(a, b):
    return a + b

def h3(a, b, c):
    return a + b + c

def h4(a, b, c, d):
    return a + b + c + d

def h5(a, b, c, d, e):
    return a + b + c + d + e

def sort_inplace(xs):
    xs.sort()
    return len(xs)
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_call_arity_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("arityhelper.py"), ARITYHELPER_PY).unwrap();
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

/// Runs `src` on both pipelines and asserts each produces `expected` on
/// stdout.
fn assert_identical_on_both_pipelines(src: &str, expected: &str) {
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
fn arity0_call_has_no_args() {
    assert_identical_on_both_pipelines(
        r#"import py "arityhelper" as h

fn main():
    print(h.h0())

main()
"#,
        "111\n",
    );
}

#[test]
fn arity1_call() {
    assert_identical_on_both_pipelines(
        r#"import py "arityhelper" as h

fn main():
    let a: int = 41
    print(h.h1(a))

main()
"#,
        "42\n",
    );
}

#[test]
fn arity2_call() {
    assert_identical_on_both_pipelines(
        r#"import py "arityhelper" as h

fn main():
    let a: int = 10
    let b: int = 20
    print(h.h2(a, b))

main()
"#,
        "30\n",
    );
}

#[test]
fn arity3_call() {
    assert_identical_on_both_pipelines(
        r#"import py "arityhelper" as h

fn main():
    let a: int = 1
    let b: int = 2
    let c: int = 3
    print(h.h3(a, b, c))

main()
"#,
        "6\n",
    );
}

#[test]
fn arity4_call() {
    assert_identical_on_both_pipelines(
        r#"import py "arityhelper" as h

fn main():
    let a: int = 1
    let b: int = 2
    let c: int = 3
    let d: int = 4
    print(h.h4(a, b, c, d))

main()
"#,
        "10\n",
    );
}

/// Arity 5 has no fixed-register entry point and falls back to the
/// list-based `olive_py_call_t` path -- the boundary case right past the
/// specialized arities.
#[test]
fn arity5_call_falls_back_to_list_path() {
    assert_identical_on_both_pipelines(
        r#"import py "arityhelper" as h

fn main():
    let a: int = 1
    let b: int = 2
    let c: int = 3
    let d: int = 4
    let e: int = 5
    print(h.h5(a, b, c, d, e))

main()
"#,
        "15\n",
    );
}

/// Spot-check (not the full copy-out matrix): a collection argument at a
/// specialized arity still gets deep-realized into a real Python object and
/// its in-place mutation still syncs back into the same Olive allocation.
#[test]
fn arity1_call_with_collection_arg_still_syncs_back() {
    assert_identical_on_both_pipelines(
        r#"import py "arityhelper" as h

fn main():
    let xs: [int] = [3, 1, 2]
    print(h.sort_inplace(xs))
    print(xs)

main()
"#,
        "3\n[1, 2, 3]\n",
    );
}
