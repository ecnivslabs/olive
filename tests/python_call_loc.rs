//! R17: call-site location without a runtime call. Before this phase, every
//! `__olive_py_call*` site got a separate `__olive_py_set_loc` MIR statement
//! pair ahead of it. The R7/R9/R15 fast-path entry points now take that
//! location as a plain trailing call argument instead -- this file proves
//! the diagnostics stay byte-identical (same fault code, same `file:line:col`,
//! same message) whether a call goes through the fast path or the legacy
//! list-based fallback, and that a `try`-caught exception still carries the
//! same location prefix. Both pipelines (JIT `pit run`, AOT release).

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

const LOCHELPER_PY: &str = r#"
def boom0():
    raise ValueError("boom0 failed on purpose")

def boom5(a, b, c, d, e):
    raise ValueError("boom5 failed on purpose")

def boom_kw(x=None):
    raise ValueError("boom_kw failed on purpose")

class Boomer:
    def boom_method(self):
        raise ValueError("boom_method failed on purpose")
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_call_loc_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lochelper.py"), LOCHELPER_PY).unwrap();
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

/// Runs `src` on both pipelines, asserting each uncaught run fails with
/// stderr containing the `E0705` fault code, the exact `<file>:<line>:<col>`
/// call-site location, and the Python exception message -- proves the
/// fast-path fold-in produces exactly the same diagnostic shape the legacy
/// `__olive_py_set_loc` statement did.
fn assert_uncaught_reports_loc(src: &str, line: u32, col: u32, py_msg: &str) {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(src);
    let expected_loc = format!("{}:{}:{}", liv_path.display(), line, col);

    for (label, out) in [
        ("jit", run_jit(&dir, &liv_path)),
        ("aot", run_aot(&dir, &liv_path)),
    ] {
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success(),
            "{label} unexpectedly succeeded: {stderr}"
        );
        assert!(
            stderr.contains("[E0705] panic: uncaught Python exception"),
            "{label} stderr missing E0705 fault:\n{stderr}"
        );
        assert!(
            stderr.contains(&expected_loc),
            "{label} stderr missing call-site location {expected_loc:?}:\n{stderr}"
        );
        assert!(
            stderr.contains(py_msg),
            "{label} stderr missing Python message {py_msg:?}:\n{stderr}"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fast_path_plain_call_reports_exact_call_site() {
    assert_uncaught_reports_loc(
        r#"import py "lochelper" as h

fn main():
    let f = h.boom0
    f()

main()
"#,
        5,
        5,
        "boom0 failed on purpose",
    );
}

#[test]
fn fast_path_fused_method_call_reports_exact_call_site() {
    assert_uncaught_reports_loc(
        r#"import py "lochelper" as h

fn main():
    let obj = h.Boomer()
    obj.boom_method()

main()
"#,
        5,
        5,
        "boom_method failed on purpose",
    );
}

#[test]
fn fast_path_kwargs_call_reports_exact_call_site() {
    assert_uncaught_reports_loc(
        r#"import py "lochelper" as h

fn main():
    h.boom_kw(x=1)

main()
"#,
        4,
        5,
        "boom_kw failed on purpose",
    );
}

#[test]
fn legacy_list_path_call_reports_exact_call_site() {
    assert_uncaught_reports_loc(
        r#"import py "lochelper" as h

fn main():
    h.boom5(1, 2, 3, 4, 5)

main()
"#,
        4,
        5,
        "boom5 failed on purpose",
    );
}

/// A `try`-caught fast-path exception still carries the `<file>:<line>:<col>:
/// ` message prefix `prepend_call_loc` builds -- a separate mechanism from
/// the abort-path thread-local this phase changed, so this is a regression
/// check that R17 left it untouched.
#[test]
fn try_caught_fast_path_message_keeps_loc_prefix() {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(
        r#"import py "lochelper" as h

fn call_it() -> int | Error:
    try h.boom0()
    return 0

fn main():
    match call_it():
        Error(msg):
            print(msg)
        n:
            print(n)

main()
"#,
    );
    let expected_prefix = format!("{}:4:5: ", liv_path.display());

    for (label, out) in [
        ("jit", run_jit(&dir, &liv_path)),
        ("aot", run_aot(&dir, &liv_path)),
    ] {
        assert!(
            out.status.success(),
            "{label} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.starts_with(&expected_prefix),
            "{label} stdout missing loc prefix {expected_prefix:?}:\n{stdout}"
        );
        assert!(
            stdout.contains("boom0 failed on purpose"),
            "{label} stdout missing Python message:\n{stdout}"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// `--emit-mir` half of the acceptance criterion: a fast-path call emits no
/// `__olive_py_set_loc`, the legacy arity-5 path still does.
#[test]
fn emit_mir_confirms_set_loc_only_on_legacy_path() {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(
        r#"import py "lochelper" as h

fn fast_call():
    h.boom0()

fn legacy_call():
    h.boom5(1, 2, 3, 4, 5)

fast_call()
legacy_call()
"#,
    );
    let out = Command::new(pit_bin())
        .arg("run")
        .arg("--emit-mir")
        .arg(&liv_path)
        .env("PYTHONPATH", &dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run --emit-mir");
    let mir = String::from_utf8_lossy(&out.stdout);

    let fast_fn = mir_function_body(&mir, "fast_call");
    let legacy_fn = mir_function_body(&mir, "legacy_call");
    assert!(
        !fast_fn.contains("__olive_py_set_loc"),
        "fast-path function unexpectedly emitted __olive_py_set_loc:\n{fast_fn}"
    );
    assert!(
        fast_fn.contains("__olive_py_call_method0"),
        "expected the fused arity-0 entry point in the fast-path function:\n{fast_fn}"
    );
    assert!(
        legacy_fn.contains("__olive_py_set_loc"),
        "legacy-path function must still emit __olive_py_set_loc:\n{legacy_fn}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Slices the `--emit-mir` dump down to one function's own printed block, so
/// a symbol match can't accidentally cross into a different function (e.g.
/// `__main__`'s `import py` bookkeeping, which legitimately still emits
/// `__olive_py_set_loc` for the import itself).
fn mir_function_body<'a>(mir: &'a str, fn_name: &str) -> &'a str {
    let marker = format!("\"{fn_name}\"");
    let start = mir.find(&marker).unwrap_or_else(|| {
        panic!("function {fn_name:?} not found in emitted MIR:\n{mir}");
    });
    let rest = &mir[start..];
    let end = rest[1..]
        .find("MirFunction {")
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    &rest[..end]
}
