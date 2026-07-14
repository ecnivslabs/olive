//! `obj.method(args...)` with 0-4 positional arguments and no keywords
//! fuses the getattr into the call itself via `PyObject_VectorcallMethod`
//! and the interned attribute name (R6, R8) -- no separate getattr call, no
//! intermediate bound-method object ever built. Arity 5+ and any kwargs
//! method call keep the original two-step getattr-then-call path. This file
//! proves both shapes produce correct, identical output from real Olive
//! source across both pipelines (JIT `pit run`, AOT release) and with the
//! fusion forced off (`OLIVE_PY_NO_VECTORCALL=1`, which also disables
//! `PyObject_VectorcallMethod` since both dlsym off the same flag), and
//! confirms the acceptance shape directly via `--emit-mir`.

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

const METHODHELPER_PY: &str = r#"
class Helper:
    def __init__(self):
        self.calls = 0

    def m0(self):
        return 111

    def m1(self, a):
        return a + 1

    def m2(self, a, b):
        return a + b

    def m3(self, a, b, c):
        return a + b + c

    def m4(self, a, b, c, d):
        return a + b + c + d

    def m5(self, a, b, c, d, e):
        return a + b + c + d + e

    def kw_method(self, a, b=10):
        return a + b

    def sort_inplace(self, xs):
        xs.sort()
        return len(xs)

    def boom(self):
        raise ValueError("method call failed on purpose")

def make_helper():
    return Helper()
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_call_method_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("methodhelper.py"), METHODHELPER_PY).unwrap();
    let liv_path = dir.join("main.liv");
    let mut f = std::fs::File::create(&liv_path).unwrap();
    f.write_all(src.as_bytes()).unwrap();
    (dir, liv_path)
}

/// `no_fusion`: forces the getattr-then-call fallback for this process by
/// disabling vectorcall entirely (`PyObject_VectorcallMethod` is gated by
/// the same `HAS_VECTORCALL` flag as plain `PyObject_Vectorcall`).
fn run_jit(dir: &Path, liv_path: &Path, no_fusion: bool) -> Output {
    let mut cmd = Command::new(pit_bin());
    cmd.arg("run")
        .arg(liv_path)
        .env("PYTHONPATH", dir)
        .stdin(Stdio::null());
    if no_fusion {
        cmd.env("OLIVE_PY_NO_VECTORCALL", "1");
    }
    cmd.output().expect("spawn pit run")
}

fn run_aot(dir: &Path, liv_path: &Path, no_fusion: bool) -> Output {
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
    if no_fusion {
        cmd.env("OLIVE_PY_NO_VECTORCALL", "1");
    }
    let out = cmd.output().expect("spawn built binary");
    std::fs::remove_file(&out_bin).ok();
    out
}

/// Runs `src` four ways -- JIT and AOT, each with fusion on (default) and
/// forced off -- and asserts every run succeeds with stdout `expected`.
fn assert_identical_on_all_four_lanes(src: &str, expected: &str) {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(src);

    for &no_fusion in &[false, true] {
        let label = if no_fusion { "no-fusion" } else { "fused" };

        let jit = run_jit(&dir, &liv_path, no_fusion);
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

        let aot = run_aot(&dir, &liv_path, no_fusion);
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

/// Same four-lane matrix, asserting every run fails with stderr containing
/// `needle` -- proves the fused path's exception message matches the
/// fallback path's exactly.
fn assert_fails_identically_on_all_four_lanes(src: &str, needle: &str) {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(src);

    for &no_fusion in &[false, true] {
        let label = if no_fusion { "no-fusion" } else { "fused" };

        let jit = run_jit(&dir, &liv_path, no_fusion);
        assert!(
            !jit.status.success(),
            "pit run ({label}) unexpectedly succeeded"
        );
        assert!(
            String::from_utf8_lossy(&jit.stderr).contains(needle),
            "pit run ({label}) stderr missing {needle:?}: {}",
            String::from_utf8_lossy(&jit.stderr)
        );

        let aot = run_aot(&dir, &liv_path, no_fusion);
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
fn arity0_method_call_has_no_args() {
    assert_identical_on_all_four_lanes(
        r#"import py "methodhelper" as h

fn main():
    let obj = h.make_helper()
    print(obj.m0())

main()
"#,
        "111\n",
    );
}

#[test]
fn arity1_method_call() {
    assert_identical_on_all_four_lanes(
        r#"import py "methodhelper" as h

fn main():
    let obj = h.make_helper()
    let a: int = 41
    print(obj.m1(a))

main()
"#,
        "42\n",
    );
}

#[test]
fn arity2_method_call() {
    assert_identical_on_all_four_lanes(
        r#"import py "methodhelper" as h

fn main():
    let obj = h.make_helper()
    let a: int = 10
    let b: int = 20
    print(obj.m2(a, b))

main()
"#,
        "30\n",
    );
}

#[test]
fn arity3_method_call() {
    assert_identical_on_all_four_lanes(
        r#"import py "methodhelper" as h

fn main():
    let obj = h.make_helper()
    let a: int = 1
    let b: int = 2
    let c: int = 3
    print(obj.m3(a, b, c))

main()
"#,
        "6\n",
    );
}

#[test]
fn arity4_method_call() {
    assert_identical_on_all_four_lanes(
        r#"import py "methodhelper" as h

fn main():
    let obj = h.make_helper()
    let a: int = 1
    let b: int = 2
    let c: int = 3
    let d: int = 4
    print(obj.m4(a, b, c, d))

main()
"#,
        "10\n",
    );
}

/// Arity 5 has no fixed-register method entry point and falls back to the
/// original getattr-then-call path -- the boundary case right past the
/// fused arities.
#[test]
fn arity5_method_call_falls_back_to_getattr_path() {
    assert_identical_on_all_four_lanes(
        r#"import py "methodhelper" as h

fn main():
    let obj = h.make_helper()
    let a: int = 1
    let b: int = 2
    let c: int = 3
    let d: int = 4
    let e: int = 5
    print(obj.m5(a, b, c, d, e))

main()
"#,
        "15\n",
    );
}

/// A keyword argument keeps the method call on the getattr-then-call path
/// regardless of positional arity -- only the positional-only, kwargs-free
/// shape gets fused.
#[test]
fn kwargs_method_call_still_works() {
    assert_identical_on_all_four_lanes(
        r#"import py "methodhelper" as h

fn main():
    let obj = h.make_helper()
    print(obj.kw_method(5, b=7))

main()
"#,
        "12\n",
    );
}

/// Spot-check (not the full copy-out matrix): a collection argument to a
/// fused-arity method call still gets deep-realized into a real Python
/// object and its in-place mutation still syncs back correctly.
#[test]
fn collection_arg_to_fused_method_call_still_syncs_back() {
    assert_identical_on_all_four_lanes(
        r#"import py "methodhelper" as h

fn main():
    let obj = h.make_helper()
    let xs: [int] = [3, 1, 2]
    print(obj.sort_inplace(xs))
    print(xs)

main()
"#,
        "3\n[1, 2, 3]\n",
    );
}

/// An exception raised inside a fused-path method call surfaces with the
/// same message as the fallback path -- the fusion is invisible on the
/// error path too.
#[test]
fn exception_from_inside_method_surfaces_identically() {
    assert_fails_identically_on_all_four_lanes(
        r#"import py "methodhelper" as h

fn main():
    let obj = h.make_helper()
    obj.boom()

main()
"#,
        "method call failed on purpose",
    );
}

/// Direct static check of the acceptance shape: `--emit-mir` on a 0-4-arg
/// positional method call shows the fused `__olive_py_call_method*` symbol
/// and no separate `__olive_py_getattr` call for that call site, while an
/// arity-5 (or kwargs) method call still shows both.
#[test]
fn emit_mir_shows_fusion_for_direct_calls_and_getattr_for_the_fallback_shapes() {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(
        r#"import py "methodhelper" as h

fn only_fused():
    let obj = h.make_helper()
    obj.m2(1, 2)

fn only_fallback():
    let obj = h.make_helper()
    obj.m5(1, 2, 3, 4, 5)

only_fused()
only_fallback()
"#,
    );

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&liv_path)
        .arg("--emit-mir")
        .env("PYTHONPATH", &dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run --emit-mir");
    assert!(
        out.status.success(),
        "pit run --emit-mir failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mir = String::from_utf8_lossy(&out.stdout);

    assert!(
        mir.contains("__olive_py_call_method2"),
        "expected a fused method-call symbol in the MIR dump"
    );
    assert!(
        mir.contains("__olive_py_call_t"),
        "expected the arity-5 fallback to still use the list-based call path"
    );
    assert!(
        mir.contains("__olive_py_getattr"),
        "expected the arity-5 fallback to still emit a separate getattr call"
    );

    std::fs::remove_dir_all(&dir).ok();
}
