//! Tearing down a session (`quit`) must never hang, even when it races a
//! breakpoint the debuggee hasn't reached yet: a `continue` immediately
//! followed by `quit`, with no wait for a `stopped` event in between, is a
//! valid sequence a real agent can send (`continue` is documented as
//! fire-and-forget). `DebugSession::drop` used to send a single resume
//! signal and then block on joining the debuggee thread; if the debuggee
//! hit the breakpoint moments later, it parked with no one left to resume
//! it, and the whole process hung forever.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

fn write_program(src: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "olive_dap_teardown_{}_{id}.liv",
        std::process::id()
    ));
    std::fs::write(&path, src).unwrap();
    path
}

/// A long-enough loop that the debuggee thread almost certainly hasn't
/// reached line 2 yet by the time the un-synchronized `quit` below runs.
const SRC: &str =
    "fn main():\n    let mut i = 0\n    while i < 200000:\n        i = i + 1\n    print(i)\n";

#[test]
fn quit_racing_an_unreached_breakpoint_does_not_hang() {
    let path = write_program(SRC);
    let liv = path.to_str().unwrap();

    let mut child = Command::new(pit_bin())
        .arg("debug")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pit debug");

    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, r#"{{"id":1,"cmd":"launch","program":"{liv}"}}"#).unwrap();
        writeln!(
            stdin,
            r#"{{"id":2,"cmd":"break","source":"{liv}","lines":[2]}}"#
        )
        .unwrap();
        writeln!(stdin, r#"{{"cmd":"continue"}}"#).unwrap();
        writeln!(stdin, r#"{{"id":3,"cmd":"quit"}}"#).unwrap();
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            assert!(status.success(), "pit debug exited non-zero: {status:?}");
            std::fs::remove_file(&path).ok();
            return;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            std::fs::remove_file(&path).ok();
            panic!("pit debug hung on quit racing an unreached breakpoint");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
