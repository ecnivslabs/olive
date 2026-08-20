//! Arithmetic and bitwise operators on types lowering does not implement.
//!
//! The checker used to unify same-type operands blindly, so `d | e` on dicts,
//! `a - b` on lists, `%` and shifts on floats, and every operator on tuples,
//! enums, trait objects, and functions compiled to integer bit operations on
//! heap pointers (faults, garbage, leaks). Each case below must now fail at
//! compile time with E0404 on both pipelines; the valid combinations after
//! them must keep working on both pipelines.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

fn write_src(src: &str) -> PathBuf {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "olive_operator_rejection_{}_{id}.liv",
        std::process::id()
    ));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(src.as_bytes()).unwrap();
    path
}

fn run_jit(path: &std::path::Path) -> (String, i32) {
    let out = Command::new(pit_bin())
        .arg("run")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    (
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn build_aot(path: &std::path::Path) -> (String, bool) {
    let out_bin = path.with_extension("bin");
    let build = Command::new(pit_bin())
        .arg("build")
        .arg("--release")
        .arg(path)
        .arg("-o")
        .arg(&out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit build");
    std::fs::remove_file(&out_bin).ok();
    (
        String::from_utf8_lossy(&build.stderr).into_owned(),
        build.status.success(),
    )
}

/// The program must fault at runtime on both pipelines: exit 1 with the
/// coded panic. Used for runtime guards where the static type cannot name
/// the bad shape (union members, dynamic words).
fn assert_faults_e0700(src: &str, frag: &str) {
    assert_faults_with(src, "[E0700]", frag);
}

/// Same shape for E0713 arithmetic faults.
fn assert_faults_e0713(src: &str, frag: &str) {
    assert_faults_with(src, "[E0713]", frag);
}

fn assert_faults_with(src: &str, code: &str, frag: &str) {
    let path = write_src(src);
    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    assert_eq!(out.status.code(), Some(1), "jit exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(code), "jit stderr: {stderr}");
    assert!(stderr.contains(frag), "jit stderr: {stderr}");
    let out_bin = path.with_extension("bin");
    let build = Command::new(pit_bin())
        .arg("build")
        .arg("--release")
        .arg(&path)
        .arg("-o")
        .arg(&out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit build");
    assert!(
        build.status.success(),
        "aot build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(&out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn built binary");
    assert_eq!(out.status.code(), Some(1), "aot exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(code), "aot stderr: {stderr}");
    assert!(stderr.contains(frag), "aot stderr: {stderr}");
    std::fs::remove_file(&out_bin).ok();
    std::fs::remove_file(&path).ok();
}

/// The program must be rejected at compile time on both pipelines: `pit run`
/// exits 1 with the coded diagnostic, and `pit build` fails with it too.
fn assert_rejected_with(src: &str, code: &str, frag: &str) {
    let path = write_src(src);
    let (stderr, exit) = run_jit(&path);
    assert_eq!(exit, 1, "jit exit: {exit}, stderr: {stderr}");
    assert!(stderr.contains(code), "jit stderr: {stderr}");
    assert!(stderr.contains(frag), "jit stderr: {stderr}");
    let (build_stderr, ok) = build_aot(&path);
    assert!(!ok, "aot build unexpectedly succeeded for: {src}");
    assert!(build_stderr.contains(code), "aot stderr: {build_stderr}");
    assert!(build_stderr.contains(frag), "aot stderr: {build_stderr}");
    std::fs::remove_file(&path).ok();
}

/// The program must be rejected at compile time on both pipelines: `pit run`
/// exits 1 with the E0404 diagnostic, and `pit build` fails with it too.
fn assert_rejected(src: &str, frag: &str) {
    assert_rejected_with(src, "[E0404]", frag);
}

/// The program must still compile and run identically on both pipelines.
fn assert_accepted(src: &str, expected: &str) {
    let path = write_src(src);
    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    assert!(
        out.status.success(),
        "jit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
    let out_bin = path.with_extension("bin");
    let build = Command::new(pit_bin())
        .arg("build")
        .arg("--release")
        .arg(&path)
        .arg("-o")
        .arg(&out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit build");
    assert!(
        build.status.success(),
        "aot build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(&out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn built binary");
    assert!(out.status.success(), "aot run failed");
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
    std::fs::remove_file(&out_bin).ok();
    std::fs::remove_file(&path).ok();
}

#[test]
fn dict_bitwise_or_rejected_with_merge_help() {
    assert_rejected(
        "fn main():\n    let a = {\"k1\": 1}\n    let b = {\"k2\": 2}\n    let c = a | b\n    print(c)\n",
        "d.update(other)",
    );
}

#[test]
fn dict_arithmetic_rejected() {
    assert_rejected(
        "fn main():\n    let a = {\"k1\": 1}\n    let b = {\"k2\": 2}\n    let c = a - b\n    print(c)\n",
        "not defined for",
    );
}

#[test]
fn dict_augmented_or_rejected() {
    assert_rejected(
        "fn main():\n    let a = {\"k1\": 1}\n    let b = {\"k2\": 2}\n    a |= b\n    print(len(a))\n",
        "not defined for",
    );
}

#[test]
fn list_subtraction_rejected() {
    assert_rejected(
        "fn main():\n    let c = [1] - [2]\n    print(c)\n",
        "concatenate lists",
    );
}

#[test]
fn string_subtraction_rejected() {
    assert_rejected(
        "fn main():\n    let c = \"x\" - \"y\"\n    print(c)\n",
        "concatenate strings",
    );
}

#[test]
fn tuple_addition_rejected() {
    assert_rejected(
        "fn main():\n    let c = (1, 2) + (3, 4)\n    print(c)\n",
        "not defined for",
    );
}

#[test]
fn float_remainder_rejected() {
    assert_rejected(
        "fn main():\n    let c = 1.5 % 2.5\n    print(c)\n",
        "floats support only",
    );
}

#[test]
fn float_bitwise_or_rejected() {
    assert_rejected(
        "fn main():\n    let c = 1.5 | 2.5\n    print(c)\n",
        "floats support only",
    );
}

#[test]
fn set_addition_rejected() {
    assert_rejected(
        "fn main():\n    let c = {1} + {2}\n    print(c)\n",
        "sets support only",
    );
}

#[test]
fn function_addition_rejected() {
    assert_rejected(
        "fn f() -> int:\n    return 1\nfn g() -> int:\n    return 2\nfn main():\n    let c = f + g\n    print(c)\n",
        "not defined for",
    );
}

#[test]
fn enum_addition_rejected() {
    assert_rejected(
        "enum E:\n    V(int)\n    W(int)\nfn main():\n    let c = V(1) + W(2)\n    print(c)\n",
        "not defined for",
    );
}

#[test]
fn sum_min_max_misuse_rejected() {
    assert_rejected(
        "fn main():\n    print(sum([\"a\", \"b\"]))\n",
        "requires a list, tuple, or set",
    );
    assert_rejected(
        "fn main():\n    print(sum({\"a\": 1}))\n",
        "requires a list, tuple, or set",
    );
    assert_rejected(
        "fn main():\n    print(sum(5))\n",
        "requires a list, tuple, or set",
    );
    assert_rejected(
        "fn main():\n    print(min({\"a\": 1}, {\"b\": 2}))\n",
        "comparable numbers or strings",
    );
}

#[test]
fn union_method_calls_rejected_with_narrow_help() {
    assert_rejected_with(
        "fn f(v: str | int):\n    return v.upper()\nfn main():\n    print(f(\"hi\"))\n",
        "[E0422]",
        "narrow the union first",
    );
    assert_rejected_with(
        "fn f(v: dict[str, int] | int):\n    return v.get(\"a\", -1)\nfn main():\n    print(f({\"a\": 5}))\n",
        "[E0422]",
        "narrow the union first",
    );
}

#[test]
fn join_element_mismatch_rejected() {
    assert_rejected_with(
        "fn main():\n    print(\",\".join([1, 2]))\n",
        "[E0404]",
        "must be `[str]`",
    );
    assert_rejected_with(
        "fn main():\n    print(\",\".join(5))\n",
        "[E0404]",
        "must be `[str]`",
    );
    assert_accepted(
        "fn main():\n    print(\",\".join([]))\n    print(\",\".join([\"a\", \"b\"]))\n",
        "\na,b\n",
    );
}

#[test]
fn collection_element_mismatches_rejected() {
    assert_rejected_with(
        "fn main():\n    let l = [1]\n    l.append(\"a\")\n    print(l)\n",
        "[E0400]",
        "mismatched types",
    );
    assert_rejected_with(
        "fn main():\n    let l = [1, 2]\n    l.insert(\"a\", 9)\n    print(l)\n",
        "[E0400]",
        "mismatched types",
    );
    assert_rejected_with(
        "fn main():\n    let l = [1]\n    l.extend([\"a\"])\n    print(l)\n",
        "[E0400]",
        "mismatched types",
    );
    assert_rejected_with(
        "fn main():\n    let d = {\"a\": 1}\n    d.update({\"b\": \"x\"})\n    print(d)\n",
        "[E0400]",
        "mismatched types",
    );
    assert_rejected_with(
        "fn main():\n    let d = {\"a\": 1}\n    print(d.setdefault(\"zz\", \"s\"))\n",
        "[E0400]",
        "mismatched types",
    );
    assert_rejected_with(
        "fn main():\n    print([1, 2].count(\"a\"))\n",
        "[E0400]",
        "mismatched types",
    );
    assert_rejected_with(
        "fn f(v: Any):\n    let l = [1]\n    l.append(v)\n    print(l)\nfn main():\n    f(2)\n",
        "[E0404]",
        "requires a concrete argument",
    );
}

/// Integer `**` is checked in both pipelines (unlike `+`/`-`/`*`, whose
/// per-op checks cost 30-50% in release): one exponentiation pays O(log n)
/// checked multiplies total, so there is no hot path to protect, and the
/// old code aborted in debug while wrapping silently in release.
#[test]
fn int_pow_values_and_overflow() {
    assert_accepted(
        "fn main():\n    print(2 ** 10)\n    print(-2 ** 2)\n    print(2 ** 3 ** 2)\n    print(0 ** 0)\n    print(2 ** 62)\n    print(2.0 ** 3.0)\n",
        "1024\n-4\n512\n1\n4611686018427387904\n8.0\n",
    );
    assert_faults_e0713(
        "fn main():\n    print(10 ** 30)\n",
        "10 ** 30 does not fit in i64",
    );
    assert_faults_e0713("fn main():\n    print(2 ** -1)\n", "negative exponent");
}

#[test]
fn struct_enum_to_python_faults() {
    assert_faults_e0700(
        "import py \"json\" as json\nstruct S:\n    x: int\nfn main():\n    print(json.dumps(S(1)))\n",
        "cannot convert struct or enum",
    );
    assert_faults_e0700(
        "import py \"json\" as json\nenum E:\n    V(int)\nfn main():\n    print(json.dumps(V(3)))\n",
        "cannot convert struct or enum",
    );
    assert_faults_e0700(
        "import py \"json\" as json\nstruct S:\n    x: int\nfn main():\n    print(json.dumps([S(1)]))\n",
        "cannot convert struct or enum",
    );
}

#[test]
fn builtin_arity_mismatches_rejected() {
    assert_rejected_with(
        "fn main():\n    print(sum())\n",
        "[E0402]",
        "function signature mismatch",
    );
    assert_rejected_with(
        "fn main():\n    print(min(1, 2, 3))\n",
        "[E0402]",
        "function signature mismatch",
    );
    assert_rejected_with(
        "fn main():\n    print(abs())\n",
        "[E0402]",
        "function signature mismatch",
    );
    assert_rejected_with(
        "fn main():\n    print(sorted())\n",
        "[E0402]",
        "function signature mismatch",
    );
    assert_rejected_with(
        "fn main():\n    print(input(\"a\", \"b\"))\n",
        "[E0402]",
        "function signature mismatch",
    );
}

#[test]
fn method_arity_mismatches_rejected() {
    assert_rejected_with(
        "fn main():\n    let l = [1]\n    l.append()\n    print(l)\n",
        "[E0403]",
        "wrong number of arguments",
    );
    assert_rejected_with(
        "fn main():\n    let l = [1]\n    l.insert(0)\n    print(l)\n",
        "[E0403]",
        "wrong number of arguments",
    );
    assert_rejected_with(
        "fn main():\n    print([1, 2].pop(0))\n",
        "[E0403]",
        "wrong number of arguments",
    );
    assert_rejected_with(
        "fn main():\n    let l = [2, 1]\n    l.sort(1)\n    print(l)\n",
        "[E0403]",
        "wrong number of arguments",
    );
    assert_rejected_with(
        "fn main():\n    let d = {\"a\": 1}\n    d.remove()\n    print(d)\n",
        "[E0403]",
        "wrong number of arguments",
    );
    assert_rejected_with(
        "fn main():\n    let s = {1}\n    s.add(2, 3)\n    print(s)\n",
        "[E0403]",
        "wrong number of arguments",
    );
    assert_rejected_with(
        "fn main():\n    print(\"abc\".find())\n",
        "[E0403]",
        "wrong number of arguments",
    );
    assert_rejected_with(
        "fn main():\n    print(\"x\".upper(1))\n",
        "[E0403]",
        "wrong number of arguments",
    );
}

#[test]
fn iteration_misuse_rejected() {
    assert_rejected_with(
        "fn main():\n    for x in 5:\n        print(x)\n",
        "[E0404]",
        "cannot iterate over",
    );
    assert_rejected_with(
        "struct S:\n    x: int\nfn main():\n    for v in S(1):\n        print(v)\n",
        "[E0404]",
        "cannot iterate over",
    );
    assert_rejected_with(
        "fn f(v: [int] | int):\n    for x in v:\n        print(x)\nfn main():\n    f([1])\n",
        "[E0404]",
        "narrow the union first",
    );
    assert_rejected_with(
        "fn main():\n    print([x for x in 5])\n",
        "[E0404]",
        "cannot iterate over",
    );
}

#[test]
fn iteration_edge_shapes_work() {
    assert_accepted(
        "fn f(v: [int] | None):\n    for x in v:\n        print(x)\nfn main():\n    f([1])\n    f(None)\n    print(\"done\")\n",
        "1\ndone\n",
    );
    assert_accepted(
        "fn main():\n    for x in \"ab\"[0]:\n        print(x)\n    print(\"done\")\n",
        "a\ndone\n",
    );
    assert_accepted(
        "fn f(v: Any):\n    for x in v:\n        print(x)\nfn main():\n    f([7])\n    f(5)\n    print(\"done\")\n",
        "7\ndone\n",
    );
}

/// A monomorphized generic body can reach lowering with a struct operand
/// whose dunder is absent (generic bodies skip the checker's dunder gate):
/// instead of aborting codegen, lowering faults with the checker's own
/// wording. Concrete shapes keep their compile-time E0404.
/// A monomorphized generic body can reach lowering with a collection type
/// the checker gate passed as an unresolved param: `sum`/`min`/`max` would
/// read struct words with the integer reducers (a segfault once the garbage
/// is used), and sorts would silently mis-order. Lowering faults with the
/// checker's own wording instead. Concrete shapes keep compile-time E0404.
#[test]
fn generic_bad_collection_instantiation_faults() {
    assert_faults_e0700(
        "struct S:\n    x: int\nfn mysum[T](xs: [T]) -> T:\n    return sum(xs)\nfn main():\n    print(mysum([S(1), S(2)]))\n",
        "`sum` requires a list, tuple, or set of numbers",
    );
    assert_faults_e0700(
        "struct S:\n    x: int\nfn mymin[T](xs: [T]) -> T:\n    return min(xs)\nfn main():\n    print(mymin([S(2), S(1)]))\n",
        "`min` requires a list, tuple, or set of numbers",
    );
    assert_faults_e0700(
        "struct S:\n    x: int\nfn mm[T](a: T, b: T) -> T:\n    return min(a, b)\nfn main():\n    print(mm(S(1), S(2)))\n",
        "`min` requires two comparable numbers or strings",
    );
    assert_faults_e0700(
        "struct S:\n    x: int\nfn mm[T](a: T, b: T) -> T:\n    return max(a, b)\nfn main():\n    print(mm(S(1), S(2)))\n",
        "`max` requires two comparable numbers or strings",
    );
    assert_faults_e0700(
        "struct S:\n    x: int\nfn srt[T](xs: [T]):\n    xs.sort()\n    return xs\nfn main():\n    print(srt([S(2), S(1)]))\n",
        "`S` has no `__lt__` defined",
    );
    assert_faults_e0700(
        "struct S:\n    x: int\nfn srt[T](xs: [T]):\n    return sorted(xs)\nfn main():\n    print(srt([S(2), S(1)]))\n",
        "`S` has no `__lt__` defined",
    );
    assert_accepted(
        "fn mysum[T](xs: [T]) -> T:\n    return sum(xs)\nfn mymin[T](xs: [T]) -> T:\n    return min(xs)\nfn srt[T](xs: [T]):\n    xs.sort()\n    return xs\nfn main():\n    print(mysum([1, 2, 3]))\n    print(mymin([3, 1, 2]))\n    print(srt([3, 1, 2]))\n    print(srt([\"b\", \"a\"]))\n",
        "6\n1\n[1, 2, 3]\n[\"a\", \"b\"]\n",
    );
    assert_accepted(
        "fn sum(n: int) -> int:\n    let mut s = 0\n    let mut i = 1\n    while i <= n:\n        s = s + i\n        i = i + 1\n    return s\nfn main():\n    print(sum(10))\n    print(sum(100))\n",
        "55\n5050\n",
    );
}

#[test]
fn generic_missing_dunder_faults_cleanly() {
    assert_faults_e0700(
        "struct S:\n    x: int\nfn lt[T](a: T, b: T) -> bool:\n    return a < b\nfn main():\n    print(lt(S(1), S(2)))\n",
        "`S` has no `__lt__` defined",
    );
    assert_faults_e0700(
        "struct S:\n    x: int\nfn add[T](a: T, b: T) -> T:\n    return a + b\nfn main():\n    print(add(S(1), S(2)))\n",
        "`S` has no `__add__` defined",
    );
    assert_accepted(
        "struct S:\n    x: int\nimpl S:\n    fn __lt__(self: &S, other: &S) -> bool:\n        return self.x < other.x\nfn lt[T](a: T, b: T) -> bool:\n    return a < b\nfn main():\n    print(lt(S(1), S(2)))\n    print(lt(1, 2))\n",
        "True\nTrue\n",
    );
}

/// Monomorphized generic functions must return the specialized type, not
/// `Any`: the specialization had no signature entry, so `_return` defaulted
/// to `Any` and boxed concrete values the caller then read raw (`ident`
/// over `[3, 1, 2]` came back `[26, 10, 18]`, floats/bools/nested garbled
/// the same way, and even `sort` on ints mis-sorted).
#[test]
fn generic_container_returns_keep_concrete_values() {
    assert_accepted(
        "fn ident[T](xs: [T]):\n    return xs\nfn main():\n    print(ident([3, 1, 2]))\n    print(ident([1.5, 2.5]))\n    print(ident([True, False]))\n    print(ident([[1], [2]]))\n",
        "[3, 1, 2]\n[1.5, 2.5]\n[True, False]\n[[1], [2]]\n",
    );
    assert_accepted(
        "fn srt[T](xs: [T]):\n    xs.sort()\n    return xs\nfn main():\n    print(srt([3, 1, 2]))\n",
        "[1, 2, 3]\n",
    );
    assert_accepted(
        "struct Res:\n    s: str\nimpl Res:\n    fn __drop__(self):\n        print(\"drop \"+self.s)\n\nfn ident[T](xs: [T]):\n    return xs\n\nfn main():\n    let y = ident([Res(\"a\"), Res(\"b\")])\n    print(y[0].s)\n    print(y[1].s)\n    print(\"done\")\n",
        "a\nb\ndone\ndrop a\ndrop b\n",
    );
    assert_accepted(
        "fn ident[T](xs: [T]):\n    return xs\nfn main():\n    let a: Any = ident([3, 1, 2])\n    print(a)\n    print(a[0])\n",
        "[3, 1, 2]\n3\n",
    );
}

#[test]
fn generic_bodies_hold_sound_gate_behavior() {
    assert_rejected_with(
        "fn neg[T](x: T) -> T:\n    return -x\nfn main():\n    print(neg(5))\n",
        "[E0404]",
        "not defined for `T`",
    );
    assert_rejected_with(
        "fn cond[T](x: T):\n    if x:\n        print(\"yes\")\nfn main():\n    cond(True)\n",
        "[E0404]",
        "cannot be used as a condition",
    );
    assert_accepted(
        "fn has[T](xs: [T], v: T) -> bool:\n    return v in xs\nfn mylen[T](xs: [T]) -> int:\n    return len(xs)\nfn lt[T](a: T, b: T) -> bool:\n    return a < b\nfn add[T](a: T, b: T) -> T:\n    return a + b\nfn main():\n    print(has([1, 2], 2))\n    print(mylen([1]))\n    print(lt(1, 2))\n    print(add(3, 4))\n",
        "True\n1\nTrue\n7\n",
    );
}

#[test]
fn slice_misuse_rejected() {
    assert_rejected("fn main():\n    print({\"a\": 1}[0:1])\n", "cannot slice");
    assert_rejected("fn main():\n    print(5[0:1])\n", "cannot slice");
    assert_rejected(
        "fn f(v: [int] | str):\n    return v[0:1]\nfn main():\n    print(f([9]))\n",
        "narrow the union first",
    );
}

#[test]
fn membership_misuse_rejected() {
    assert_rejected_with(
        "fn main():\n    print(1 in 2)\n",
        "[E0404]",
        "container on the right",
    );
    assert_rejected_with(
        "fn main():\n    print(1 in \"abc\")\n",
        "[E0404]",
        "string needle",
    );
    assert_rejected_with(
        "fn f(v: [int] | int):\n    return 1 in v\nfn main():\n    print(f([1]))\n",
        "[E0404]",
        "narrow the union first",
    );
    assert_rejected_with(
        "fn f(v: Any):\n    return 1 in v\nfn main():\n    print(f([1]))\n",
        "[E0404]",
        "narrow to a container first",
    );
    assert_rejected_with(
        "fn main():\n    print([1] in [[1]])\n",
        "[E0404]",
        "scalar needle",
    );
}

#[test]
fn deep_membership_works() {
    assert_accepted(
        "struct S:\n    x: int\nfn main():\n    print(S(1) in [S(1)])\n    print(S(2) in [S(1)])\n    print(1 in [1, 2])\n    print(9 not in [1])\n    print(\"a\" in \"abc\")\n    print(0 in bytes_new(4))\n",
        "True\nFalse\nTrue\nTrue\nTrue\nTrue\n",
    );
}

#[test]
fn len_of_scalars_rejected() {
    assert_rejected("fn main():\n    print(len(5))\n", "sized collection");
    assert_rejected(
        "struct S:\n    x: int\nfn main():\n    print(len(S(1)))\n",
        "sized collection",
    );
}

#[test]
fn len_of_union_dispatches_by_member() {
    assert_accepted(
        "fn f(v: [int] | str):\n    return len(v)\nfn main():\n    print(f([1, 2, 3]))\n    print(f(\"hello\"))\n",
        "3\n5\n",
    );
}

#[test]
fn len_of_unsized_union_member_faults() {
    let src = "fn f(v: [int] | int):\n    return len(v)\nfn main():\n    print(f(5))\n";
    let path = write_src(src);
    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[E0700]"), "stderr: {stderr}");
    let out_bin = path.with_extension("bin");
    let build = Command::new(pit_bin())
        .arg("build")
        .arg("--release")
        .arg(&path)
        .arg("-o")
        .arg(&out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit build");
    assert!(build.status.success(), "aot build failed");
    let out = Command::new(&out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn built binary");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[E0700]"), "aot stderr: {stderr}");
    std::fs::remove_file(&out_bin).ok();
    std::fs::remove_file(&path).ok();
}

#[test]
fn ordering_of_aggregates_rejected() {
    assert_rejected(
        "fn main():\n    print({\"a\": 1} < {\"b\": 2})\n",
        "ordering `<`",
    );
    assert_rejected("fn main():\n    print([2] < [1])\n", "ordering `<`");
    assert_rejected("fn main():\n    print([1] < 2)\n", "ordering `<`");
    assert_rejected(
        "fn f(v: int | None) -> bool:\n    return v < 1\nfn main():\n    print(f(0))\n",
        "narrow the union first",
    );
}

#[test]
fn unary_negation_of_aggregates_rejected() {
    assert_rejected("fn main():\n    print(-[1])\n", "unary operator `-`");
    assert_rejected("fn main():\n    print(-{\"a\": 1})\n", "unary operator `-`");
    assert_rejected(
        "struct S:\n    x: int\nfn main():\n    print(-S(1))\n",
        "unary operator `-`",
    );
}

#[test]
fn invert_of_list_and_float_rejected() {
    assert_rejected("fn main():\n    print(~[1])\n", "unary operator `~`");
    assert_rejected("fn main():\n    print(~1.5)\n", "unary operator `~`");
}

#[test]
fn valid_combinations_still_work() {
    assert_accepted(
        "fn main():\n    print([1] + [2])\n    print(\"ab\" * 2)\n    print(2 * [1])\n    print(6 * 7)\n    print(7 % 3)\n    print({1} | {2})\n    print({1} & {2})\n    print(1.5 + 2.5)\n    print(2.0 ** 3.0)\n",
        "[1, 2]\nabab\n[1, 1]\n42\n1\n{1, 2}\n{}\n4.0\n8.0\n",
    );
    assert_accepted(
        "fn main():\n    print(sum([1, 2, 3]))\n    print(sum([1.5, 2.5]))\n    print(sum([]))\n    print(sum([True, False, True]))\n    print(min([3, 1]))\n    print(max([3, 1]))\n    print(sum((1, 2)))\n    print(sum({1, 2}))\n    print(min(\"b\", \"a\"))\n    print(max(\"b\", \"a\"))\n",
        "6\n4.0\n0\n2\n1\n3\n3\n3\na\nb\n",
    );
    assert_accepted(
        "fn main():\n    let mut a = 3\n    a += 4\n    print(a)\n    let mut b = [1]\n    b += [2]\n    print(b)\n",
        "7\n[1, 2]\n",
    );
    assert_accepted(
        "fn f(v: int | None) -> int:\n    if v != None:\n        return -v\n    return 0\nfn main():\n    print(-5)\n    print(-1.5)\n    print(~5)\n    print(f(3))\n    print(f(None))\n",
        "-5\n-1.5\n-6\n-3\n0\n",
    );
    assert_accepted(
        "fn g(v: int | None) -> bool:\n    if v != None:\n        return v < 1\n    return False\nfn main():\n    print(\"a\" < \"b\")\n    print(\"b\" < \"a\")\n    print(\"abc\" <= \"abd\")\n    print(\"b\" > \"a\")\n    print(1 < 2)\n    print(2.5 > 1)\n    print(g(0))\n    print(g(None))\n",
        "True\nFalse\nTrue\nTrue\nTrue\nTrue\nTrue\nFalse\n",
    );
    assert_accepted(
        "fn len(x: int) -> int:\n    return x * 2\nfn main():\n    print(len(5))\n",
        "10\n",
    );
}
