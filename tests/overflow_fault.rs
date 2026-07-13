//! Roadmap E14: `i64` arithmetic that overflows must fault (`E0713`) instead
//! of silently wrapping, in both pipelines. `abort_with` calls
//! `std::process::exit`, so this has to run as a real subprocess -- an
//! in-process JIT call would kill the test runner along with the fault.

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
        "olive_overflow_fault_{}_{id}.liv",
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

/// Same as `run_aot`, but also returns stdout, to check the wrapped value
/// release actually prints when it silently wraps `+`/`-`/`*` overflow.
fn run_aot_stdout(path: &std::path::Path) -> (String, String, i32) {
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
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// `i64::MAX + 1` faults `E0713` with both operands, in both pipelines,
/// whether or not the operands are compile-time constants (the release
/// optimizer's constant folder must not silently pre-wrap the value).
///
/// Checking `+`/`-`/`*` cost 30-50% on arithmetic-heavy release code
/// (`compare.sh`), so only `pit run` checks them; `pit build --release`
/// wraps silently instead (documented in `basics.md` and this fault's own
/// explain entry). The `/ -1`/`% -1` corner below is unconditional in both
/// pipelines since it costs one branch on the divisor, not on every op.
#[test]
fn add_overflow_faults_debug_wraps_release() {
    let src = "fn main():\n    let max: i64 = 9223372036854775807\n    let one: i64 = 1\n    print(max + one)\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 1, "jit stderr: {stderr}");
    assert!(stderr.contains("[E0713]"), "jit stderr: {stderr}");
    assert!(
        stderr.contains("9223372036854775807 + 1"),
        "jit stderr: {stderr}"
    );

    let (stdout, stderr, code) = run_aot_stdout(&path);
    assert_eq!(code, 0, "aot stderr: {stderr}");
    assert!(stderr.is_empty(), "aot stderr: {stderr}");
    assert_eq!(
        stdout.trim(),
        "-9223372036854775808",
        "aot must wrap, not fault: stdout: {stdout}"
    );

    std::fs::remove_file(&path).ok();
}

/// `i64::MIN - 1` faults in debug, wraps in release, same as the add case.
#[test]
fn sub_overflow_faults_debug_wraps_release() {
    let src = "fn main():\n    let min: i64 = -9223372036854775807 - 1\n    let one: i64 = 1\n    print(min - one)\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 1, "jit stderr: {stderr}");
    assert!(stderr.contains("[E0713]"), "jit stderr: {stderr}");

    let (stdout, stderr, code) = run_aot_stdout(&path);
    assert_eq!(code, 0, "aot stderr: {stderr}");
    assert!(stderr.is_empty(), "aot stderr: {stderr}");
    assert_eq!(
        stdout.trim(),
        "9223372036854775807",
        "aot must wrap, not fault: stdout: {stdout}"
    );

    std::fs::remove_file(&path).ok();
}

/// `i64::MAX * 2` faults in debug, wraps in release, same as the add case.
#[test]
fn mul_overflow_faults_debug_wraps_release() {
    let src = "fn main():\n    let max: i64 = 9223372036854775807\n    let two: i64 = 2\n    print(max * two)\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 1, "jit stderr: {stderr}");
    assert!(stderr.contains("[E0713]"), "jit stderr: {stderr}");

    let (stdout, stderr, code) = run_aot_stdout(&path);
    assert_eq!(code, 0, "aot stderr: {stderr}");
    assert!(stderr.is_empty(), "aot stderr: {stderr}");
    assert_eq!(
        stdout.trim(),
        "-2",
        "aot must wrap, not fault: stdout: {stdout}"
    );

    std::fs::remove_file(&path).ok();
}

/// `i64::MIN / -1` is the classic corner that hardware traps on with no
/// context; it must fault `E0713` cleanly instead, in both pipelines.
#[test]
fn div_min_by_neg_one_faults_both_pipelines() {
    let src = "fn main():\n    let min: i64 = -9223372036854775807 - 1\n    let neg_one: i64 = -1\n    print(min / neg_one)\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 1, "jit stderr: {stderr}");
    assert!(stderr.contains("[E0713]"), "jit stderr: {stderr}");
    assert!(
        stderr.contains("-9223372036854775808 / -1"),
        "jit stderr: {stderr}"
    );

    let (stderr, code) = run_aot(&path);
    assert_eq!(code, 1, "aot stderr: {stderr}");
    assert!(stderr.contains("[E0713]"), "aot stderr: {stderr}");
    assert!(
        stderr.contains("-9223372036854775808 / -1"),
        "aot stderr: {stderr}"
    );

    std::fs::remove_file(&path).ok();
}

/// `i64::MIN % -1` shares the same hardware corner as `/ -1`.
#[test]
fn mod_min_by_neg_one_faults_both_pipelines() {
    let src = "fn main():\n    let min: i64 = -9223372036854775807 - 1\n    let neg_one: i64 = -1\n    print(min % neg_one)\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 1, "jit stderr: {stderr}");
    assert!(stderr.contains("[E0713]"), "jit stderr: {stderr}");
    assert!(
        stderr.contains("-9223372036854775808 % -1"),
        "jit stderr: {stderr}"
    );

    let (stderr, code) = run_aot(&path);
    assert_eq!(code, 1, "aot stderr: {stderr}");
    assert!(stderr.contains("[E0713]"), "aot stderr: {stderr}");
    assert!(
        stderr.contains("-9223372036854775808 % -1"),
        "aot stderr: {stderr}"
    );

    std::fs::remove_file(&path).ok();
}

/// Ordinary in-range arithmetic is unaffected: no fault, correct result.
#[test]
fn in_range_arithmetic_is_silent_both_pipelines() {
    let src = "fn main():\n    let a: i64 = 40\n    let b: i64 = 2\n    print(a + b)\n    print(a - b)\n    print(a * b)\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 0, "jit stderr: {stderr}");
    assert!(stderr.is_empty(), "jit stderr: {stderr}");

    let (stderr, code) = run_aot(&path);
    assert_eq!(code, 0, "aot stderr: {stderr}");
    assert!(stderr.is_empty(), "aot stderr: {stderr}");

    std::fs::remove_file(&path).ok();
}
