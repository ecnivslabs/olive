//! R19: exporting an Olive function value as a genuine Python `PyCFunction`
//! (`olive_py_make_callable`), so Python code can hold and call it like any
//! other callable -- `sorted(xs, key=f)`, `map(f, xs)`, a callback parameter
//! on an imported module. Proves real cross-into-Python invocation (not the
//! native, Python-free `sorted(xs, key=f)` builtin E5.5 already covers)
//! across both pipelines, plus the runtime's failure and reentrancy paths.

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

const HELPER_PY: &str = r#"
def apply(fn, x):
    return fn(x)

def apply_str(fn, s):
    return fn(s)

def apply_mixed(fn, s, f):
    return fn(s, f)

def sum_map(fn, n):
    return sum(map(fn, range(n)))

def reentrant_apply(fn, x):
    # Calls back into Olive while Python itself is mid-call, matching the
    # shape `sorted`/`map` use internally.
    return fn(x) + 1
"#;

fn write_case(dir_tag: &str, src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "olive_callable_export_{dir_tag}_{}_{id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cbhelper.py"), HELPER_PY).unwrap();
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

fn assert_identical_on_both_pipelines(tag: &str, src: &str, expected: &str) {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(tag, src);

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

/// `sorted(xs, key=f)` through real CPython (`builtins.sorted`), not the
/// native Python-free E5.5 builtin -- the actual R19 crossing.
#[test]
fn builtins_sorted_with_olive_key_callback() {
    assert_identical_on_both_pipelines(
        "sorted_key",
        r#"import py "builtins" as builtins

fn square(x: int) -> int:
    return x * x

fn main():
    let xs: [int] = [5, -3, 8, -1, 9, 2]
    let ys = builtins.sorted(xs, key=square)
    print(ys)

main()
"#,
        "[-1, 2, -3, 5, 8, 9]\n",
    );
}

#[test]
fn str_param_callback() {
    assert_identical_on_both_pipelines(
        "str_param",
        r#"import py "cbhelper" as h

fn shout(s: str) -> str:
    return s + "!"

fn main():
    print(h.apply_str(shout, "hello"))

main()
"#,
        "hello!\n",
    );
}

#[test]
fn float_param_and_mixed_params_callback() {
    assert_identical_on_both_pipelines(
        "float_mixed",
        r#"import py "cbhelper" as h

fn combine(s: str, f: float) -> str:
    return s + str(f)

fn main():
    print(h.apply_mixed(combine, "value=", 2.5))

main()
"#,
        "value=2.5\n",
    );
}

/// Called at scale (1e5 iterations, entirely Python-driven via `map`) --
/// the same shape the `py_callback` benchmark measures, checked here for
/// correctness rather than speed.
#[test]
fn called_at_scale_via_python_map() {
    assert_identical_on_both_pipelines(
        "at_scale",
        r#"import py "cbhelper" as h

fn increment(x: int) -> int:
    return x + 1

fn main():
    let total = h.sum_map(increment, 100000)
    print(total)

main()
"#,
        "5000050000\n",
    );
}

/// The callback itself calls back into Python (a nested `import py` call)
/// while already invoked from inside a Python call -- `GIL_DEPTH` must
/// make the inner call a no-op re-entry, not a deadlock or a double
/// `PyGILState_Ensure`.
#[test]
fn reentrant_callback_calls_back_into_python() {
    assert_identical_on_both_pipelines(
        "reentrant",
        r#"import py "cbhelper" as h
import py "math" as math

fn double_via_python(x: int) -> int:
    let r = math.sqrt(x * x * 4)
    return int(r)

fn main():
    print(h.reentrant_apply(double_via_python, 3))

main()
"#,
        "7\n",
    );
}

/// A wrong-arity call from Python raises a real `TypeError` with a useful
/// message, surfaced through Olive's ordinary uncaught-exception path --
/// not a process abort, not a silent wrong result.
#[test]
fn wrong_arity_call_from_python_raises_type_error() {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let src = r#"import py "cbhelper" as h

fn two_args(a: int, b: int) -> int:
    return a + b

fn main():
    let r = h.apply(two_args, 5)
    print(r)

main()
"#;
    let (dir, liv_path) = write_case("wrong_arity", src);
    let jit = run_jit(&dir, &liv_path);
    assert!(
        !jit.status.success(),
        "a wrong-arity callback call must fail, not succeed"
    );
    let stderr = String::from_utf8_lossy(&jit.stderr);
    assert!(
        stderr.contains("TypeError") && stderr.contains("takes 2 argument"),
        "expected a useful TypeError message, got: {stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A function whose parameter type can't cross into a Python callable
/// (E0603) is rejected at compile time, before it ever reaches a Python
/// call site.
#[test]
fn unsupported_param_type_is_a_compile_error() {
    let src = r#"import py "cbhelper" as h

fn process(xs: [int]) -> int:
    return xs[0]

fn main():
    let r = h.apply(process, 5)
    print(r)

main()
"#;
    let (dir, liv_path) = write_case("unsupported_type", src);
    let jit = run_jit(&dir, &liv_path);
    assert!(
        !jit.status.success(),
        "an unsupported callback parameter type must fail to compile"
    );
    let stderr = String::from_utf8_lossy(&jit.stderr);
    assert!(
        stderr.contains("E0603"),
        "expected an E0603 diagnostic, got: {stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A capturing closure (not just a bare top-level fn) crosses into Python
/// correctly too -- the uniform closure-record mechanism handles captures
/// for free.
#[test]
fn capturing_closure_as_callback() {
    assert_identical_on_both_pipelines(
        "capturing",
        r#"import py "cbhelper" as h

fn make_adder(n: int) -> fn(int) -> int:
    fn adder(x: int) -> int:
        return x + n
    return adder

fn main():
    let add5 = make_adder(5)
    print(h.apply(add5, 10))

main()
"#,
        "15\n",
    );
}
