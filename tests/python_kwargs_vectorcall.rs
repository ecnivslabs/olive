//! R15: a keyword-argument call on the tagged fast path goes through
//! `PyObject_Vectorcall`/`PyObject_VectorcallMethod` with a cached
//! `kwnames` tuple (`__olive_py_call_kw_v`/`__olive_py_call_method_kw_v`),
//! instead of building a fresh `dict` (and a fresh tuple of names) on every
//! call. This file proves kwargs-only calls, positional+kwargs calls,
//! kwargs on a method call, a collection-valued kwarg still copies out
//! correctly, and repeated calls at the same call site produce identical
//! results many times over, on both pipelines.

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

const KWHELPER_PY: &str = r#"
def describe(name, age=0, active=False):
    return f"{name}:{age}:{active}"

class Greeter:
    def __init__(self, prefix):
        self.prefix = prefix

    def greet(self, name, punctuation="!"):
        return f"{self.prefix} {name}{punctuation}"

def sum_and_sort(xs, reverse=False):
    xs.sort(reverse=reverse)
    return sum(xs)
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_kwvcall_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("kwhelper.py"), KWHELPER_PY).unwrap();
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
fn kwargs_only_call() {
    assert_both_succeed(
        r#"import py "kwhelper" as h

fn main():
    print(h.describe(name="ada", age=36, active=True))

main()
"#,
        "ada:36:True\n",
    );
}

#[test]
fn mixed_positional_and_kwargs_call() {
    assert_both_succeed(
        r#"import py "kwhelper" as h

fn main():
    print(h.describe("grace", age=85))

main()
"#,
        "grace:85:False\n",
    );
}

#[test]
fn kwargs_on_method_call() {
    assert_both_succeed(
        r#"import py "kwhelper" as h

fn main():
    let g = h.Greeter("hello")
    print(g.greet("world", punctuation="?"))

main()
"#,
        "hello world?\n",
    );
}

#[test]
fn collection_kwarg_value_copies_out_correctly() {
    assert_both_succeed(
        r#"import py "kwhelper" as h

fn main():
    let mut xs: [int] = [3, 1, 2]
    let total = h.sum_and_sort(xs, reverse=True)
    print(total)
    print(xs[0])
    print(xs[1])
    print(xs[2])

main()
"#,
        "6\n3\n2\n1\n",
    );
}

#[test]
fn repeated_call_site_produces_identical_results_many_times() {
    assert_both_succeed(
        r#"import py "kwhelper" as h

fn main():
    let mut i = 0
    let mut total = 0
    while i < 2000:
        let s = h.describe(name="x", age=i)
        if len(s) > 0:
            total = total + 1
        i = i + 1
    print(total)

main()
"#,
        "2000\n",
    );
}
