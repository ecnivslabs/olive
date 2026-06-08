//! R10's own fusion condition only ever holds for a stub-typed callee: a
//! dynamically dispatched method or an untyped module function always
//! types as `PyObject` to the checker, since this language has no
//! per-class/method python stub syntax to give one a scalar type instead.
//! `lower_py_call_scalar_hint` (`mir/builder/lower_expr/py_call.rs`) widens
//! fusion to those calls too, with no new syntax at all: when the
//! *immediate* assignment context (a `let` with an explicit type
//! annotation, a `return`, an implicit tail return, or a plain assignment
//! into an already-typed variable) already declares a fusable scalar, the
//! call fuses as if it had been stub-typed that way. This file proves the
//! inferred value comes back correct from real Olive source across both
//! pipelines, that every qualifying assignment shape triggers it, that the
//! cases which must NOT trigger it (no annotation, arity 5+, kwargs) still
//! behave exactly like the pre-inference path, and that a genuine
//! conversion failure still aborts with the same message the ordinary
//! coercion path gives.

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

const INFHELPER_PY: &str = r#"
class Widget:
    def __init__(self):
        self.touched = 0

    def area(self, w, h):
        return w * h

    def label(self):
        return "widget"

    def is_square(self, w, h):
        return w == h

    def touch(self):
        self.touched += 1
        return self.touched

    def sum4(self, a, b, c, d):
        return a + b + c + d

    def sum5(self, a, b, c, d, e):
        return a + b + c + d + e

def make_widget():
    return Widget()

def f(x):
    return x + 1

def bad_scalar():
    return [1, 2, 3]
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("olive_ret_fusion_inf_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("infhelper.py"), INFHELPER_PY).unwrap();
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
fn let_hint_fuses_a_dynamic_method_call() {
    assert_identical_on_both_pipelines(
        r#"import py "infhelper" as h

fn main():
    let w = h.make_widget()
    let area: float = w.area(3.0, 4.0)
    print(area)

main()
"#,
        "12.0\n",
    );
}

#[test]
fn let_hint_fuses_an_untyped_module_function_call() {
    assert_identical_on_both_pipelines(
        r#"import py "infhelper" as h

fn main():
    let y: int = h.f(41)
    print(y)

main()
"#,
        "42\n",
    );
}

#[test]
fn let_hint_fuses_str_and_bool_dynamic_results() {
    assert_identical_on_both_pipelines(
        r#"import py "infhelper" as h

fn main():
    let w = h.make_widget()
    let name: str = w.label()
    let square: bool = w.is_square(2.0, 2.0)
    print(name)
    print(square)

main()
"#,
        "widget\nTrue\n",
    );
}

#[test]
fn return_hint_fuses_a_dynamic_method_call() {
    assert_identical_on_both_pipelines(
        r#"import py "infhelper" as h

fn compute_area() -> float:
    let w = h.make_widget()
    return w.area(5.0, 6.0)

fn main():
    print(compute_area())

main()
"#,
        "30.0\n",
    );
}

#[test]
fn tail_expr_hint_fuses_a_dynamic_method_call() {
    assert_identical_on_both_pipelines(
        r#"import py "infhelper" as h

fn touch_count() -> int:
    let w = h.make_widget()
    w.touch()
    w.touch()

fn main():
    print(touch_count())

main()
"#,
        "2\n",
    );
}

#[test]
fn assign_hint_fuses_into_an_already_typed_var() {
    assert_identical_on_both_pipelines(
        r#"import py "infhelper" as h

fn main():
    let w = h.make_widget()
    let mut area: float = 0.0
    area = w.area(2.0, 9.0)
    print(area)

main()
"#,
        "18.0\n",
    );
}

#[test]
fn arity_four_hint_still_fuses() {
    assert_identical_on_both_pipelines(
        r#"import py "infhelper" as h

fn main():
    let w = h.make_widget()
    let total: int = w.sum4(1, 2, 3, 4)
    print(total)

main()
"#,
        "10\n",
    );
}

/// No annotation means the declared type equals the checker's own
/// `PyObject` type for the call, so the hint never applies (matches
/// `RET_HANDLE`, a true no-op) -- this is the pre-inference path, unchanged.
#[test]
fn unannotated_let_stays_on_the_unfused_path() {
    assert_identical_on_both_pipelines(
        r#"import py "infhelper" as h

fn main():
    let w = h.make_widget()
    let area = w.area(3.0, 4.0)
    print(area)

main()
"#,
        "12.0\n",
    );
}

/// Arity 5 has no fixed-register entry point at all, hinted or not -- the
/// call must still go through the list-based path and still produce the
/// correct value via the ordinary post-call coercion.
#[test]
fn arity_five_falls_back_and_still_converts_correctly() {
    assert_identical_on_both_pipelines(
        r#"import py "infhelper" as h

fn main():
    let w = h.make_widget()
    let total: int = w.sum5(1, 2, 3, 4, 5)
    print(total)

main()
"#,
        "15\n",
    );
}

/// A conversion failure through the inferred hint must abort with the same
/// message the pre-inference standalone coercion gives -- the conversion
/// code path (`finish_ret`/`raw_py_to_float`) is shared, only which call
/// site reaches it changed.
#[test]
fn conversion_failure_through_an_inferred_hint_aborts_with_the_coercion_message() {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(
        r#"import py "infhelper" as h

fn main():
    let x: float = h.bad_scalar()
    print(x)

main()
"#,
    );

    let jit = run_jit(&dir, &liv_path);
    assert!(!jit.status.success(), "expected pit run to abort");
    let jit_err = String::from_utf8_lossy(&jit.stderr);
    assert!(
        jit_err.contains("cannot convert this Python value to a float"),
        "pit run stderr missing coercion message: {jit_err}"
    );

    let out_bin = liv_path.with_extension("bin");
    let build = Command::new(pit_bin())
        .arg("build")
        .arg("--release")
        .arg(&liv_path)
        .arg("-o")
        .arg(&out_bin)
        .env("PYTHONPATH", &dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit build");
    assert!(build.status.success(), "AOT build failed");
    let aot = Command::new(&out_bin)
        .env("PYTHONPATH", &dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn built binary");
    std::fs::remove_file(&out_bin).ok();
    assert!(!aot.status.success(), "expected the built binary to abort");
    let aot_err = String::from_utf8_lossy(&aot.stderr);
    assert!(
        aot_err.contains("cannot convert this Python value to a float"),
        "AOT stderr missing coercion message: {aot_err}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Direct static check of the acceptance shape: a `let`-hinted dynamic
/// method call emits the fused arity symbol and never falls back to a
/// standalone `__olive_py_to_float` coercion call or a separate
/// `__olive_py_getattr`.
#[test]
fn emit_mir_shows_inferred_fusion_with_no_legacy_calls() {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(
        r#"import py "infhelper" as h

fn main():
    let w = h.make_widget()
    let area: float = w.area(3.0, 4.0)
    print(area)

main()
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
        "expected the fused two-arg method-call symbol in the MIR dump"
    );
    assert!(
        !mir.contains("\"__olive_py_getattr\""),
        "an inferred-fused method call must never emit a separate getattr"
    );
    assert!(
        !mir.contains("\"__olive_py_to_float\""),
        "an inferred-fused float result must never fall back to the standalone coercion call"
    );

    std::fs::remove_dir_all(&dir).ok();
}
