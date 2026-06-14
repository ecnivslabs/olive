//! Scripted stdin session against the real `pit debug <file>` binary: plain
//! newline-delimited JSON, no framing, the agent-facing contract this file
//! exists to pin down (the schema in `dap.md`'s D8 section is the source of
//! truth for field names).

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

fn write_program(src: &str, tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "olive_dap_headless_{}_{tag}_{id}.liv",
        std::process::id()
    ));
    std::fs::write(&path, src).unwrap();
    path
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: i64,
    /// Every line ever read, in arrival order -- see the identical field on
    /// `tests/dap_conformance.rs`'s `Session`: an `output` event can land
    /// interleaved with an unrelated request's response, so anything a
    /// `read_*` call doesn't match still has to be kept, not dropped.
    log: Vec<Value>,
}

impl Session {
    fn start(path: &Path) -> Self {
        let mut child = Command::new(pit_bin())
            .arg("debug")
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn pit debug");
        let stdin = child.stdin.take().unwrap();
        let reader = BufReader::new(child.stdout.take().unwrap());
        Session {
            child,
            stdin,
            reader,
            next_id: 1,
            log: Vec::new(),
        }
    }

    fn write_line(&mut self, v: &Value) {
        let line = serde_json::to_string(v).unwrap();
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// A fire-and-forget command: no `id`, no response expected.
    fn fire(&mut self, cmd: &str) {
        self.write_line(&json!({"cmd": cmd}));
    }

    /// A command that gets a matching `{"id":N,...}` response.
    fn request(&mut self, cmd: &str, mut args: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        args["id"] = json!(id);
        args["cmd"] = json!(cmd);
        self.write_line(&args);
        self.read_until(|m| m.get("id").and_then(Value::as_i64) == Some(id))
            .pop()
            .unwrap()
    }

    fn read_line(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("read a line");
        assert!(!line.is_empty(), "child closed stdout unexpectedly");
        serde_json::from_str(line.trim()).expect("valid JSON line")
    }

    fn read_until(&mut self, mut pred: impl FnMut(&Value) -> bool) -> Vec<Value> {
        let mut out = Vec::new();
        loop {
            let msg = self.read_line();
            self.log.push(msg.clone());
            let done = pred(&msg);
            out.push(msg);
            if done {
                return out;
            }
        }
    }

    fn read_event(&mut self, name: &str) -> Value {
        self.read_until(|m| m.get("event").and_then(Value::as_str) == Some(name))
            .pop()
            .unwrap()
    }

    fn stdout_so_far(&self) -> String {
        self.log
            .iter()
            .filter(|m| {
                m.get("event").and_then(Value::as_str) == Some("output")
                    && m.get("category").and_then(Value::as_str) == Some("stdout")
            })
            .map(|m| m["text"].as_str().unwrap_or(""))
            .collect()
    }
}

#[test]
fn launch_break_stack_vars_eval_continue_output_and_exit() {
    let src = "fn helper():\n    print(\"in helper\")\nfn main():\n    print(\"start\")\n    let xs = [1, 2, 3]\n    helper()\n    print(\"end\")\n";
    let path = write_program(src, "core");
    let mut session = Session::start(&path);

    let resp = session.request(
        "launch",
        json!({"program": path.to_str().unwrap(), "stopOnEntry": false}),
    );
    assert_eq!(resp["ok"], true, "launch failed: {resp}");

    let resp = session.request(
        "break",
        json!({"source": path.to_str().unwrap(), "lines": [6]}),
    );
    assert_eq!(resp["ok"], true);
    let lines = resp["lines"].as_array().unwrap();
    assert_eq!(lines[0]["line"], 6);
    assert_eq!(lines[0]["verified"], true);

    session.fire("continue");
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["reason"], "breakpoint");
    assert_eq!(stopped["fn"], "main");
    assert_eq!(stopped["line"], 6);

    let resp = session.request("stack", json!({}));
    let frames = resp["frames"].as_array().unwrap();
    assert_eq!(frames[0]["fn"], "main");
    assert_eq!(frames[0]["line"], 6);

    let resp = session.request("vars", json!({"frame": 0, "ref": 0}));
    let vars = resp["vars"].as_array().unwrap();
    let xs = vars.iter().find(|v| v["name"] == "xs").unwrap();
    assert_eq!(xs["value"], "[1, 2, 3]");
    assert_eq!(xs["type"], "[int]");
    let xs_ref = xs["ref"].as_i64().unwrap();
    assert!(xs_ref > 0);

    let resp = session.request("vars", json!({"frame": 0, "ref": xs_ref}));
    let children = resp["vars"].as_array().unwrap();
    let values: Vec<&str> = children
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert_eq!(values, vec!["1", "2", "3"]);

    let resp = session.request("eval", json!({"frame": 0, "expr": "xs[1]"}));
    assert_eq!(resp["value"], "2");
    assert_eq!(resp["type"], "int");

    session.fire("continue");
    session.read_event("exited");
    let stdout = session.stdout_so_far();
    assert!(stdout.contains("start"), "stdout so far: {stdout:?}");
    assert!(stdout.contains("in helper"), "stdout so far: {stdout:?}");
    assert!(stdout.contains("end"), "stdout so far: {stdout:?}");

    let quit = session.request("quit", json!({}));
    assert_eq!(quit["ok"], true);
    let status = session.child.wait().expect("pit debug exits");
    assert!(status.success());
    std::fs::remove_file(&path).ok();
}

#[test]
fn fault_path_emits_fault_event_then_process_exits_1() {
    let src = "fn get(xs: [int], i: int) -> int:\n    return xs[i]\nfn main():\n    let xs = [1, 2, 3]\n    print(get(xs, 9))\n";
    let path = write_program(src, "fault");
    let mut session = Session::start(&path);

    let resp = session.request("launch", json!({"program": path.to_str().unwrap()}));
    assert_eq!(resp["ok"], true, "launch failed: {resp}");

    session.fire("continue");
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["reason"], "exception");
    let fault = session.read_event("fault");
    assert_eq!(fault["code"], "E0701");
    assert_eq!(fault["file"], path.to_str().unwrap());

    // Resuming past a fault means `abort_with` runs to completion and the
    // whole `pit debug` process exits 1, same D5 contract as the DAP
    // frontend -- don't wait for a response, the process may die first.
    session.fire("continue");
    let status = session.child.wait().expect("pit debug exits");
    assert_eq!(status.code(), Some(1));
    std::fs::remove_file(&path).ok();
}
