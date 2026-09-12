//! R2b: a collection argument passed into a Python call is synced back into
//! the same Olive allocation after the call returns, on both the success and
//! the exception path, so `xs.sort()`, `random.shuffle(xs)`, `d.update(...)`
//! called from Olive on an Olive collection behave exactly like the
//! equivalent Python code. Both pipelines (JIT `pit run`, AOT release).

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

/// A Python module every test can import functions from: real Python
/// mutating methods on the Olive collection passed as an argument. Native
/// Olive `.append`/`.sort`/`.update`/etc dispatch to Olive's own methods, not
/// Python's, so a helper function is the only way to exercise Python-side
/// mutation of a passed-in value.
const WBHELPER_PY: &str = r#"
def just_append(xs, v):
    xs.append(v)

def just_pop(xs):
    return xs.pop()

def do_sort(xs):
    xs.sort()

def do_update(d, extra):
    d.update(extra)

def colliding_key_update(d):
    d[1] = "b"

def colliding_float_update(d):
    d["1"] = 1.0000000000000007
    d[1] = 2.5

def mutate_then_raise(xs):
    xs.append(999)
    raise ValueError("boom")

def push_wrong_type(xs, v):
    xs[0] = v

def same_list_twice(a, b):
    a.append(1)
    return len(b)

def touch_nested(outer):
    outer[0].append(42)

def flip_set(s):
    s.add(999)
    s.discard(1)

def take_dict(d):
    return len(d)

def key_type(d):
    return type(next(iter(d))).__name__

def key_type_kw(*, d):
    return type(next(iter(d))).__name__

def set_key(d, k):
    d[k] = 9

def set_key_after(a, b, c, d, e, target, key):
    target[key] = 9

def first_item(xs):
    return xs[0]

def first_set_item(s):
    return next(iter(s))

def first_dict_value(d):
    return next(iter(d.values()))

def contains(s, value):
    return value in s

def set_first(xs, value):
    xs[0] = value

def set_value(s, value):
    s.add(value)

def first_type(xs):
    return type(xs[0]).__name__

def first_value_type(d):
    return type(next(iter(d.values()))).__name__

def inspect_dict(d):
    return len(d)

def identity(x):
    return x

def nested_value(d):
    return d["inner"]

def inspect_legacy(*args):
    d = args[-1]
    return [(type(k).__name__, k, type(v).__name__, v) for k, v in d.items()]

def any_dict_items(d):
    return [(type(k).__name__, k, type(v).__name__) for k, v in d.items()]

def make_custom_key():
    class Key:
        pass
    return Key()

def put_custom_key(d, key):
    d[key] = 1

def make_big_key():
    return 1 << 100

def put_big_key(d):
    d[1 << 100] = 1

def put_high_u64_key(d):
    d[1 << 63] = 7

def first_key(d):
    return next(iter(d))

def inspect_tuple_key(d):
    return [(type(k).__name__, k) for k in d]

def add_int_to_set(s, v):
    s.add(v)

def print_set(s):
    print(sorted(s))

probe_deleted = False

class Probe:
    def __del__(self):
        global probe_deleted
        probe_deleted = True

def make_probe():
    return Probe()

def consume_probe(x):
    return 1

def bad_probe(xs):
    xs[0] = object()
    return Probe()

def probe_was_deleted():
    return probe_deleted

def consume5(a, b, c, d, xs):
    return xs

class MutatingKey:
    def __str__(self):
        self.d.clear()
        self.d[7] = 11
        return "k"

def make_mutating_key():
    return MutatingKey()

def put_mutating_key(d, key):
    key.d = d
    d[key] = 1

import gc
import weakref
any_key_refs = []

def replace_any_key(d):
    d.clear()
    class Key:
        pass
    key = Key()
    any_key_refs.append(weakref.ref(key))
    d[key] = 1

def live_any_keys():
    gc.collect()
    return sum(ref() is not None for ref in any_key_refs)

def append_first17(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15, a16):
    a0.append(999)

def kw17(d, **kw):
    d[2] = 9
"#;

/// Writes `wbhelper.py` and a `main.liv` (source `src`) into a fresh temp
/// directory, returning the directory and the `.liv` path.
fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_writeback_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("wbhelper.py"), WBHELPER_PY).unwrap();
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

/// Runs `src` under both pipelines and asserts each succeeds with stdout
/// exactly `expected`.
fn assert_both_succeed(src: &str, expected: &str) {
    assert_both_succeed_with(src, |stdout, pipeline, stderr| {
        assert_eq!(
            stdout.replace("\r\n", "\n"),
            expected.replace("\r\n", "\n"),
            "{pipeline} stderr: {stderr}"
        );
    });
}

/// Runs `src` under both pipelines and hands each stdout to `check`. Use this
/// over `assert_both_succeed` when the output embeds a printed dict, whose
/// entry order follows the hash and is not a property worth pinning.
fn assert_both_succeed_with(src: &str, check: impl Fn(&str, &str, &str)) {
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
    check(
        &String::from_utf8_lossy(&jit.stdout),
        "jit",
        &String::from_utf8_lossy(&jit.stderr),
    );

    let aot = run_aot(&dir, &liv_path);
    assert!(
        aot.status.success(),
        "AOT binary failed: {}",
        String::from_utf8_lossy(&aot.stderr)
    );
    check(
        &String::from_utf8_lossy(&aot.stdout),
        "aot",
        &String::from_utf8_lossy(&aot.stderr),
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Runs `src` under both pipelines and asserts each fails with stderr
/// containing `needle`.
fn assert_both_fail_with(src: &str, needle: &str) {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(src);

    let jit = run_jit(&dir, &liv_path);
    assert!(!jit.status.success(), "pit run unexpectedly succeeded");
    assert!(
        String::from_utf8_lossy(&jit.stderr).contains(needle),
        "jit stderr missing {needle:?}: {}",
        String::from_utf8_lossy(&jit.stderr)
    );

    let aot = run_aot(&dir, &liv_path);
    assert!(!aot.status.success(), "AOT binary unexpectedly succeeded");
    assert!(
        String::from_utf8_lossy(&aot.stderr).contains(needle),
        "aot stderr missing {needle:?}: {}",
        String::from_utf8_lossy(&aot.stderr)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn shuffle_reorders_the_olive_list_in_place() {
    assert_both_succeed(
        r#"import py "random" as random

fn main():
    let mut xs: [int] = [1, 2, 3, 4, 5]
    random.seed(0)
    random.shuffle(xs)
    print(xs)

main()
"#,
        "[3, 2, 1, 5, 4]\n",
    );
}

#[test]
fn python_sort_mutates_the_passed_list() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut xs: [int] = [5, 3, 4, 1, 2]
    h.do_sort(xs)
    print(xs)

main()
"#,
        "[1, 2, 3, 4, 5]\n",
    );
}

#[test]
fn dict_update_mutates_the_passed_dict() {
    assert_both_succeed_with(
        r#"import py "wbhelper" as h

fn main():
    let mut d: {str: int} = {"a": 1, "b": 2}
    let extra: {str: int} = {"c": 3}
    h.do_update(d, extra)
    print(d)

main()
"#,
        |stdout, pipeline, stderr| {
            let printed = stdout.trim_end();
            for entry in ["\"a\": 1", "\"b\": 2", "\"c\": 3"] {
                assert!(
                    printed.contains(entry),
                    "{pipeline}: {printed:?} missing {entry}, stderr: {stderr}"
                );
            }
            assert_eq!(printed.matches(": ").count(), 3, "{pipeline}: {printed:?}");
        },
    );
}

#[test]
fn append_and_pop_grow_and_shrink_the_passed_list() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut xs: [int] = [1, 2, 3]
    h.just_append(xs, 42)
    print(xs)
    let popped = h.just_pop(xs)
    print(xs)
    print(popped)

main()
"#,
        "[1, 2, 3, 42]\n[1, 2, 3]\n42\n",
    );
}

#[test]
fn exception_path_still_syncs_the_partial_mutation() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn call_it(xs: [int]) -> int | Error:
    try h.mutate_then_raise(xs)
    return 0

fn main():
    let mut xs: [int] = [1, 2, 3]
    match call_it(xs):
        Error(_):
            print("caught")
        n:
            print(n)
    print(xs)

main()
"#,
        "caught\n[1, 2, 3, 999]\n",
    );
}

#[test]
fn same_list_passed_twice_aliases_one_python_object() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut xs: [int] = [1, 2, 3]
    let n = h.same_list_twice(xs, xs)
    print(n)
    print(xs)

main()
"#,
        "4\n[1, 2, 3, 1]\n",
    );
}

#[test]
fn dict_writeback_with_colliding_keys_keeps_last_value() {
    assert_both_succeed_with(
        r#"import py "wbhelper" as h

fn main():
    let mut d: {str: str} = {"1": "a"}
    h.colliding_key_update(d)
    print(d)

main()
"#,
        |stdout, pipeline, stderr| {
            let printed = stdout.trim_end();
            assert!(
                printed.contains("\"1\": \"b\""),
                "{pipeline}: {printed:?} missing colliding entry, stderr: {stderr}"
            );
            assert_eq!(printed.matches(": ").count(), 1, "{pipeline}: {printed:?}");
        },
    );
}

#[test]
fn colliding_float_dict_keys_do_not_free_raw_bits_as_strings() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut d: {str: float} = {"1": 0.5}
    h.colliding_float_update(d)
    print(d["1"])

main()
"#,
        "2.5\n",
    );
}

#[test]
fn wrong_element_type_faults_with_e0714() {
    assert_both_fail_with(
        r#"import py "wbhelper" as h

fn main():
    let mut xs: [int] = [1, 2, 3]
    h.push_wrong_type(xs, "oops")
    print(xs)

main()
"#,
        "E0714",
    );
}

#[test]
fn any_typed_nested_list_syncs_the_inner_mutation() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut outer: [Any] = [[1, 2], [3]]
    h.touch_nested(outer)
    print(outer)

main()
"#,
        "[[1, 2, 42], [3]]\n",
    );
}

#[test]
fn any_python_subscript_keys_cross_as_python_values() {
    assert_both_succeed(
        r#"import py "builtins" as b

fn main():
    let d: PyObject = b.dict()
    let none: PyObject = None
    print(none)
    d[None] = 4
    d[0] = 5
    print(d[None])
    print(d[0])
    let ki: Any = 5
    let kf: Any = 1.5
    let kb: Any = True
    let kn: Any = None
    let ks: Any = "s"
    d[ki] = 1
    d[kf] = 2
    d[kb] = 3
    d[kn] = 4
    d[ks] = 5
    print(d[ki])
    print(d[kf])
    print(d[kb])
    print(d[kn])
    print(d[ks])

main()
"#,
        "None\n4\n5\n1\n2\n3\n4\n5\n",
    );
}

/// A concretely nested list (`[[int]]`, not `[Any]`) has no flat 4-bit tag
/// that can express "the inner element is itself a raw int list" (see
/// `py_collection_tag`'s doc comment), so this argument gets no copy-out at
/// all -- the same honest, documented limitation as R2a's pre-writeback
/// behavior, not silent corruption.
#[test]
fn concretely_typed_nested_list_is_a_safe_no_op() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut outer: [[int]] = [[1, 2], [3]]
    h.touch_nested(outer)
    print(outer)

main()
"#,
        "[[1, 2], [3]]\n",
    );
}

#[test]
fn empty_list_and_dict_arguments_are_handled() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut xs: [int] = []
    h.just_append(xs, 7)
    print(xs)
    let mut d: {str: int} = {}
    let extra: {str: int} = {"z": 9}
    h.do_update(d, extra)
    print(d)

main()
"#,
        "[7]\n{\"z\": 9}\n",
    );
}

#[test]
fn set_membership_change_syncs() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut s: set[int] = {1, 2, 4, 6}
    h.flip_set(s)
    h.print_set(s)

main()
"#,
        "[2, 4, 6, 999]\n",
    );
}

#[test]
fn int_set_writeback_with_big_ints() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut s: set[int] = {1, 2}
    h.add_int_to_set(s, 99999)
    h.print_set(s)

main()
"#,
        "[1, 2, 99999]\n",
    );
}

#[test]
fn int_dict_keys_cross_to_python_without_string_dereference() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let d: {int: int} = {1: 2}
    print(h.take_dict(d))

main()
"#,
        "1\n",
    );
}

#[test]
fn typed_dict_key_kinds_survive_python_crossing_and_writeback() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut bi: {bool: int} = {True: 2}
    print(h.key_type(bi))
    print(h.key_type_kw(d=bi))
    h.set_key(bi, False)
    print(bi[False])

    let mut ii: {int: int} = {99999: 2}
    print(h.key_type(ii))
    h.set_key(ii, 100001)
    print(ii[100001])

    let mut fi: {float: int} = {1.5: 2}
    print(h.key_type(fi))
    h.set_key(fi, 2.5)
    print(fi[2.5])

    let mut si: {str: int} = {"x": 2}
    print(h.key_type(si))
    h.set_key(si, "y")
    print(si["y"])

    let mut ai: {bool: Any} = {True: 2}
    print(h.key_type(ai))
    h.set_key(ai, False)
    print(ai[False])

    let mut ei: {int: int} = {}
    h.set_key(ei, 99999)
    print(ei[99999])
    h.set_key_after(1, 2, 3, 4, 5, ei, 100001)
    print(ei[100001])

main()
"#,
        "bool\nbool\n9\nint\n9\nfloat\n9\nstr\n9\nbool\n9\n9\n9\n",
    );
}

#[test]
fn f32_and_none_collections_preserve_static_types() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let xs: [f32] = [1.5]
    print(h.first_item(xs))
    let mut xs2: [f32] = [1.5]
    h.set_first(xs2, 2.5)
    print(xs2[0])

    let s: set[f32] = {1.5}
    print(h.first_set_item(s))
    let mut s2: set[f32] = {1.5}
    h.set_value(s2, 2.5)
    print(h.contains(s2, 2.5))

    let d: {int: f32} = {1: 1.5}
    print(h.first_dict_value(d))
    let mut d2: {f32: int} = {1.5: 2}
    h.set_key(d2, 2.5)
    print(d2[2.5])

    let ns: [None] = [None]
    print(h.first_type(ns))
    let nd: {str: None} = {"x": None}
    print(h.first_value_type(nd))

main()
"#,
        "1.5\n2.5\n1.5\nTrue\n1.5\n9\nNoneType\nNoneType\n",
    );
}

#[test]
fn unsigned_values_cross_as_unsigned_python_integers() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let one: u64 = 1
    let high: u64 = one << 63
    print(h.identity(high))
    let d: {u64: int} = {high: 1}
    print(h.key_type(d))
    let xs: [u64] = [high]
    print(h.first_item(xs))
    let s: set[u64] = {high}
    print(h.first_set_item(s))
    let values: {int: u64} = {1: high}
    print(h.first_dict_value(values))

main()
"#,
        "9223372036854775808\nint\n9223372036854775808\n9223372036854775808\n9223372036854775808\n",
    );
}

#[test]
fn safe_failed_writeback_releases_python_result() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn run(xs: [int]) -> PyObject | Error:
    return try h.bad_probe(xs)

fn main():
    match run([1]):
        Error(_):
            print("caught")
        _:
            print("bad")
    print(h.probe_was_deleted())

main()
"#,
        "caught\nTrue\n",
    );
}

#[test]
fn safe_writeback_rejects_custom_dict_key_without_aborting() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn run(d: {str: Any}, key: PyObject) -> PyObject | Error:
    return try h.put_mutating_key(d, key)

fn main():
    let mut d: {str: Any} = {"old": 5}
    let key: PyObject = h.make_mutating_key()
    match run(d, key):
        Error(_e):
            print("caught")
        _:
            print("bad")
    print(d.get("old", -1))

main()
"#,
        "caught\n5\n",
    );
}

#[test]
fn nested_typed_collections_cross_with_static_types() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let d: {str: {bool: None}} = {"inner": {True: None}}
    print(h.nested_value(d))
    let xs: [[f32]] = [[1.5]]
    print(xs)

main()
"#,
        "{True: None}\n[[1.5]]\n",
    );
}

#[test]
fn legacy_high_arity_typed_dict_keeps_key_and_value_types() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let d: {bool: bool} = {True: True}
    print(h.inspect_legacy(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, d))

main()
"#,
        "[('bool', True, 'bool', True)]\n",
    );
}

#[test]
fn concrete_tuple_dict_key_crosses_as_hashable_python_key() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn run(d: {(int, f32): int}) -> PyObject | Error:
    return try h.inspect_tuple_key(d)

fn main():
    let d: {(int, f32): int} = {(1, 1.5): 7}
    match run(d):
        Error(_e):
            print("caught")
        _:
            print("bad")

main()
"#,
        "bad\n",
    );
}

#[test]
fn safe_call_reports_unhashable_native_dict_key() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn run(d: {Any: int}) -> PyObject | Error:
    return try h.inspect_dict(d)

fn main():
    let d: {Any: int} = {[1]: 2}
    match run(d):
        Error(_e):
            print("caught")
        _:
            print("bad")

main()
"#,
        "caught\n",
    );
}

#[test]
fn list_based_none_collection_keeps_its_owner() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let xs: [None] = [None]
    h.consume5(0, 0, 0, 0, xs)
    print(xs)

main()
"#,
        "[None]\n",
    );
}

#[test]
fn any_bool_keys_remain_distinct_from_int_keys() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let d: {Any: int} = {True: 1, 1: 2}
    print(len(d))
    print(h.any_dict_items(d))

main()
"#,
        "2\n[('bool', True, 'int')]\n",
    );
}

#[test]
fn any_python_object_key_identity_survives_writeback() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let key: PyObject = h.make_custom_key()
    let mut d: {Any: int} = {}
    h.put_custom_key(d, key)
    print(d.get(key, -1))

main()
"#,
        "1\n",
    );
}

#[test]
fn any_wide_integer_key_identity_survives_writeback() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let key: PyObject = h.make_big_key()
    let mut d: {Any: int} = {}
    h.put_big_key(d)
    print(d.get(key, -1))

main()
"#,
        "1\n",
    );
}

#[test]
fn any_unsigned_key_preserves_unsigned_value() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let one: u64 = 1
    let high: u64 = one << 63
    let d: {Any: int} = {high: 1}
    print(h.first_key(d))

main()
"#,
        "9223372036854775808\n",
    );
}

#[test]
fn typed_unsigned_key_relookup_survives_python_mutation() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let one: u64 = 1
    let high: u64 = one << 63
    let mut d: {u64: int} = {}
    h.put_high_u64_key(d)
    print(high in d)
    print(d[high])

main()
"#,
        "True\n7\n",
    );
}

#[test]
fn any_object_keys_are_released_on_dict_writeback() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut d: {Any: int} = {}
    h.replace_any_key(d)
    print(h.live_any_keys())
    h.replace_any_key(d)
    print(h.live_any_keys())

main()
"#,
        "1\n1\n",
    );
}

#[test]
fn custom_dict_key_rejected_without_iteration_use_after_free() {
    assert_both_fail_with(
        r#"import py "wbhelper" as h

fn main():
    let mut d: {str: Any} = {}
    let key: PyObject = h.make_mutating_key()
    h.put_mutating_key(d, key)

main()
"#,
        "E0714",
    );
}

#[test]
fn many_keywords_with_positional_dict_use_preconverted_arguments() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let d: {int: int} = {}
    h.kw17(d, a0=0, a1=1, a2=2, a3=3, a4=4, a5=5, a6=6, a7=7, a8=8, a9=9, a10=10, a11=11, a12=12, a13=13, a14=14, a15=15, a16=16)
    print(d.get(2, -1))

main()
"#,
        "-1\n",
    );
}

#[test]
fn seventeen_argument_call_forces_no_copy_out_without_crashing() {
    assert_both_succeed(
        r#"import py "wbhelper" as h

fn main():
    let mut xs: [int] = [1, 2, 3]
    h.append_first17(xs, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16)
    print(xs)

main()
"#,
        "[1, 2, 3]\n",
    );
}

#[test]
fn repeated_calls_on_the_same_list_do_not_leak() {
    assert_both_succeed(
        r#"import py "sys" as sys
import py "wbhelper" as h

fn main():
    let mut xs: [int] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
    let mut i = 0
    while i < 2000:
        h.do_sort(xs)
        i = i + 1
    let before = sys.getallocatedblocks()
    let mut j = 0
    while j < 2000:
        h.do_sort(xs)
        j = j + 1
    let after = sys.getallocatedblocks()
    print(after - before < 200)

main()
"#,
        "True\n",
    );
}
