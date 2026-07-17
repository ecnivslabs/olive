//! `abort_with` calls `std::process::exit`, so observing a fault-stop and the
//! process still exiting needs a real subprocess -- an in-process JIT call
//! would kill the test runner along with the fault, the same constraint
//! `tests/assert_fault.rs` documents for plain (non-debug) faults.

use pit::tooling::dap::engine::{DebugEvent, StopReason};
use pit::tooling::dap::launch::launch;
use pit::tooling::dap::values;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const CHILD_ENV: &str = "OLIVE_DAP_FAULT_CHILD";
const PATH_ENV: &str = "OLIVE_DAP_FAULT_PATH";

#[test]
fn index_out_of_bounds_stops_with_e0701_then_resume_exits_1() {
    if std::env::var_os(CHILD_ENV).is_some() {
        run_child();
        return;
    }

    // The index has to arrive through an uninlined function parameter
    // (`Optimizer::minimal()` never inlines) rather than a literal, or the
    // compiler's static bounds check catches `xs[9]` as a compile error
    // (E0006) before this ever becomes a runtime fault.
    let src = "fn get(xs: [int], i: int) -> int:\n    return xs[i]\nfn main():\n    let xs = [1, 2, 3]\n    print(get(xs, 9))\n";
    let path = std::env::temp_dir().join(format!("olive_dap_fault_{}.liv", std::process::id()));
    std::fs::write(&path, src).unwrap();

    let exe = std::env::current_exe().unwrap();
    let out = Command::new(&exe)
        .arg("index_out_of_bounds_stops_with_e0701_then_resume_exits_1")
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(CHILD_ENV, "1")
        .env(PATH_ENV, &path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn self as child");

    std::fs::remove_file(&path).ok();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("FAULT_CODE=E0701"),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stdout.contains("FAULT_FN=get"), "stdout: {stdout}");
    assert!(stdout.contains("LOCAL_xs=[1, 2, 3]"), "stdout: {stdout}");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {stdout}\nstderr: {stderr}"
    );
}

/// Launches the same debug session in-process, drives it to the fault, reads
/// the stack and a local, then resumes -- which runs `abort_with` to
/// completion and ends this (child) process with exit code 1. Deliberately
/// never returns after the second `cont()`: the only way this process is
/// meant to end is via that `process::exit`, so nothing here races it.
fn run_child() {
    let path = PathBuf::from(std::env::var(PATH_ENV).expect("path env set by parent"));
    let session = launch(path.to_str().unwrap(), false).expect("launch failed");

    session.cont();
    match session.events().recv().unwrap() {
        DebugEvent::Stopped {
            reason: StopReason::Fault { code, message },
            frame,
        } => {
            println!("FAULT_CODE={code}");
            println!("FAULT_FN={}", frame.name);
            println!("FAULT_MSG={message}");
            let vars = values::frame_variables(&session, 0);
            if let Some(xs) = vars.iter().find(|v| v.name == "xs") {
                println!("LOCAL_xs={}", xs.value);
            }
        }
        other => panic!("expected Stopped(Fault), got {other:?}"),
    }

    session.cont();
    loop {
        std::thread::park();
    }
}
