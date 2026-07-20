//! Scripted end-to-end session against the real `pit dap` binary, covering
//! `dataBreakpointInfo`/`setDataBreakpoints`. Session/transport plumbing is
//! a deliberate copy of `dap_setvar.rs`'s, matching this codebase's existing
//! precedent of each protocol test file being self-contained.

use serde_json::{Value, json};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

fn write_message(stdin: &mut ChildStdin, value: &Value) {
    let body = serde_json::to_vec(value).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.flush().unwrap();
}

fn read_message(reader: &mut BufReader<ChildStdout>) -> Value {
    let mut content_length = None;
    loop {
        let mut line = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            reader.read_exact(&mut byte).expect("read header byte");
            line.push(byte[0]);
            if line.ends_with(b"\r\n") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&line);
        let trimmed = text.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(rest.trim().parse::<usize>().unwrap());
        }
    }
    let len = content_length.expect("Content-Length header present");
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).expect("read body");
    serde_json::from_slice(&body).expect("valid JSON")
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    seq: i64,
    log: Vec<Value>,
}

impl Session {
    fn start() -> Self {
        let mut child = Command::new(pit_bin())
            .arg("dap")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn pit dap");
        let stdin = child.stdin.take().unwrap();
        let reader = BufReader::new(child.stdout.take().unwrap());
        Session {
            child,
            stdin,
            reader,
            seq: 1,
            log: Vec::new(),
        }
    }

    fn request(&mut self, command: &str, args: Value) -> i64 {
        let seq = self.seq;
        self.seq += 1;
        write_message(
            &mut self.stdin,
            &json!({"seq": seq, "type": "request", "command": command, "arguments": args}),
        );
        seq
    }

    fn read_until(&mut self, mut pred: impl FnMut(&Value) -> bool) -> Vec<Value> {
        let mut out = Vec::new();
        loop {
            let msg = read_message(&mut self.reader);
            self.log.push(msg.clone());
            let done = pred(&msg);
            out.push(msg);
            if done {
                return out;
            }
        }
    }

    fn read_response(&mut self, request_seq: i64) -> Value {
        self.read_until(|m| {
            m["type"] == "response" && m["request_seq"].as_i64() == Some(request_seq)
        })
        .pop()
        .unwrap()
    }

    fn read_event(&mut self, name: &str) -> Value {
        self.read_until(|m| m["type"] == "event" && m["event"] == name)
            .pop()
            .unwrap()
    }

    fn stdout_so_far(&self) -> String {
        self.log
            .iter()
            .filter(|m| {
                m["type"] == "event" && m["event"] == "output" && m["body"]["category"] == "stdout"
            })
            .map(|m| m["body"]["output"].as_str().unwrap_or(""))
            .collect()
    }
}

fn write_program(src: &str, tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "olive_dap_watch_{}_{tag}_{id}.liv",
        std::process::id()
    ));
    std::fs::write(&path, src).unwrap();
    path
}

fn handshake(session: &mut Session) {
    let seq = session.request("initialize", json!({}));
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], true);
    assert_eq!(resp["body"]["supportsDataBreakpoints"], true);
}

fn launch_stop_on_entry(session: &mut Session, path: &Path) {
    let seq = session.request(
        "launch",
        json!({"program": path.to_str().unwrap(), "stopOnEntry": true}),
    );
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], true, "launch failed: {resp}");
    session.read_event("initialized");
}

fn configuration_done(session: &mut Session) {
    let seq = session.request("configurationDone", json!({}));
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], true);
}

fn disconnect(session: &mut Session) {
    let seq = session.request("disconnect", json!({}));
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], true);
    let status = session.child.wait().expect("pit dap exits");
    assert!(status.success());
}

fn frame_id(session: &mut Session) -> i64 {
    stack_top(session).0
}

/// `(frameId, line)` of the innermost frame -- the real DAP `stopped` event
/// body carries only `reason`/`threadId` (see `server.rs::stopped_body`),
/// not line/fn, so a test that wants those has to ask `stackTrace`.
fn stack_top(session: &mut Session) -> (i64, i64) {
    let seq = session.request("stackTrace", json!({"threadId": 1}));
    let resp = session.read_response(seq);
    let frame = &resp["body"]["stackFrames"][0];
    (
        frame["id"].as_i64().unwrap(),
        frame["line"].as_i64().unwrap(),
    )
}

const SRC: &str = "fn main():\n    let mut total = 0\n    let mut i = 0\n    while i < 5:\n        total = total + i\n        i = i + 1\n    print(total)\n";

#[test]
fn data_breakpoint_stops_on_every_write_to_a_watched_local() {
    let mut session = Session::start();
    handshake(&mut session);

    let path = write_program(SRC, "writes");
    launch_stop_on_entry(&mut session, &path);
    configuration_done(&mut session);
    session.read_event("stopped"); // entry

    let fid = frame_id(&mut session);
    let seq = session.request(
        "dataBreakpointInfo",
        json!({"frameId": fid, "name": "total"}),
    );
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["accessTypes"], json!(["write"]));
    let data_id = resp["body"]["dataId"].as_str().unwrap().to_string();

    let seq = session.request(
        "setDataBreakpoints",
        json!({"breakpoints": [{"dataId": data_id}]}),
    );
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["breakpoints"][0]["verified"], true);

    let seq = session.request("continue", json!({"threadId": 1}));
    session.read_response(seq);
    session.read_event("continued");

    // First write: `let mut total = 0` on line 2.
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "data breakpoint");
    let fid = frame_id(&mut session);
    let seq = session.request("evaluate", json!({"expression": "total", "frameId": fid}));
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["result"], "0");

    // Second write: the loop body's `total = total + i` on line 5, i == 0.
    // No line breakpoint coexists with the watch in this function, so the
    // reported line deliberately stays wherever it last was (2, from the
    // entry stop) rather than tracking to 5 -- see `debug_should_check_stmt`
    // in hooks.rs for why keeping it live on every statement was reverted
    // (it corrupted ownership state under real load).
    let seq = session.request("continue", json!({"threadId": 1}));
    session.read_response(seq);
    session.read_event("continued");
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "data breakpoint");
    let (_, line) = stack_top(&mut session);
    assert_eq!(line, 2);

    // Drop the watch and let it run to completion: 0+1+2+3+4 == 10.
    let seq = session.request("setDataBreakpoints", json!({"breakpoints": []}));
    session.read_response(seq);
    let seq = session.request("continue", json!({"threadId": 1}));
    session.read_response(seq);
    session.read_event("continued");
    session.read_event("exited");
    let stdout = session.stdout_so_far();
    assert!(stdout.contains("10"), "stdout so far: {stdout:?}");

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn data_breakpoint_condition_filters_which_write_stops() {
    let mut session = Session::start();
    handshake(&mut session);

    let path = write_program(SRC, "condition");
    launch_stop_on_entry(&mut session, &path);
    configuration_done(&mut session);
    session.read_event("stopped"); // entry

    let fid = frame_id(&mut session);
    let seq = session.request(
        "dataBreakpointInfo",
        json!({"frameId": fid, "name": "total"}),
    );
    let resp = session.read_response(seq);
    let data_id = resp["body"]["dataId"].as_str().unwrap().to_string();

    let seq = session.request(
        "setDataBreakpoints",
        json!({"breakpoints": [{"dataId": data_id, "condition": "total >= 6"}]}),
    );
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["breakpoints"][0]["verified"], true);

    let seq = session.request("continue", json!({"threadId": 1}));
    session.read_response(seq);
    session.read_event("continued");

    // Every earlier write (0, 0, 1, 3) is skipped; the first write where
    // `total` reaches 6 (i == 3, iteration 4) is the only one that stops.
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "data breakpoint");
    let fid = frame_id(&mut session);
    let seq = session.request("evaluate", json!({"expression": "total", "frameId": fid}));
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["result"], "6");

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}

/// Regression coverage for a real bug found while benchmarking: a watched
/// cell lives in a specific function, but `debug_store` only exists on that
/// function's `$debug` variant. Variant dispatch is decided per-call, so a
/// function watched but never independently line-breakpointed would revert
/// to its clean (uninstrumented) body the moment it's called again --
/// silently dropping the watch. Calling the watched function a *second*
/// time, after removing the line breakpoint that was only there to get an
/// initial frame reference, is what actually exercises that path.
#[test]
fn data_breakpoint_survives_a_fresh_call_with_no_line_breakpoint_left_in_the_function() {
    let mut session = Session::start();
    handshake(&mut session);

    let src = "fn helper() -> int:\n    let mut x = 0\n    x = 5\n    return x\nfn main():\n    print(helper())\n    print(helper())\n";
    let path = write_program(src, "fresh_call");
    launch_stop_on_entry(&mut session, &path);

    let seq = session.request(
        "setBreakpoints",
        json!({"source": {"path": path.to_str().unwrap()}, "breakpoints": [{"line": 3}]}),
    );
    session.read_response(seq);
    configuration_done(&mut session);
    session.read_event("stopped"); // entry

    let seq = session.request("continue", json!({"threadId": 1}));
    session.read_response(seq);
    session.read_event("continued");
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "breakpoint");

    let fid = frame_id(&mut session);
    let seq = session.request("dataBreakpointInfo", json!({"frameId": fid, "name": "x"}));
    let resp = session.read_response(seq);
    let data_id = resp["body"]["dataId"].as_str().unwrap().to_string();
    let seq = session.request(
        "setDataBreakpoints",
        json!({"breakpoints": [{"dataId": data_id}]}),
    );
    session.read_response(seq);

    // Remove the line breakpoint: only the watch is left owning `helper`.
    let seq = session.request(
        "setBreakpoints",
        json!({"source": {"path": path.to_str().unwrap()}, "breakpoints": []}),
    );
    session.read_response(seq);

    // Still inside the first call: the line-3 write to `x` this same call
    // was already parked at.
    let seq = session.request("continue", json!({"threadId": 1}));
    session.read_response(seq);
    session.read_event("continued");
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "data breakpoint");
    let fid = frame_id(&mut session);
    let seq = session.request("evaluate", json!({"expression": "x", "frameId": fid}));
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["result"], "5");

    // A fresh, second call to `helper()` -- no line breakpoint owns it
    // anymore, so this only stops if the watch alone kept it instrumented.
    let seq = session.request("continue", json!({"threadId": 1}));
    session.read_response(seq);
    session.read_event("continued");
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "data breakpoint");
    let fid = frame_id(&mut session);
    let seq = session.request("evaluate", json!({"expression": "x", "frameId": fid}));
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["result"], "0");

    let seq = session.request("setDataBreakpoints", json!({"breakpoints": []}));
    session.read_response(seq);
    let seq = session.request("continue", json!({"threadId": 1}));
    session.read_response(seq);
    session.read_event("continued");
    session.read_event("exited");
    let stdout = session.stdout_so_far();
    assert_eq!(stdout, "5\n5\n");

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}

/// Regression coverage for a real crash found while benchmarking: forcing
/// `debug_stmt` (and the reload it triggers for every named local) to run
/// on every statement whenever any watch was active -- reverted, see
/// `debug_should_check_stmt`'s doc comment -- corrupted a loop that
/// reallocates and moves a list each iteration (`E0707` stale reference,
/// reproduced reliably within a few hundred iterations of the matrix_mult
/// benchmark). This is the same shape at a size small enough to run as a
/// unit test: a data breakpoint on a variable unrelated to the loop's list
/// churn must never trip a fault, no matter how many times it fires.
#[test]
fn data_breakpoint_on_an_unrelated_local_does_not_corrupt_a_reallocating_loop() {
    let mut session = Session::start();
    handshake(&mut session);

    let src = "fn build(n: int):\n    let mut a = list_new(n)\n    let mut i = 0\n    while i < n:\n        let mut row = list_new(n)\n        let mut j = 0\n        while j < n:\n            row[j] = i + j\n            j = j + 1\n        a[i] = row\n        i = i + 1\n    return a\nfn main():\n    let result = build(4)\n    print(result[0][0])\n";
    let path = write_program(src, "realloc_loop");
    launch_stop_on_entry(&mut session, &path);

    let seq = session.request(
        "setBreakpoints",
        json!({"source": {"path": path.to_str().unwrap()}, "breakpoints": [{"line": 2}]}),
    );
    session.read_response(seq);
    configuration_done(&mut session);
    session.read_event("stopped"); // entry

    let seq = session.request("continue", json!({"threadId": 1}));
    session.read_response(seq);
    session.read_event("continued");
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "breakpoint");

    let fid = frame_id(&mut session);
    let seq = session.request("dataBreakpointInfo", json!({"frameId": fid, "name": "i"}));
    let resp = session.read_response(seq);
    let data_id = resp["body"]["dataId"].as_str().unwrap().to_string();
    let seq = session.request(
        "setDataBreakpoints",
        json!({"breakpoints": [{"dataId": data_id}]}),
    );
    session.read_response(seq);
    let seq = session.request(
        "setBreakpoints",
        json!({"source": {"path": path.to_str().unwrap()}, "breakpoints": []}),
    );
    session.read_response(seq);

    // Resume past every remaining watch hit until the program exits --
    // the exact hit count depends on `i`'s write sites, not worth pinning.
    loop {
        let seq = session.request("continue", json!({"threadId": 1}));
        session.read_response(seq);
        let msg = session.read_until(|m| {
            m["type"] == "event" && (m["event"] == "stopped" || m["event"] == "exited")
        });
        let last = msg.last().unwrap();
        if last["event"] == "exited" {
            break;
        }
        assert_eq!(
            last["body"]["reason"], "data breakpoint",
            "unexpected stop: {last}"
        );
    }

    let stdout = session.stdout_so_far();
    assert_eq!(stdout, "0\n");

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn data_breakpoint_info_rejects_container_children_and_invalid_data_ids() {
    let mut session = Session::start();
    handshake(&mut session);

    let path = write_program(SRC, "invalid");
    launch_stop_on_entry(&mut session, &path);
    configuration_done(&mut session);
    session.read_event("stopped"); // entry

    let fid = frame_id(&mut session);
    let seq = session.request(
        "dataBreakpointInfo",
        json!({"frameId": fid, "name": "total", "variablesReference": 12345}),
    );
    let resp = session.read_response(seq);
    assert!(resp["body"]["dataId"].is_null(), "resp: {resp}");

    let seq = session.request(
        "setDataBreakpoints",
        json!({"breakpoints": [{"dataId": "not-a-real-id"}]}),
    );
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["breakpoints"][0]["verified"], false);
    assert!(resp["body"]["breakpoints"][0]["message"].is_string());

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}
