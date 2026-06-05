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

/// `u64::MAX + 1` shares the exact same checked/wraps split as the `i64`
/// cases above: `emit_checked_arith` picks `uadd_overflow` over
/// `sadd_overflow` by the operand's static type (`is_u64_op`), same `checked`
/// gate. `u64::MAX` has no literal form (the lexer parses magnitude before
/// sign and rejects anything over `i64::MAX`), so it's built from two
/// in-range steps: `i64::MAX` widened to `u64`, doubled, plus one.
#[test]
fn add_overflow_u64_faults_debug_wraps_release() {
    let src = "fn main():\n    let a: u64 = 9223372036854775807\n    let max: u64 = a * 2 + 1\n    let one: u64 = 1\n    print(max + one)\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 1, "jit stderr: {stderr}");
    assert!(stderr.contains("[E0713]"), "jit stderr: {stderr}");
    assert!(
        stderr.contains("18446744073709551615 + 1"),
        "jit stderr: {stderr}"
    );

    let (stdout, stderr, code) = run_aot_stdout(&path);
    assert_eq!(code, 0, "aot stderr: {stderr}");
    assert!(stderr.is_empty(), "aot stderr: {stderr}");
    assert_eq!(
        stdout.trim(),
        "0",
        "aot must wrap, not fault: stdout: {stdout}"
    );

    std::fs::remove_file(&path).ok();
}

/// `u64` has no negative side to wrap into, so its underflow corner is `0 -
/// 1`, not `MIN - 1`.
#[test]
fn sub_overflow_u64_faults_debug_wraps_release() {
    let src = "fn main():\n    let zero: u64 = 0\n    let one: u64 = 1\n    print(zero - one)\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 1, "jit stderr: {stderr}");
    assert!(stderr.contains("[E0713]"), "jit stderr: {stderr}");
    assert!(stderr.contains("0 - 1"), "jit stderr: {stderr}");

    let (stdout, stderr, code) = run_aot_stdout(&path);
    assert_eq!(code, 0, "aot stderr: {stderr}");
    assert!(stderr.is_empty(), "aot stderr: {stderr}");
    assert_eq!(
        stdout.trim(),
        "18446744073709551615",
        "aot must wrap, not fault: stdout: {stdout}"
    );

    std::fs::remove_file(&path).ok();
}

/// `u64::MAX * 2` faults in debug, wraps in release, same as the add case.
#[test]
fn mul_overflow_u64_faults_debug_wraps_release() {
    let src = "fn main():\n    let a: u64 = 9223372036854775807\n    let max: u64 = a * 2 + 1\n    let two: u64 = 2\n    print(max * two)\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 1, "jit stderr: {stderr}");
    assert!(stderr.contains("[E0713]"), "jit stderr: {stderr}");
    assert!(
        stderr.contains("18446744073709551615 * 2"),
        "jit stderr: {stderr}"
    );

    let (stdout, stderr, code) = run_aot_stdout(&path);
    assert_eq!(code, 0, "aot stderr: {stderr}");
    assert!(stderr.is_empty(), "aot stderr: {stderr}");
    assert_eq!(
        stdout.trim(),
        "18446744073709551614",
        "aot must wrap, not fault: stdout: {stdout}"
    );

    std::fs::remove_file(&path).ok();
}

/// In-range `u64` arithmetic is unaffected, mirroring the `i64` case, and
/// also exercises `print`/`str`/f-string `u64` formatting near `i64::MAX`
/// (a value whose top bit is unset, so it would print correctly even
/// through the signed formatter -- the real regression coverage for that
/// formatter split is `u64_max_prints_and_formats_unsigned` below).
#[test]
fn in_range_u64_arithmetic_is_silent_both_pipelines() {
    let src = "fn main():\n    let a: u64 = 40\n    let b: u64 = 2\n    print(a + b)\n    print(a - b)\n    print(a * b)\n";
    let path = write_src(src);

    let (stderr, code) = run_jit(&path);
    assert_eq!(code, 0, "jit stderr: {stderr}");
    assert!(stderr.is_empty(), "jit stderr: {stderr}");

    let (stderr, code) = run_aot(&path);
    assert_eq!(code, 0, "aot stderr: {stderr}");
    assert!(stderr.is_empty(), "aot stderr: {stderr}");

    std::fs::remove_file(&path).ok();
}

/// `1u64 << 63` has its top bit set, so a signed formatter reads it as
/// `i64::MIN`. `print`, `str()`, and f-string interpolation each had their
/// own dispatch to the unsigned formatter, and the `str`/f-string ones had
/// their own separate release-only regression on top: the release
/// optimizer's constant folder collapses `one << 63` into a bare
/// `Constant::Int`, which carries no u64 tag, so a dispatch that
/// re-derives the type from the call's operand at codegen time silently
/// falls back to the signed formatter. Both `str`/f-string sites now name
/// the unsigned formatter at MIR-build time instead (matching how `print`
/// already worked), before the fold ever runs. `--release` is what
/// exercises the fold; a debug AOT build takes the same dispatch code but
/// never folds the shift, so both are checked here for full coverage of
/// the fix.
#[test]
fn u64_max_prints_and_formats_unsigned_both_pipelines() {
    let src = "fn main():\n    let one: u64 = 1\n    let max: u64 = one << 63\n    print(max)\n    print(str(max))\n    print(f\"{max}\")\n";
    let path = write_src(src);
    let expected = "9223372036854775808\n9223372036854775808\n9223372036854775808\n";

    let (stdout, stderr, code) = run_aot_stdout(&path);
    assert_eq!(code, 0, "debug aot stderr: {stderr}");
    assert_eq!(stdout, expected, "debug aot stdout: {stdout}");

    let out_bin = path.with_extension("bin");
    let build = std::process::Command::new(pit_bin())
        .arg("build")
        .arg("--release")
        .arg(&path)
        .arg("-o")
        .arg(&out_bin)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn pit build --release");
    assert!(
        build.status.success(),
        "release build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let out = std::process::Command::new(&out_bin)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn release binary");
    std::fs::remove_file(&out_bin).ok();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        expected,
        "release aot stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    std::fs::remove_file(&path).ok();
}
