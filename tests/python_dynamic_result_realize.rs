//! Regression coverage for a real bug found while building R12's bytes
//! round-trip benchmark: `coerce_pyobj_if_needed` (the post-call step for a
//! dynamic Python call whose result R10 didn't fuse -- a module function or
//! instance method with no stub, or any call shape the arity-specialized
//! entry points can't reach) only ever converted a numeric target
//! (`int`/`float`/`bool`, via a raw `Cast` the codegen special-cases for a
//! `PyObject` source). For every other declared result type -- `str`,
//! `bytes`, a typed list, a typed dict, a tuple -- it silently returned the
//! raw, unconverted `PyObject` handle, which then got read as if it were
//! already a value of that type (garbage: a `bytes`-typed local backed by a
//! PyObject handle read a nonsense length, for instance). The fix routes
//! through the general `coerce`, which already realizes every one of these
//! targets correctly and is exercised everywhere else in the compiler. This
//! file locks in every affected target type, both pipelines.

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

const DRHELPER_PY: &str = r#"
def make_bytes():
    return b"hello"

def make_list():
    return [1, 2, 3]

def make_dict():
    return {"a": 1, "b": 2}

def make_tuple():
    return (7, "seven")

class Widget:
    def label_bytes(self):
        return b"widget"

    def items(self):
        return [10, 20, 30]

def make_widget():
    return Widget()

def five_arg_str(a, b, c, d, e):
    return "joined"
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_dyn_realize_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("drhelper.py"), DRHELPER_PY).unwrap();
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
fn dynamic_module_call_bytes_result_realizes_correctly() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let x: bytes = h.make_bytes()
    print(len(x))

main()
"#,
        "5\n",
    );
}

#[test]
fn dynamic_module_call_typed_list_result_realizes_correctly() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let xs: [int] = h.make_list()
    print(xs)

main()
"#,
        "[1, 2, 3]\n",
    );
}

#[test]
fn dynamic_module_call_typed_dict_result_realizes_correctly() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let d: {str: int} = h.make_dict()
    print(d["a"] + d["b"])

main()
"#,
        "3\n",
    );
}

#[test]
fn dynamic_module_call_tuple_result_realizes_correctly() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let t: (int, str) = h.make_tuple()
    let n, s = t
    print(n)
    print(s)

main()
"#,
        "7\nseven\n",
    );
}

#[test]
fn dynamic_instance_method_bytes_result_realizes_correctly() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let w = h.make_widget()
    let b: bytes = w.label_bytes()
    print(len(b))

main()
"#,
        "6\n",
    );
}

#[test]
fn dynamic_instance_method_typed_list_result_realizes_correctly() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let w = h.make_widget()
    let xs: [int] = w.items()
    print(xs)

main()
"#,
        "[10, 20, 30]\n",
    );
}

/// Five positional args have no arity-specialized entry point at all
/// (R10 fusion never reaches this shape), so a stub-typed `str` result here
/// exercises the exact same unfused `coerce_pyobj_if_needed` path a
/// dynamically-typed call does -- pins that the fix didn't regress the
/// numeric/str case the old code happened to get right already.
#[test]
fn five_arg_stub_typed_str_result_still_realizes_correctly() {
    assert_both_succeed(
        r#"import py "drhelper" as h:
    fn five_arg_str(a: int, b: int, c: int, d: int, e: int) -> str

fn main():
    print(h.five_arg_str(1, 2, 3, 4, 5))

main()
"#,
        "joined\n",
    );
}
