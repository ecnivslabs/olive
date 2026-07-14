//! `olive_py_call_t`/`_safe` route through `PyObject_Vectorcall` when the
//! loaded libpython exports it, tuple-free. This file proves the fallback
//! path (`OLIVE_PY_NO_VECTORCALL=1`, forced off for a build that does have
//! it) produces byte-identical output to the default vectorcall path for
//! the same programs the tag scheme already covers -- the dispatch is an
//! internal runtime choice, invisible from the language side. Both
//! pipelines (JIT `pit run`, AOT release).

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

const TAGHELPER_PY: &str = r#"
def type_name(x):
    return type(x).__name__

def identity(x):
    return x

def add2(a, b):
    return a + b

def zero_args():
    return 99
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_vectorcall_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("taghelper.py"), TAGHELPER_PY).unwrap();
    let liv_path = dir.join("main.liv");
    let mut f = std::fs::File::create(&liv_path).unwrap();
    f.write_all(src.as_bytes()).unwrap();
    (dir, liv_path)
}

/// `no_vectorcall`: forces the tuple-call fallback for this process so both
/// entry points get exercised even on a build where vectorcall is present.
fn run_jit(dir: &Path, liv_path: &Path, no_vectorcall: bool) -> Output {
    let mut cmd = Command::new(pit_bin());
    cmd.arg("run")
        .arg(liv_path)
        .env("PYTHONPATH", dir)
        .stdin(Stdio::null());
    if no_vectorcall {
        cmd.env("OLIVE_PY_NO_VECTORCALL", "1");
    }
    cmd.output().expect("spawn pit run")
}

fn run_aot(dir: &Path, liv_path: &Path, no_vectorcall: bool) -> Output {
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
    let mut cmd = Command::new(&out_bin);
    cmd.env("PYTHONPATH", dir).stdin(Stdio::null());
    if no_vectorcall {
        cmd.env("OLIVE_PY_NO_VECTORCALL", "1");
    }
    let out = cmd.output().expect("spawn built binary");
    std::fs::remove_file(&out_bin).ok();
    out
}

/// Runs `src` four ways -- JIT and AOT, each with vectorcall on (default)
/// and forced off -- and asserts every run produces `expected` on stdout.
fn assert_identical_on_all_four_lanes(src: &str, expected: &str) {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(src);

    for &no_vectorcall in &[false, true] {
        let label = if no_vectorcall {
            "no-vectorcall"
        } else {
            "vectorcall"
        };

        let jit = run_jit(&dir, &liv_path, no_vectorcall);
        assert!(
            jit.status.success(),
            "pit run ({label}) failed: {}",
            String::from_utf8_lossy(&jit.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&jit.stdout),
            expected,
            "pit run ({label}) stderr: {}",
            String::from_utf8_lossy(&jit.stderr)
        );

        let aot = run_aot(&dir, &liv_path, no_vectorcall);
        assert!(
            aot.status.success(),
            "AOT ({label}) failed: {}",
            String::from_utf8_lossy(&aot.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&aot.stdout),
            expected,
            "AOT ({label}) stderr: {}",
            String::from_utf8_lossy(&aot.stderr)
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn scalar_args_identical_with_and_without_vectorcall() {
    assert_identical_on_all_four_lanes(
        r#"import py "taghelper" as h

fn main():
    let x: int = 42
    let y: float = 3.5
    let s: str = "hello"
    let b: bool = True
    print(h.type_name(x))
    print(h.identity(x))
    print(h.type_name(y))
    print(h.type_name(s))
    print(h.type_name(b))
    print(h.type_name(None))

main()
"#,
        "int\n42\nfloat\nstr\nbool\nNoneType\n",
    );
}

#[test]
fn two_arg_call_identical_with_and_without_vectorcall() {
    assert_identical_on_all_four_lanes(
        r#"import py "taghelper" as h

fn main():
    let a: int = 3
    let b: int = 4
    print(h.add2(a, b))

main()
"#,
        "7\n",
    );
}

#[test]
fn pyobject_arg_identical_with_and_without_vectorcall() {
    // Round-trips a live PyObject handle (ARG_PYOBJECT), not a raw scalar --
    // exercises the borrow-not-steal decref discipline vectorcall requires.
    assert_identical_on_all_four_lanes(
        r#"import py "taghelper" as h

fn main():
    let obj = h.identity(123)
    print(h.type_name(obj))
    print(h.identity(obj))

main()
"#,
        "int\n123\n",
    );
}

#[test]
fn zero_arg_call_identical_with_and_without_vectorcall() {
    assert_identical_on_all_four_lanes(
        r#"import py "taghelper" as h

fn main():
    print(h.zero_args())

main()
"#,
        "99\n",
    );
}
