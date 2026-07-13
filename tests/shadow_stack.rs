//! Roadmap E13.2: a fault mid-call-chain prints the frames back to `main` in
//! the debug (`pit run`) pipeline, and nothing extra in AOT release. Runs as
//! a subprocess for the same reason as `assert_fault.rs`: a fault calls
//! `std::process::exit`.

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
        "olive_shadow_stack_{}_{id}.liv",
        std::process::id()
    ));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(src.as_bytes()).unwrap();
    path
}

const CHAIN_SRC: &str =
    "fn c():\n    assert 1 == 2\n\nfn b():\n    c()\n\nfn a():\n    b()\n\nfn main():\n    a()\n";

/// A three-deep call chain (`a` -> `b` -> `c`, fault in `c`) prints exactly
/// three frames in debug, innermost first.
#[test]
fn three_deep_chain_prints_three_frames_in_debug() {
    let path = write_src(CHAIN_SRC);

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(stderr.contains("stack (innermost first):"), "{stderr}");
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.trim_start().starts_with("│") && l.contains(": "))
        .filter(|l| l.contains("(") && l.contains(".liv:"))
        .collect();
    assert_eq!(
        lines.len(),
        3,
        "expected exactly 3 frames, got {}: {stderr}",
        lines.len()
    );
    assert!(
        lines[0].contains(": c ("),
        "innermost should be c: {stderr}"
    );
    assert!(lines[1].contains(": b ("), "next should be b: {stderr}");
    assert!(
        lines[2].contains(": a ("),
        "outermost should be a: {stderr}"
    );

    std::fs::remove_file(&path).ok();
}

/// The same fault under AOT release shows no stack section at all.
#[test]
fn no_stack_section_in_aot_release() {
    let path = write_src(CHAIN_SRC);
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
        "AOT build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let out = Command::new(&out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn built binary");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[E0712]"), "{stderr}");
    assert!(!stderr.contains("stack (innermost first):"), "{stderr}");

    std::fs::remove_file(&path).ok();
    std::fs::remove_file(&out_bin).ok();
}

/// A leaf-level fault (no intervening calls) shows no stack section either --
/// there is nothing to report.
#[test]
fn no_stack_section_for_leaf_fault() {
    let path = write_src("fn main():\n    assert 1 == 2\n");

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[E0712]"), "{stderr}");
    assert!(!stderr.contains("stack (innermost first):"), "{stderr}");

    std::fs::remove_file(&path).ok();
}
