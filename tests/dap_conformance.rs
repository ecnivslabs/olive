//! Scripted end-to-end session against the real `pit dap` binary over its
//! actual stdio transport, cloned from `tests/lsp_conformance.rs`'s pattern
//! (a framed-stdio subprocess, request/response matched by sequence number
//! rather than JSON-RPC id). Covers launch, breakpoints, stepping, and the
//! variable inspection requests (scopes, variables, evaluate, exceptionInfo).

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
    /// Every frame ever read, event or response, in arrival order. A real
    /// client keeps everything it receives rather than discarding whatever
    /// it wasn't in the middle of waiting for; output events in particular
    /// can arrive interleaved with an unrelated request's response.
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

    /// Reads frames until `pred` matches one, returning every frame seen
    /// (the matching one last). Any raw program byte landing on the wire
    /// mid-frame would desync `Content-Length` parsing and fail loudly
    /// here, so a clean read through a print-heavy program is itself the
    /// proof that redirect.rs never let output leak past the protocol.
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

    /// Every `output` event's text seen so far, across every `read_*` call,
    /// concatenated in arrival order.
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
        "olive_dap_conformance_{}_{tag}_{id}.liv",
        std::process::id()
    ));
    std::fs::write(&path, src).unwrap();
    path
}

fn handshake(session: &mut Session) {
    let seq = session.request("initialize", json!({}));
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], true);
    assert_eq!(resp["body"]["supportsConfigurationDoneRequest"], true);
}

fn launch_program(session: &mut Session, path: &Path, stop_on_entry: bool) {
    let seq = session.request(
        "launch",
        json!({"program": path.to_str().unwrap(), "stopOnEntry": stop_on_entry}),
    );
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], true, "launch failed: {resp}");
    session.read_event("initialized");
}

fn set_breakpoints(session: &mut Session, path: &Path, lines: &[i64]) {
    let breakpoints: Vec<Value> = lines.iter().map(|&l| json!({"line": l})).collect();
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

#[test]
fn breakpoint_step_continue_output_and_exit() {
    let mut session = Session::start();
    handshake(&mut session);

    let src = "fn helper():\n    print(\"in helper\")\nfn main():\n    print(\"start\")\n    helper()\n    print(\"end\")\n";
    let path = write_program(src, "core");
    launch_program(&mut session, &path, false);
    set_breakpoints(&mut session, &path, &[4]);
    configuration_done(&mut session);

    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "breakpoint");

    let seq = session.request("threads", json!({}));
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["threads"][0]["id"], 1);

    let seq = session.request("stackTrace", json!({"threadId": 1}));
    let resp = session.read_response(seq);
    let frames = resp["body"]["stackFrames"].as_array().unwrap();
    assert_eq!(frames[0]["name"], "main");
    assert_eq!(frames[0]["line"], 4);

    let seq = session.request("next", json!({"threadId": 1}));
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], true);
    session.read_event("continued");
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "step");

    let seq = session.request("continue", json!({"threadId": 1}));
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], true);
    session.read_event("continued");

    let exited = session.read_event("exited");
    let stdout_text = session.stdout_so_far();
    assert!(
        stdout_text.contains("start"),
        "stdout so far: {stdout_text:?}"
    );
    assert!(
        stdout_text.contains("in helper"),
        "stdout so far: {stdout_text:?}"
    );
    assert!(
        stdout_text.contains("end"),
        "stdout so far: {stdout_text:?}"
    );
    assert_eq!(exited["body"]["exitCode"], 0);

    session.read_event("terminated");
    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn scopes_and_variables_round_trip_with_nested_expansion() {
    let mut session = Session::start();
    handshake(&mut session);

    let path = write_program(
        "fn main():\n    let xs = [1, 2, 3]\n    print(xs)\n",
        "scopes",
    );
    launch_program(&mut session, &path, false);
    set_breakpoints(&mut session, &path, &[3]);
    configuration_done(&mut session);
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "breakpoint");

    let seq = session.request("stackTrace", json!({"threadId": 1}));
    let resp = session.read_response(seq);
    let frame_id = resp["body"]["stackFrames"][0]["id"].as_i64().unwrap();

    let seq = session.request("scopes", json!({"frameId": frame_id}));
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["scopes"][0]["name"], "Locals");
    let scope_ref = resp["body"]["scopes"][0]["variablesReference"]
        .as_i64()
        .unwrap();

    let seq = session.request("variables", json!({"variablesReference": scope_ref}));
    let resp = session.read_response(seq);
    let vars = resp["body"]["variables"].as_array().unwrap();
    let xs = vars.iter().find(|v| v["name"] == "xs").unwrap();
    assert_eq!(xs["value"], "[1, 2, 3]");
    let xs_ref = xs["variablesReference"].as_i64().unwrap();
    assert!(xs_ref > 0);

    let seq = session.request("variables", json!({"variablesReference": xs_ref}));
    let resp = session.read_response(seq);
    let children = resp["body"]["variables"].as_array().unwrap();
    let values: Vec<&str> = children
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert_eq!(values, vec!["1", "2", "3"]);

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn evaluate_resolves_a_path_expression() {
    let mut session = Session::start();
    handshake(&mut session);

    let src = "struct Point:\n    x: int\n    y: int\n    fn __init__(self, x: int, y: int):\n        self.x = x\n        self.y = y\n\nfn main():\n    let p = Point(1, 2)\n    let xs = [1, 2, 3]\n    print(p)\n    print(xs)\n";
    let path = write_program(src, "eval");
    launch_program(&mut session, &path, false);
    set_breakpoints(&mut session, &path, &[11]);
    configuration_done(&mut session);
    session.read_event("stopped");

    let seq = session.request("evaluate", json!({"expression": "p.x", "frameId": 0}));
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["result"], "1");

    let seq = session.request("evaluate", json!({"expression": "xs[1]", "frameId": 0}));
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["result"], "2");

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn fault_program_stops_with_exception_then_exception_info() {
    let mut session = Session::start();
    handshake(&mut session);

    let src = "fn get(xs: [int], i: int) -> int:\n    return xs[i]\nfn main():\n    let xs = [1, 2, 3]\n    print(get(xs, 9))\n";
    let path = write_program(src, "fault");
    launch_program(&mut session, &path, false);
    configuration_done(&mut session);

    let stopped = session.read_event("stopped");
    assert_eq!(stopped["body"]["reason"], "exception");

    let seq = session.request("exceptionInfo", json!({"threadId": 1}));
    let resp = session.read_response(seq);
    assert_eq!(resp["body"]["exceptionId"], "E0701");

    // Resuming past a fault means `abort_with` runs to completion and the
    // whole `pit dap` process exits 1 -- no graceful disconnect handshake
    // here, so just observe the process die.
    session.request("continue", json!({"threadId": 1}));
    let status = session.child.wait().expect("pit dap exits");
    assert_eq!(status.code(), Some(1));
    std::fs::remove_file(&path).ok();
}

#[test]
fn bad_evaluate_expression_returns_error_response() {
    let mut session = Session::start();
    handshake(&mut session);

    let path = write_program("fn main():\n    let x = 1\n    print(x)\n", "badeval");
    launch_program(&mut session, &path, false);
    set_breakpoints(&mut session, &path, &[3]);
    configuration_done(&mut session);
    session.read_event("stopped");

    let seq = session.request("evaluate", json!({"expression": "nope", "frameId": 0}));
    let resp = session.read_response(seq);
    assert_eq!(resp["success"], false);
    assert!(
        resp["message"]
            .as_str()
            .unwrap()
            .contains("no such variable")
    );

    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn request_while_running_returns_error_not_hang() {
    let mut session = Session::start();
    handshake(&mut session);

    let src = "fn main():\n    let mut i = 0\n    while i < 100000000:\n        i = i + 1\n    print(i)\n";
    let path = write_program(src, "running");
    launch_program(&mut session, &path, false);
    configuration_done(&mut session);

    let seq = session.request("evaluate", json!({"expression": "i", "frameId": 0}));
    let resp = session.read_response(seq);
    assert_eq!(
        resp["success"], false,
        "evaluate while running should error, not hang: {resp}"
    );

    session.request("pause", json!({}));
    session.read_event("stopped");
    disconnect(&mut session);
    std::fs::remove_file(&path).ok();
}
