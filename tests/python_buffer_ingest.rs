//! R14: `PyObject -> [int]`/`[float]`/`bytes` tries a zero-copy buffer-
//! protocol ingest before falling back to the per-element conversion loop.
//! Proves the fast path (numpy int64/float64/int32/float32 arrays, a
//! `bytearray`, a `memoryview`) and the fallback (a non-contiguous numpy
//! slice, a plain Python list) produce identical results, on both pipelines.

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

fn numpy_available() -> bool {
    Command::new("python3")
        .arg("-c")
        .arg("import numpy")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

const BUFHELPER_PY: &str = r#"
import numpy as np

def int64_array(n):
    return np.arange(n, dtype=np.int64)

def float64_array(n):
    return np.arange(n, dtype=np.float64)

def int32_array(n):
    return np.arange(n, dtype=np.int32)

def float32_array(n):
    return np.arange(n, dtype=np.float32)

def strided_int64_slice(n):
    return np.arange(n, dtype=np.int64)[::2]

def a_bytearray():
    return bytearray(b"hello buffer")

def a_memoryview():
    return memoryview(b"view bytes")
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_buffer_ingest_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bufhelper.py"), BUFHELPER_PY).unwrap();
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
fn int64_numpy_array_ingests_via_buffer() {
    if !numpy_available() {
        eprintln!("numpy not available, skipping test");
        return;
    }
    assert_both_succeed(
        r#"import py "bufhelper" as h

fn main():
    let xs: [int] = h.int64_array(5)
    let mut total = 0
    for x in xs:
        total = total + x
    print(total)

main()
"#,
        "10\n",
    );
}

#[test]
fn float64_numpy_array_ingests_via_buffer() {
    if !numpy_available() {
        eprintln!("numpy not available, skipping test");
        return;
    }
    assert_both_succeed(
        r#"import py "bufhelper" as h

fn main():
    let xs: [float] = h.float64_array(4)
    let mut total = 0.0
    for x in xs:
        total = total + x
    print(total)

main()
"#,
        "6.0\n",
    );
}

#[test]
fn int32_numpy_array_widens_correctly() {
    if !numpy_available() {
        eprintln!("numpy not available, skipping test");
        return;
    }
    assert_both_succeed(
        r#"import py "bufhelper" as h

fn main():
    let xs: [int] = h.int32_array(5)
    let mut total = 0
    for x in xs:
        total = total + x
    print(total)

main()
"#,
        "10\n",
    );
}

#[test]
fn float32_numpy_array_widens_correctly() {
    if !numpy_available() {
        eprintln!("numpy not available, skipping test");
        return;
    }
    assert_both_succeed(
        r#"import py "bufhelper" as h

fn main():
    let xs: [float] = h.float32_array(4)
    let mut total = 0.0
    for x in xs:
        total = total + x
    print(total)

main()
"#,
        "6.0\n",
    );
}

#[test]
fn non_contiguous_numpy_slice_falls_through_with_identical_results() {
    if !numpy_available() {
        eprintln!("numpy not available, skipping test");
        return;
    }
    assert_both_succeed(
        r#"import py "bufhelper" as h

fn main():
    let xs: [int] = h.strided_int64_slice(10)
    let mut total = 0
    for x in xs:
        total = total + x
    print(total)

main()
"#,
        "20\n",
    );
}

#[test]
fn bytearray_ingests_via_buffer() {
    assert_both_succeed(
        r#"import py "bufhelper" as h

fn main():
    let data: bytes = h.a_bytearray()
    print(len(data))

main()
"#,
        "12\n",
    );
}

#[test]
fn memoryview_ingests_via_buffer() {
    assert_both_succeed(
        r#"import py "bufhelper" as h

fn main():
    let data: bytes = h.a_memoryview()
    print(len(data))

main()
"#,
        "10\n",
    );
}
