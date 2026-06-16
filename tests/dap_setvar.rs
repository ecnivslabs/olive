//! Scripted end-to-end session against the real `pit dap` binary, covering
//! `setVariable` and `setExpression`. Split out of `dap_conformance.rs` to
//! keep that file focused on the read-only inspection surface; this one is
//! entirely about the write path. Session/transport plumbing is a deliberate
//! copy of `dap_conformance.rs`'s, matching `dap_headless.rs`'s own existing
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
        "olive_dap_setvar_{}_{tag}_{id}.liv",
        std::process::id()
    ));
    std::fs::write(&path, src).unwrap();
    path
}

fn handshake(session: &mut Session) {
    let seq = session.request("initialize", json!({}));
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], true);
    assert_eq!(resp["body"]["supportsSetVariable"], true);
    assert_eq!(resp["body"]["supportsSetExpression"], true);
}

fn launch_program(session: &mut Session, path: &Path) {
    let seq = session.request("launch", json!({"program": path.to_str().unwrap()}));
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], true, "launch failed: {resp}");
    session.read_event("initialized");
}

fn set_breakpoints(session: &mut Session, path: &Path, breakpoints: &[Value]) {
    let seq = session.request(
        "setBreakpoints",
        json!({"source": {"path": path.to_str().unwrap()}, "breakpoints": breakpoints}),
    );
    let resp = session.read_response(seq);
    let verified: Vec<bool> = resp["body"]["breakpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["verified"].as_bool().unwrap())
        .collect();
    assert!(
        verified.iter().all(|&v| v),
        "breakpoints not verified: {resp}"
    );
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

fn scope_ref(session: &mut Session, frame_id: i64) -> i64 {
    let seq = session.request("scopes", json!({"frameId": frame_id}));
    let resp = session.read_response(seq);
    resp["body"]["scopes"][0]["variablesReference"]
        .as_i64()
        .unwrap()
}

/// The loop naturally runs to `i == 5`; a breakpoint fires once (`i == 2`),
/// `setVariable` jams `i` to 100, and the very next line -- `i = i + 1`,
/// still inside the loop -- reads the patched value back, not the frozen
/// one. Getting `101` instead of `5` is only possible if the write reached
/// the real, running local, not just the debugger's own display of it.
#[test]
fn set_variable_on_a_local_changes_continued_execution() {
    let mut session = Session::start();
    handshake(&mut session);

    let src = "fn main():\n    let mut i = 0\n    while i < 5:\n        i = i + 1\n    print(i)\n";
    let path = write_program(src, "loop");
    launch_program(&mut session, &path);
    set_breakpoints(
        &mut session,
        &path,
        &[json!({"line": 4, "condition": "i == 2"})],
    );
    configuration_done(&mut session);
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "breakpoint");

    let seq = session.request("stackTrace", json!({"threadId": 1}));
    let resp = session.read_response(seq);
    let frame_id = resp["body"]["stackFrames"][0]["id"].as_i64().unwrap();
    let scope = scope_ref(&mut session, frame_id);

    let seq = session.request(
        "setVariable",
        json!({"variablesReference": scope, "name": "i", "value": "100"}),
    );
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], true, "setVariable failed: {resp}");
    assert_eq!(resp["body"]["value"], "100");

    let seq = session.request("continue", json!({"threadId": 1}));
    session.read_response(seq);
    session.read_event("continued");
    session.read_event("exited");

    let stdout = session.stdout_so_far();
    assert!(stdout.contains("101"), "stdout so far: {stdout:?}");
    assert!(!stdout.contains("\n5\n"), "stdout so far: {stdout:?}");

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}

/// `setExpression` on a list element, a dict value, and a struct field: all
/// three are direct heap writes (no reload/mirror involved), so they take
/// effect immediately, not just on the next safepoint.
#[test]
fn set_expression_writes_container_children_immediately() {
    let mut session = Session::start();
    handshake(&mut session);

    let src = concat!(
        "struct Point:\n",
        "    x: int\n",
        "    y: int\n",
        "    fn __init__(self, x: int, y: int):\n",
        "        self.x = x\n",
        "        self.y = y\n",
        "\n",
        "fn main():\n",
        "    let p = Point(1, 2)\n",
        "    let xs = [10, 20, 30]\n",
        "    let d = {\"a\": 1}\n",
        "    print(p.x)\n",
        "    print(xs[1])\n",
        "    print(d[\"a\"])\n",
    );
    let path = write_program(src, "children");
    launch_program(&mut session, &path);
    set_breakpoints(&mut session, &path, &[json!({"line": 12})]);
    configuration_done(&mut session);
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "breakpoint");

    let seq = session.request("stackTrace", json!({"threadId": 1}));
    let resp = session.read_response(seq);
    let frame_id = resp["body"]["stackFrames"][0]["id"].as_i64().unwrap();

    for (expr, value) in [("p.x", "42"), ("xs[1]", "99"), ("d[\"a\"]", "7")] {
        let seq = session.request(
            "setExpression",
            json!({"frameId": frame_id, "expression": expr, "value": value}),
        );
        let resp = session.read_response(seq);
        assert_eq!(
            resp["success"], true,
            "setExpression({expr}) failed: {resp}"
        );
        assert_eq!(resp["body"]["value"], value);
    }

    let seq = session.request("continue", json!({"threadId": 1}));
    session.read_response(seq);
    session.read_event("continued");
    session.read_event("exited");

    let stdout = session.stdout_so_far();
    assert!(stdout.contains("42"), "stdout so far: {stdout:?}");
    assert!(stdout.contains("99"), "stdout so far: {stdout:?}");
    assert!(stdout.contains("7"), "stdout so far: {stdout:?}");

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}

/// `f32` locals reload through a special bitcast path (`translate.rs`) since
/// their raw mirror encoding doesn't match a plain `f64` bit-width: this
/// pins that the write is accepted, renders back correctly, and the session
/// runs to a clean exit afterward. Doesn't assert the patched value through
/// further `f32` arithmetic, a cast, or `print` -- `pit run` confirms
/// (independent of any debug session) that those already have pre-existing
/// bugs of their own, so an assertion built on them would be pinning
/// unrelated compiler defects, not this feature.
#[test]
fn set_variable_on_f32_local_round_trips_and_resumes_cleanly() {
    let mut session = Session::start();
    handshake(&mut session);

    let src = "fn main():\n    let mut f: f32 = 1.0\n    print(1)\n";
    let path = write_program(src, "f32");
    launch_program(&mut session, &path);
    set_breakpoints(&mut session, &path, &[json!({"line": 3})]);
    configuration_done(&mut session);
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "breakpoint");

    let seq = session.request("stackTrace", json!({"threadId": 1}));
    let resp = session.read_response(seq);
    let frame_id = resp["body"]["stackFrames"][0]["id"].as_i64().unwrap();
    let scope = scope_ref(&mut session, frame_id);

    let seq = session.request(
        "setVariable",
        json!({"variablesReference": scope, "name": "f", "value": "2.75"}),
    );
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], true, "setVariable failed: {resp}");
    assert_eq!(resp["body"]["value"], "2.75");

    let seq = session.request("evaluate", json!({"frameId": frame_id, "expression": "f"}));
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["result"], "2.75");

    let seq = session.request("continue", json!({"threadId": 1}));
    session.read_response(seq);
    session.read_event("continued");
    session.read_event("exited");

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}

/// A frame that isn't the topmost one is honestly rejected, not silently
/// accepted and then lost: `write_value` refuses any `frame_idx != 0`.
#[test]
fn set_variable_on_an_outer_frame_local_is_rejected() {
    let mut session = Session::start();
    handshake(&mut session);

    let src = "fn inner():\n    print(\"in inner\")\nfn main():\n    let mut x = 1\n    inner()\n    print(x)\n";
    let path = write_program(src, "outer");
    launch_program(&mut session, &path);
    set_breakpoints(&mut session, &path, &[json!({"line": 2})]);
    configuration_done(&mut session);
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "breakpoint");

    let seq = session.request("stackTrace", json!({"threadId": 1}));
    let resp = session.read_response(seq);
    let frames = resp["body"]["stackFrames"].as_array().unwrap();
    assert_eq!(frames[0]["name"], "inner");
    assert_eq!(frames[1]["name"], "main");
    let outer_frame_id = frames[1]["id"].as_i64().unwrap();
    let outer_scope = scope_ref(&mut session, outer_frame_id);

    let seq = session.request(
        "setVariable",
        json!({"variablesReference": outer_scope, "name": "x", "value": "999"}),
    );
    let resp = session.read_response(seq);
    assert_eq!(
        resp["success"], false,
        "an outer frame's local must not be reported as settable: {resp}"
    );

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}

/// Whole-value replacement of a container is out of scope (would need a
/// general expression evaluator and heap allocator this debugger doesn't
/// have) -- rejected with a clear error, not silently truncated or ignored.
#[test]
fn set_variable_rejects_whole_list_replacement() {
    let mut session = Session::start();
    handshake(&mut session);

    let src = "fn main():\n    let xs = [1, 2, 3]\n    print(xs)\n";
    let path = write_program(src, "composite");
    launch_program(&mut session, &path);
    set_breakpoints(&mut session, &path, &[json!({"line": 3})]);
    configuration_done(&mut session);
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "breakpoint");

    let seq = session.request("stackTrace", json!({"threadId": 1}));
    let resp = session.read_response(seq);
    let frame_id = resp["body"]["stackFrames"][0]["id"].as_i64().unwrap();
    let scope = scope_ref(&mut session, frame_id);

    let seq = session.request(
        "setVariable",
        json!({"variablesReference": scope, "name": "xs", "value": "[4, 5, 6]"}),
    );
    let resp = session.read_response(seq);
    assert_eq!(
        resp["success"], false,
        "whole-list replacement must not be reported as successful: {resp}"
    );

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}
