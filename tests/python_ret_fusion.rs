//! When a call's declared result is a scalar (`int`/`float`/`str`/`bool`) or
//! `None`, the arity-specialized `__olive_py_call{0..4}`/`__olive_py_call_method{0..4}`
//! entry points convert the result inside the call itself, under the same
//! GIL, instead of wrapping a handle and paying a second boundary crossing
//! (`__olive_py_to_int`/`_float`/`_str`, another GIL pair, then a decref) to
//! unwrap it back out. This file proves the fused value comes back correct
//! from real Olive source across both pipelines (JIT `pit run`, AOT
//! release), that a statement-position call whose result is unused runs
//! cleanly many times, that a genuine conversion failure still aborts with
//! the same message the old standalone coercion gave, and confirms the
//! acceptance shape directly via `--emit-mir`.

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

const RETHELPER_PY: &str = r#"
def sq(x):
    return x * x

def pi():
    return 3.5

def greet(name):
    return "hi " + name

def is_even(x):
    return x % 2 == 0

def add4(a, b, c, d):
    return a + b + c + d

def bad_int():
    return [1, 2, 3]

def sort_and_len(xs):
    xs.sort()
    return len(xs)

class Widget:
    def __init__(self):
        self.touched = 0

    def area(self, w, h):
        return w * h

    def touch(self):
        self.touched += 1

def make_widget():
    return Widget()
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_ret_fusion_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rethelper.py"), RETHELPER_PY).unwrap();
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
fn stub_typed_int_result_fuses() {
    assert_identical_on_both_pipelines(
        r#"import py "rethelper" as h:
    fn sq(x: int) -> int

fn main():
    print(h.sq(7))

main()
"#,
        "49\n",
    );
}

#[test]
fn stub_typed_float_result_fuses_with_zero_arg_call() {
    assert_identical_on_both_pipelines(
        r#"import py "rethelper" as h:
    fn pi() -> float

fn main():
    print(h.pi())

main()
"#,
        "3.5\n",
    );
}

#[test]
fn stub_typed_str_result_fuses() {
    assert_identical_on_both_pipelines(
        r#"import py "rethelper" as h:
    fn greet(name: str) -> str

fn main():
    print(h.greet("olive"))

main()
"#,
        "hi olive\n",
    );
}

#[test]
fn stub_typed_bool_result_fuses() {
    assert_identical_on_both_pipelines(
        r#"import py "rethelper" as h:
    fn is_even(x: int) -> bool

fn main():
    print(h.is_even(4))
    print(h.is_even(7))

main()
"#,
        "True\nFalse\n",
    );
}

#[test]
fn stub_typed_arity4_result_fuses() {
    assert_identical_on_both_pipelines(
        r#"import py "rethelper" as h:
    fn add4(a: int, b: int, c: int, d: int) -> int

fn main():
    print(h.add4(1, 2, 3, 4))

main()
"#,
        "10\n",
    );
}

/// This language has no per-class/method python stub syntax, so a method
/// call's own checker-assigned type is always `PyObject`, whatever the
/// surrounding `let` declares -- `emit_py_method_call` never sees a scalar
/// `result_ty` for one and never fuses it. `area`'s value still comes out
/// right through the untouched pre-R10 wrap-then-cast-at-assignment path;
/// this only guards that path still works, not a fusion claim.
#[test]
fn method_call_result_still_converts_correctly_with_no_stub_to_fuse_from() {
    assert_identical_on_both_pipelines(
        r#"import py "rethelper" as h

fn main():
    let w = h.make_widget()
    let area: float = w.area(3.0, 4.0)
    print(area)

main()
"#,
        "12.0\n",
    );
}

/// A collection argument at a fused call must still copy out, exactly as it
/// did before the result side fused.
#[test]
fn collection_arg_still_syncs_back_alongside_a_fused_result() {
    assert_identical_on_both_pipelines(
        r#"import py "rethelper" as h:
    fn sort_and_len(xs: [int]) -> int

fn main():
    let xs: [int] = [3, 1, 2]
    print(h.sort_and_len(xs))
    print(xs)

main()
"#,
        "3\n[1, 2, 3]\n",
    );
}

/// A statement-position call whose result is never read (no stub, no
/// binding: the checker's own type stays `PyObject`) forces `RET_NONE`
/// regardless, so it never builds a handle at all. Looping many times must
/// neither crash nor hang.
#[test]
fn discarded_statement_position_call_runs_cleanly_many_times() {
    assert_identical_on_both_pipelines(
        r#"import py "rethelper" as h

fn main():
    let w = h.make_widget()
    let mut i = 0
    while i < 20000:
        w.touch()
        i = i + 1
    print("done")

main()
"#,
        "done\n",
    );
}

/// A stub-typed call whose Python function actually returns a value that
/// can't convert must abort with the same message the pre-R10 standalone
/// `__olive_py_to_int` coercion gave -- the conversion code path is shared,
/// only where it runs (inside the call vs. after) changed.
#[test]
fn conversion_failure_on_a_fused_int_result_aborts_with_the_coercion_message() {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(
        r#"import py "rethelper" as h:
    fn bad_int() -> int

fn main():
    print(h.bad_int())

main()
"#,
    );

    let jit = run_jit(&dir, &liv_path);
    assert!(!jit.status.success(), "expected pit run to abort");
    let jit_err = String::from_utf8_lossy(&jit.stderr);
    assert!(
        jit_err.contains("cannot convert this Python value to an integer"),
        "pit run stderr missing coercion message: {jit_err}"
    );

    let aot = run_aot_expect_failure(&dir, &liv_path);
    let aot_err = String::from_utf8_lossy(&aot.stderr);
    assert!(
        aot_err.contains("cannot convert this Python value to an integer"),
        "AOT stderr missing coercion message: {aot_err}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

fn run_aot_expect_failure(dir: &Path, liv_path: &Path) -> Output {
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
    assert!(!out.status.success(), "expected the built binary to abort");
    out
}

/// Direct static check of the acceptance shape: `--emit-mir` on stub-typed
/// scalar calls shows the fused arity symbol and never falls back to a
/// standalone `__olive_py_to_int`/`_float`/`_str` coercion call for them.
#[test]
fn emit_mir_shows_fusion_with_no_standalone_coercion_calls() {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(
        r#"import py "rethelper" as h:
    fn sq(x: int) -> int
    fn pi() -> float
    fn greet(name: str) -> str
    fn is_even(x: int) -> bool

fn main():
    print(h.sq(7))
    print(h.pi())
    print(h.greet("olive"))
    print(h.is_even(4))

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
        mir.contains("__olive_py_call_method0"),
        "expected the fused zero-arg call symbol in the MIR dump"
    );
    assert!(
        mir.contains("__olive_py_call_method1"),
        "expected the fused one-arg call symbol in the MIR dump"
    );
    assert!(
        !mir.contains("\"__olive_py_to_int\""),
        "a fused int result must never fall back to the standalone coercion call"
    );
    assert!(
        !mir.contains("\"__olive_py_to_float\""),
        "a fused float result must never fall back to the standalone coercion call"
    );
    assert!(
        !mir.contains("\"__olive_py_to_str\""),
        "a fused str result must never fall back to the standalone coercion call"
    );

    std::fs::remove_dir_all(&dir).ok();
}
