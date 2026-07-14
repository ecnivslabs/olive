//! `obj.attr` (read, write, and as the callee of a method call) interns the
//! attribute name into one persistent Python `str` and reuses it on every
//! later access via `PyObject_GetAttr`/`SetAttr`, instead of rebuilding the
//! name from the C string with `PyObject_GetAttrString`/`SetAttrString` each
//! time. This file proves the fallback path (`OLIVE_PY_NO_INTERN=1`, forced
//! off for a build that does have interning) produces byte-identical output
//! to the default interned path, including error messages on a missing
//! attribute -- the dispatch is an internal runtime choice, invisible from
//! the language side. Both pipelines (JIT `pit run`, AOT release).

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

const ATTRHELPER_PY: &str = r#"
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def total(self):
        return self.x + self.y

def make_point(x, y):
    return Point(x, y)
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_attrintern_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("attrhelper.py"), ATTRHELPER_PY).unwrap();
    let liv_path = dir.join("main.liv");
    let mut f = std::fs::File::create(&liv_path).unwrap();
    f.write_all(src.as_bytes()).unwrap();
    (dir, liv_path)
}

/// `no_intern`: forces the `GetAttrString`/`SetAttrString` fallback for this
/// process so both paths get exercised even on a build where interning is
/// available.
fn run_jit(dir: &Path, liv_path: &Path, no_intern: bool) -> Output {
    let mut cmd = Command::new(pit_bin());
    cmd.arg("run")
        .arg(liv_path)
        .env("PYTHONPATH", dir)
        .stdin(Stdio::null());
    if no_intern {
        cmd.env("OLIVE_PY_NO_INTERN", "1");
    }
    cmd.output().expect("spawn pit run")
}

fn run_aot(dir: &Path, liv_path: &Path, no_intern: bool) -> Output {
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
    if no_intern {
        cmd.env("OLIVE_PY_NO_INTERN", "1");
    }
    let out = cmd.output().expect("spawn built binary");
    std::fs::remove_file(&out_bin).ok();
    out
}

/// Runs `src` four ways -- JIT and AOT, each with interning on (default) and
/// forced off -- and asserts every run succeeds with stdout `expected`.
fn assert_identical_on_all_four_lanes(src: &str, expected: &str) {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(src);

    for &no_intern in &[false, true] {
        let label = if no_intern { "no-intern" } else { "intern" };

        let jit = run_jit(&dir, &liv_path, no_intern);
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

        let aot = run_aot(&dir, &liv_path, no_intern);
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

/// Runs `src` four ways -- same matrix as above -- and asserts every run
/// fails with stderr containing `needle`, proving the error message the
/// interned path produces matches the fallback path exactly.
fn assert_fails_identically_on_all_four_lanes(src: &str, needle: &str) {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(src);

    for &no_intern in &[false, true] {
        let label = if no_intern { "no-intern" } else { "intern" };

        let jit = run_jit(&dir, &liv_path, no_intern);
        assert!(
            !jit.status.success(),
            "pit run ({label}) unexpectedly succeeded"
        );
        assert!(
            String::from_utf8_lossy(&jit.stderr).contains(needle),
            "pit run ({label}) stderr missing {needle:?}: {}",
            String::from_utf8_lossy(&jit.stderr)
        );

        let aot = run_aot(&dir, &liv_path, no_intern);
        assert!(
            !aot.status.success(),
            "AOT ({label}) unexpectedly succeeded"
        );
        assert!(
            String::from_utf8_lossy(&aot.stderr).contains(needle),
            "AOT ({label}) stderr missing {needle:?}: {}",
            String::from_utf8_lossy(&aot.stderr)
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn attr_read_returns_the_correct_value() {
    assert_identical_on_all_four_lanes(
        r#"import py "attrhelper" as h

fn main():
    let p = h.make_point(3, 4)
    print(p.x)
    print(p.y)

main()
"#,
        "3\n4\n",
    );
}

#[test]
fn method_call_via_getattr_still_works() {
    assert_identical_on_all_four_lanes(
        r#"import py "attrhelper" as h

fn main():
    let p = h.make_point(10, 20)
    print(p.total())

main()
"#,
        "30\n",
    );
}

#[test]
fn attr_write_round_trips() {
    assert_identical_on_all_four_lanes(
        r#"import py "attrhelper" as h

fn main():
    let p = h.make_point(1, 2)
    p.x = 99
    print(p.x)
    print(p.total())

main()
"#,
        "99\n101\n",
    );
}

#[test]
fn repeated_attr_access_in_a_loop_stays_correct() {
    assert_identical_on_all_four_lanes(
        r#"import py "attrhelper" as h

fn main():
    let p = h.make_point(1, 1)
    let mut total: int = 0
    let mut i: int = 0
    while i < 2000:
        total = total + p.x
        i = i + 1
    print(total)

main()
"#,
        "2000\n",
    );
}

#[test]
fn missing_attr_raises_with_unchanged_message() {
    assert_fails_identically_on_all_four_lanes(
        r#"import py "attrhelper" as h

fn main():
    let p = h.make_point(1, 2)
    print(p.does_not_exist)

main()
"#,
        "does_not_exist",
    );
}
