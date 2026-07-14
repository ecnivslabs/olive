//! Deeply nested statements/expressions must fail with a clean E0200/E0201
//! diagnostic instead of overflowing the native stack (SIGABRT), in every
//! compiler phase: parser, resolver, type checker, and MIR builder. The
//! guards are sized to the debug build's fatter frames, so this runs the
//! debug `pit` binary as a subprocess -- an in-process overflow would kill
//! the test runner.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

fn write_src(src: &str) -> PathBuf {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "olive_deep_nesting_{}_{id}.liv",
        std::process::id()
    ));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(src.as_bytes()).unwrap();
    path
}

fn run_jit(path: &Path) -> (String, String, i32) {
    let out = Command::new(pit_bin())
        .arg("run")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn pit run");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn nested_ifs(depth: usize) -> String {
    let mut src = String::from("let mut x = 0\n");
    for i in 0..depth {
        for _ in 0..i {
            src.push_str("    ");
        }
        src.push_str("if x < 1000000:\n");
    }
    for i in (1..=depth).rev() {
        for _ in 0..i {
            src.push_str("    ");
        }
        src.push_str("x = x + 1\n");
    }
    src.push_str("print(x)\n");
    src
}

fn paren_chain(depth: usize) -> String {
    format!(
        "let y = {}1{}\nprint(y)\n",
        "(".repeat(depth),
        "+1)".repeat(depth)
    )
}

#[track_caller]
fn assert_no_abort(stdout: &str, stderr: &str, code: i32) {
    assert_ne!(
        code,
        -1,
        "compiler crashed on deep nesting (killed by signal):\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn stmt_nesting_within_limit_runs_cleanly() {
    let path = write_src(&nested_ifs(50));
    let (stdout, stderr, code) = run_jit(&path);
    assert_eq!(code, 0, "expected clean run:\n{stderr}");
    assert_eq!(stdout.trim(), "50");
    std::fs::remove_file(&path).ok();
}

#[test]
fn stmt_nesting_over_limit_diagnoses_instead_of_aborting() {
    for depth in [101, 300] {
        let path = write_src(&nested_ifs(depth));
        let (stdout, stderr, code) = run_jit(&path);
        assert_no_abort(&stdout, &stderr, code);
        assert_ne!(code, 0);
        let flat = format!("{stdout}{stderr}");
        assert!(
            flat.contains("nested too deeply"),
            "depth {depth}: expected a nesting diagnostic, got:\n{flat}"
        );
        std::fs::remove_file(&path).ok();
    }
}

#[test]
fn expr_nesting_within_limit_runs_cleanly() {
    let path = write_src(&paren_chain(50));
    let (stdout, stderr, code) = run_jit(&path);
    assert_eq!(code, 0, "expected clean run:\n{stderr}");
    assert_eq!(stdout.trim(), "51");
    std::fs::remove_file(&path).ok();
}

#[test]
fn expr_nesting_over_limit_diagnoses_instead_of_aborting() {
    for depth in [101, 400, 2000] {
        let path = write_src(&paren_chain(depth));
        let (stdout, stderr, code) = run_jit(&path);
        assert_no_abort(&stdout, &stderr, code);
        assert_ne!(code, 0);
        let flat = format!("{stdout}{stderr}");
        assert!(
            flat.contains("nested too deeply"),
            "depth {depth}: expected a nesting diagnostic, got:\n{flat}"
        );
        std::fs::remove_file(&path).ok();
    }
}
