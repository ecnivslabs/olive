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
