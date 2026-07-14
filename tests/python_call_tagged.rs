//! A py-call argument with a statically known scalar type crosses the
//! boundary via the tagged fast path (`__olive_py_call_t`/`_kw_t`, their
//! arity-specialized `__olive_py_call0..4` siblings, and their `_safe`
//! twins) -- one C-API call per scalar, no pre-conversion, no pre-call
//! handle allocation. This file proves the tag scheme decodes every case
//! correctly, including two cases the old raw-word heuristic (`olive_to_py`'s
//! `looks_like_float`) could not: `bool` vs `int`, and a bare `None` vs
//! integer `0`. Both pipelines (JIT `pit run`, AOT release).

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

def repr_of(x):
    return repr(x)

def identity(x):
    return x
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_tagcall_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("taghelper.py"), TAGHELPER_PY).unwrap();
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
        "jit stderr: {}",
        String::from_utf8_lossy(&jit.stderr)
    );

    let aot = run_aot(&dir, &liv_path);
    assert!(
        aot.status.success(),
        "AOT binary failed: {}",
        String::from_utf8_lossy(&aot.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&aot.stdout),
        expected,
        "aot stderr: {}",
        String::from_utf8_lossy(&aot.stderr)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn int_arg_crosses_as_python_int() {
    assert_both_succeed(
        r#"import py "taghelper" as h

fn main():
    let x: int = 42
    print(h.type_name(x))
    print(h.identity(x))

main()
"#,
        "int\n42\n",
    );
}

#[test]
fn float_arg_crosses_as_python_float() {
    assert_both_succeed(
        r#"import py "taghelper" as h

fn main():
    let x: float = 3.5
    print(h.type_name(x))
    print(h.identity(x))

main()
"#,
        "float\n3.5\n",
    );
}

#[test]
fn bool_arg_crosses_as_python_bool_not_int() {
    // `bool` and `int` are bit-identical raw words (0/1); only a static tag
    // tells the runtime which Python type to build.
    assert_both_succeed(
        r#"import py "taghelper" as h

fn main():
    let x: bool = True
    print(h.type_name(x))
    let y: bool = False
    print(h.type_name(y))

main()
"#,
        "bool\nbool\n",
    );
}

#[test]
fn none_arg_crosses_as_python_none_not_zero() {
    // A bare `None` argument's raw representation is the sentinel `0`,
    // bit-identical to integer zero; without a dedicated tag this silently
    // crossed as Python `0` instead of `None`.
    assert_both_succeed(
        r#"import py "taghelper" as h

fn main():
    print(h.type_name(None))
    print(h.repr_of(None))

main()
"#,
        "NoneType\nNone\n",
    );
}

#[test]
fn str_arg_crosses_correctly() {
    assert_both_succeed(
        r#"import py "taghelper" as h

fn main():
    let s: str = "hello"
    print(h.type_name(s))
    print(h.identity(s))

main()
"#,
        "str\nhello\n",
    );
}

#[test]
fn any_typed_int_and_none_cross_with_correct_dynamic_type() {
    // An `Any`-typed argument decodes via the inline-tag-aware path
    // (`olive_any_to_py`), not the raw heuristic. (Not `int|None`: that
    // union uses a raw-sentinel representation with its own pre-existing,
    // separately tracked None/0 ambiguity, unrelated to this tag scheme.)
    assert_both_succeed(
        r#"import py "taghelper" as h

fn f(x: Any):
    print(h.type_name(x))

fn main():
    f(7)
    f(None)

main()
"#,
        "int\nNoneType\n",
    );
}

#[test]
fn pyobject_arg_forwards_without_reconversion() {
    // The result of one py-call (already a live PyObject handle) passed as
    // an argument to another: tag 0, unwrap + incref, no reconversion.
    assert_both_succeed(
        r#"import py "taghelper" as h

fn main():
    let obj = h.identity(123)
    print(h.type_name(obj))

main()
"#,
        "int\n",
    );
}

#[test]
fn bytes_arg_crosses_correctly() {
    // Olive has no bytes literal; realize one from Python, then pass it back
    // to prove the ARG_BYTES tag's own decode path (not the collection path).
    assert_both_succeed(
        r#"import py "taghelper" as h

fn make() -> bytes:
    let obj = h.identity("hi")
    return obj.encode()

fn main():
    let b: bytes = make()
    print(h.type_name(b))

main()
"#,
        "bytes\n",
    );
}

#[test]
fn kwargs_mix_of_tag_types_all_correct() {
    assert_both_succeed(
        r#"import py "taghelper" as h

fn main():
    print(h.type_name(x=None))
    print(h.type_name(x=True))
    print(h.type_name(x=9))

main()
"#,
        "NoneType\nbool\nint\n",
    );
}

#[test]
fn fast_path_emits_no_legacy_conversion_calls() {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(
        r#"import py "taghelper" as h

fn main():
    let x: int = 1
    let y: float = 2.0
    print(h.identity(x))
    print(h.identity(y))

main()
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
    assert!(
        !mir.contains("__olive_py_from_int") && !mir.contains("__olive_py_from_float"),
        "fast-path scalar args must not go through the legacy per-arg conversion calls:\n{mir}"
    );
    assert!(
        mir.contains("__olive_py_call1"),
        "expected the tagged fast-path entry point in the emitted MIR:\n{mir}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn seventeen_scalar_args_use_legacy_fallback() {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let names = (0..17).map(|i| format!("a{i}")).collect::<Vec<_>>();
    let py = format!(
        "def sum17({params}):\n    return {sum}\n",
        params = names.join(", "),
        sum = names.join(" + "),
    );
    let src = format!(
        "import py \"sum17helper\" as h\n\nfn main():\n    {decls}\n    print(h.sum17({args}))\n\nmain()\n",
        decls = names
            .iter()
            .enumerate()
            .map(|(i, n)| format!("let {n}: int = {i}"))
            .collect::<Vec<_>>()
            .join("\n    "),
        args = names.join(", "),
    );

    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_tagcall_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sum17helper.py"), py).unwrap();
    let liv_path = dir.join("main.liv");
    std::fs::write(&liv_path, &src).unwrap();

    let out = Command::new(pit_bin())
        .arg("run")
        .arg("--emit-mir")
        .arg(&liv_path)
        .env("PYTHONPATH", &dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run --emit-mir");
    let mir = String::from_utf8_lossy(&out.stdout);
    // `"__olive_py_call"` (quoted, with its closing quote) picks out the
    // exact legacy symbol without also matching the `_t`/`_safe`/`_kw`
    // variants, all of which share it as a string prefix.
    assert!(
        mir.contains("\"__olive_py_call\"") && !mir.contains("__olive_py_call_t"),
        "17 args must fall back to the legacy entry point, not the 16-arg tagged fast path:\n{mir}"
    );

    let run = Command::new(pit_bin())
        .arg("run")
        .arg(&liv_path)
        .env("PYTHONPATH", &dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "136\n");

    std::fs::remove_dir_all(&dir).ok();
}
