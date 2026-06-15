//! Structural proof that a plain `pit run` never carries the debugger's MIR
//! instrumentation: `mir::debug_hooks::instrument` only runs inside
//! `tooling::dap::launch::launch`, so an ordinary run's MIR should never
//! contain a call to any `__olive_debug_` hook. This is the structural half
//! of the zero-overhead claim; `benchmark/scripts/dap_overhead.sh` is the
//! empirical half, and both are required together.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

fn write_program(src: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "olive_dap_zero_overhead_{}_{id}.liv",
        std::process::id()
    ));
    std::fs::write(&path, src).unwrap();
    path
}

const PROBE: &str = "fn add(a: int, b: int) -> int:\n    return a + b\n\nfn main():\n    let mut total = 0\n    let mut i = 0\n    while i < 5:\n        total = add(total, i)\n        i = i + 1\n    print(total)\n";

#[test]
fn plain_run_produces_the_expected_output() {
    let path = write_program(PROBE);
    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&path)
        .arg("--jit")
        .stdin(Stdio::null())
        .output()
        .expect("run pit");
    std::fs::remove_file(&path).ok();

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "10");
}

#[test]
fn plain_run_mir_never_contains_a_debug_hook_call() {
    let path = write_program(PROBE);
    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&path)
        .arg("--jit")
        .arg("--emit-mir")
        .stdin(Stdio::null())
        .output()
        .expect("run pit");
    std::fs::remove_file(&path).ok();

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mir = String::from_utf8_lossy(&out.stdout);
    assert!(
        !mir.contains("__olive_debug_"),
        "plain run's MIR carries a debug hook call, hook-off overhead is not structurally zero:\n{mir}"
    );
}
