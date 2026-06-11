//! Roadmap E13.1: a failing `assert` on a top-level comparison reports both
//! operand values (`E0712`), not just the source text. `abort_with` calls
//! `std::process::exit`, so this has to run as a real subprocess -- an
//! in-process JIT call would kill the test runner along with the assert.

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
        "olive_assert_fault_{}_{id}.liv",
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

fn run_aot(path: &std::path::Path) -> (String, i32) {
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
    assert!(
        build.status.success(),
        "AOT build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(&out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn built binary");
    std::fs::remove_file(&out_bin).ok();
    (
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// `assert xs == ys` on lists: both operand values must appear, and the
/// fault must be coded `E0712`, not the generic `E0700`. Runs under JIT
/// (`pit run`) and AOT release, since a wiring bug can easily fire in one
/// pipeline and not the other (W6).
#[test]
fn assert_eq_on_lists_shows_both_operands_both_pipelines() {
    let src = "fn main():\n    let xs = [1, 2, 3]\n    let ys = [1, 2, 4]\n    assert xs == ys\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 1, "jit stderr: {stderr}");
    assert!(stderr.contains("[E0712]"), "jit stderr: {stderr}");
    assert!(stderr.contains("left: [1, 2, 3]"), "jit stderr: {stderr}");
    assert!(stderr.contains("right: [1, 2, 4]"), "jit stderr: {stderr}");

    // AOT linking on Windows MSVC requires MinGW but cranelift emits COFF
    // objects while MinGW's ld expects a compatible format. The current
    // `cc`-based linker only works under MinGW-w64 (GNU target). Skip this
    // half on MSVC Windows to avoid `collect2.exe: ld returned 1 exit status`.
    if !cfg!(all(target_os = "windows", target_env = "msvc")) {
        let (stderr, code) = run_aot(&path);
        assert_eq!(code, 1, "aot stderr: {stderr}");
        assert!(stderr.contains("[E0712]"), "aot stderr: {stderr}");
        assert!(stderr.contains("left: [1, 2, 3]"), "aot stderr: {stderr}");
        assert!(stderr.contains("right: [1, 2, 4]"), "aot stderr: {stderr}");
    }

    std::fs::remove_file(&path).ok();
}

/// A non-comparison assert (`assert flag`) has nothing to introspect: no
/// `left`/`right` line, just the coded fault and the source caret.
#[test]
fn assert_non_comparison_has_no_operand_line() {
    let src = "fn main():\n    let flag = False\n    assert flag\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 1, "jit stderr: {stderr}");
    assert!(stderr.contains("[E0712]"), "jit stderr: {stderr}");
    assert!(!stderr.contains("left:"), "jit stderr: {stderr}");
    assert!(stderr.contains("assert flag"), "jit stderr: {stderr}");

    std::fs::remove_file(&path).ok();
}

/// `assert cond, "msg"`: the custom message and the operand values both show.
#[test]
fn assert_custom_message_joins_operand_values() {
    let src =
        "fn main():\n    let a = 3\n    let b = 5\n    assert a == b, \"totals must match\"\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 1, "jit stderr: {stderr}");
    assert!(stderr.contains("totals must match"), "jit stderr: {stderr}");
    assert!(stderr.contains("left: 3"), "jit stderr: {stderr}");
    assert!(stderr.contains("right: 5"), "jit stderr: {stderr}");

    std::fs::remove_file(&path).ok();
}

/// A comparison operand with a side effect is evaluated exactly once, even
/// though its value is used both for the boolean test and for the fault
/// message -- a naive "re-evaluate for display" implementation would call it
/// twice.
#[test]
fn assert_comparison_operand_evaluated_once() {
    let src = "fn bump(x: i64) -> i64:\n    print(\"called\")\n    return x + 1\n\nfn main():\n    assert bump(1) == bump(5)\n";
    let path = write_src(src);

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.matches("called").count(),
        2,
        "each operand must be evaluated exactly once: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("left: 2"), "stderr: {stderr}");
    assert!(stderr.contains("right: 6"), "stderr: {stderr}");

    std::fs::remove_file(&path).ok();
}

/// Passing asserts don't abort and don't print anything.
#[test]
fn assert_passing_is_silent() {
    let src = "fn main():\n    let a = 3\n    let b = 5\n    assert a < b\n    print(\"ok\")\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 0, "jit stderr: {stderr}");
    assert!(stderr.is_empty(), "jit stderr: {stderr}");

    std::fs::remove_file(&path).ok();
}
