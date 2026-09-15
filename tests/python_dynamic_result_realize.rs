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
    for cmd in &["python3", "python"] {
        if Command::new(cmd)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return true;
        }
    }
    false
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

const DRHELPER_PY: &str = r#"
def make_bytes():
    return b"hello"

def make_list():
    return [1, 2, 3]

def make_dict():
    return {"a": 1, "b": 2}

def make_set():
    return {1, 2, 3}

def make_f32_list():
    return [1.5, 2.5]

def make_none_list():
    return [None, None]

def make_u64_list():
    return [1 << 63]

def make_f32_set():
    return {1.5, 2.5}

def make_none_set():
    return {None}

def make_u64_set():
    return {1 << 63}

def make_u64_dict():
    return {1 << 63: 7}

def make_empty_dict():
    return {}

def echo(value):
    return value

def kind(value):
    return type(value).__name__

def make_int_dict():
    return {2: 10, 3: 30}

def make_nested_f32_matrix():
    return [[1.5]]

def make_generated_nested_f32_matrix():
    yield [1.5]

def make_nested_u64_matrix():
    return [[1 << 63]]

def make_nested_none_matrix():
    return [[None]]

def make_nested_f32_dict():
    return {"x": [1.5]}

def make_nested_none_dict():
    return {1: [None]}

def make_float_set():
    return {2.5}

def make_tuple_set():
    return {(1.5, 1 << 63)}

def make_typed_dicts():
    return {1.5: 2.5}

def make_mixed_keys():
    return {2: 10, 1.5: 20, True: 30, None: 40, b"x": 50}

def key_types(d):
    return [type(k).__name__ for k in d]

def sorted_key_types(d):
    return sorted(key_types(d))

def make_bad_iter():
    yield 1
    raise ValueError("boom")

class BadKey:
    def __hash__(self):
        return 1

    def __str__(self):
        raise ValueError("boom")

class StringKey:
    def __str__(self):
        return "k"

def make_bad_dict():
    return {BadKey(): 7}

def make_string_key_dict():
    return {StringKey(): 7}

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
fn bytes_result_survives_nested_async_tasks() {
    assert_both_succeed(
        r#"import py "drhelper" as h

async fn make() -> bytes:
    let data: bytes = h.make_bytes()
    return data

async fn forward() -> bytes:
    let data = await make()
    return data

fn main():
    let data = await forward()
    print(len(data))
    print(data[0])
    print(data[4])

main()
"#,
        "5\n104\n111\n",
    );
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

fn assert_both_fail(src: &str, needle: &str) {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(src);

    let jit = run_jit(&dir, &liv_path);
    assert!(!jit.status.success(), "pit run unexpectedly succeeded");
    assert!(
        String::from_utf8_lossy(&jit.stderr).contains(needle),
        "pit run stderr missing {needle:?}: {}",
        String::from_utf8_lossy(&jit.stderr)
    );

    let aot = run_aot(&dir, &liv_path);
    assert!(!aot.status.success(), "AOT unexpectedly succeeded");
    assert!(
        String::from_utf8_lossy(&aot.stderr).contains(needle),
        "AOT stderr missing {needle:?}: {}",
        String::from_utf8_lossy(&aot.stderr)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn failed_iterable_conversion_propagates_python_error() {
    assert_both_fail(
        r#"import py "builtins" as b

fn main():
    let xs: [int] = b.map(b.int, ["1", "not-an-int"])
    print(xs)

main()
"#,
        "ValueError",
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
fn imported_scalar_collections_preserve_widths_and_none() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let fs: [f32] = h.make_f32_list()
    print(fs)
    let ns: [None] = h.make_none_list()
    print(ns)
    let us: [u64] = h.make_u64_list()
    print(us)

main()
"#,
        "[1.5, 2.5]\n[None, None]\n[9223372036854775808]\n",
    );
}

#[test]
fn imported_unsigned_dict_keys_use_unsigned_hash_bits() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let ints: {int: int} = h.make_int_dict()
    print(ints[2])
    let one: u64 = 1
    let high: u64 = one << 63
    let d: {u64: int} = h.make_u64_dict()
    print(high in d)
    print(d[high])

main()
"#,
        "10\nTrue\n7\n",
    );
}

#[test]
fn high_u64_survives_python_object_coercion() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let one: u64 = 1
    let high: u64 = one << 63
    let p: PyObject = high
    print(h.kind(p))
    let n: u64 = h.echo(p)
    print(n)

main()
"#,
        "int\n9223372036854775808\n",
    );
}

#[test]
fn dynamic_any_dict_preserves_unsigned_key_identity() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let one: u64 = 1
    let high: u64 = one << 63
    let d: Any = h.make_u64_dict()
    print(d[high])

main()
"#,
        "7\n",
    );
}

#[test]
fn python_object_subscript_preserves_unsigned_key_width() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let one: u64 = 1
    let high: u64 = one << 63
    let d: PyObject = h.make_empty_dict()
    d[high] = 42
    print(d[high])

main()
"#,
        "42\n",
    );
}

#[test]
fn imported_scalar_sets_preserve_widths_and_none() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let fs: set[f32] = h.make_f32_set()
    print(1.5 in fs)
    let ns: set[None] = h.make_none_set()
    print(None in ns)
    let one: u64 = 1
    let high: u64 = one << 63
    let us: set[u64] = h.make_u64_set()
    print(high in us)

main()
"#,
        "True\nTrue\nTrue\n",
    );
}

#[test]
fn dynamic_module_call_typed_set_result_realizes_correctly() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let s: set[int] = h.make_set()
    print(2 in s)

main()
"#,
        "True\n",
    );
}

#[test]
fn failed_set_iterable_conversion_propagates_python_error() {
    assert_both_fail(
        r#"import py "drhelper" as h

fn main():
    let s: set[int] = h.make_bad_iter()
    print(s)

main()
"#,
        "ValueError",
    );
}

#[test]
fn failed_dict_key_conversion_propagates_python_error() {
    assert_both_fail(
        r#"import py "drhelper" as h

fn main():
    let d: {str: int} = h.make_bad_dict()
    print(d)

main()
"#,
        "ValueError",
    );
}

#[test]
fn custom_string_key_import_uses_python_string_conversion() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let d: {str: int} = h.make_string_key_dict()
    print(d["k"])

main()
"#,
        "7\n",
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
fn imported_dict_values_and_keys_preserve_declared_types() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let d: {float: f32} = h.make_typed_dicts()
    print(d[1.5])
    let m: {Any: int} = h.make_mixed_keys()
    print(h.sorted_key_types(m))

main()
"#,
        "2.5\n['NoneType', 'bool', 'bytes', 'float', 'int']\n",
    );
}

#[test]
fn imported_nested_collections_use_full_descriptors() {
    assert_both_succeed(
        r#"import py "drhelper" as h

fn main():
    let one: u64 = 1
    let high: u64 = one << 63
    let fs: [[f32]] = h.make_nested_f32_matrix()
    print(fs[0][0])
    let generated: [[f32]] = h.make_generated_nested_f32_matrix()
    print(generated[0][0])
    let us: [[u64]] = h.make_nested_u64_matrix()
    print(high in us[0])
    let ns: [[None]] = h.make_nested_none_matrix()
    print(None in ns[0])
    let fd: {str: [f32]} = h.make_nested_f32_dict()
    print(fd["x"][0])
    let nd: {int: [None]} = h.make_nested_none_dict()
    print(None in nd[1])
    let fs2: set[float] = h.make_float_set()
    print(2.5 in fs2)
    let tuples: set[(f32, u64)] = h.make_tuple_set()
    print(tuples)

main()
"#,
        "1.5\n1.5\nTrue\nTrue\n1.5\nTrue\nTrue\n{(1.5, 9223372036854775808)}\n",
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
