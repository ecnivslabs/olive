//! Conditional breakpoints, hit counts, and logpoints, driven through the
//! headless `pit debug <file>` frontend (same `Session` harness pattern as
//! `tests/dap_headless.rs`).

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
        "olive_dap_conditions_{}_{tag}_{id}.liv",
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

    fn fire(&mut self, cmd: &str) {
        self.write_line(&json!({"cmd": cmd}));
    }

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

    fn count_events(&self, name: &str) -> usize {
        self.log
            .iter()
            .filter(|m| m.get("event").and_then(Value::as_str) == Some(name))
            .count()
    }
}

fn launch_and_break(src: &str, tag: &str, line: u32, bp_extra: Value) -> (Session, PathBuf) {
    let path = write_program(src, tag);
    let mut session = Session::start(&path);
    let resp = session.request("launch", json!({"program": path.to_str().unwrap()}));
    assert_eq!(resp["ok"], true, "launch failed: {resp}");

    let mut entry = json!({"line": line});
    entry
        .as_object_mut()
        .unwrap()
        .extend(bp_extra.as_object().unwrap().clone());
    let resp = session.request(
        "break",
        json!({"source": path.to_str().unwrap(), "lines": [entry]}),
    );
    assert_eq!(resp["ok"], true);
    let lines = resp["lines"].as_array().unwrap();
    assert_eq!(
        lines[0]["verified"], true,
        "breakpoint not verified: {resp}"
    );
    (session, path)
}

#[test]
fn condition_stops_exactly_once_at_the_matching_iteration() {
    let src =
        "fn main():\n    let mut i = 0\n    while i < 1000:\n        print(i)\n        i = i + 1\n";
    let (mut session, path) = launch_and_break(src, "cond", 4, json!({"cond": "i == 500"}));

    session.fire("continue");
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["reason"], "breakpoint");

    let resp = session.request("vars", json!({"frame": 0, "ref": 0}));
    let vars = resp["vars"].as_array().unwrap();
    let i = vars.iter().find(|v| v["name"] == "i").unwrap();
    assert_eq!(i["value"], "500");

    // Resuming must run to exit without stopping a second time: the
    // condition is only ever true once in a 1000-iteration loop.
    session.fire("continue");
    session.read_event("exited");
    assert_eq!(session.count_events("stopped"), 1);

    session.request("quit", json!({}));
    let status = session.child.wait().expect("pit debug exits");
    assert!(status.success());
    std::fs::remove_file(&path).ok();
}

#[test]
fn hit_condition_stops_on_multiples() {
    let src =
        "fn main():\n    let mut i = 0\n    while i < 30:\n        print(i)\n        i = i + 1\n";
    let (mut session, path) = launch_and_break(src, "hits", 4, json!({"hits": "%10"}));

    // Hit count is 1-indexed (the first hit is hit 1), so "%10" stops on
    // the 10th/20th/30th hit -- `i` is 9/19/29 at those points.
    for expected in [9, 19, 29] {
        session.fire("continue");
        let stopped = session.read_event("stopped");
        assert_eq!(stopped["reason"], "breakpoint");
        let resp = session.request("vars", json!({"frame": 0, "ref": 0}));
        let vars = resp["vars"].as_array().unwrap();
        let i = vars.iter().find(|v| v["name"] == "i").unwrap();
        assert_eq!(i["value"], expected.to_string());
    }

    session.fire("continue");
    session.read_event("exited");
    assert_eq!(session.count_events("stopped"), 3);

    session.request("quit", json!({}));
    let status = session.child.wait().expect("pit debug exits");
    assert!(status.success());
    std::fs::remove_file(&path).ok();
}

#[test]
fn logpoint_emits_output_every_hit_and_never_stops() {
    let src = "fn main():\n    let mut i = 0\n    while i < 10:\n        print(\"tick\")\n        i = i + 1\n";
    let (mut session, path) = launch_and_break(src, "log", 4, json!({"log": "i is {i}"}));

    session.fire("continue");
    session.read_event("exited");

    assert_eq!(session.count_events("stopped"), 0);
    let texts: Vec<String> = session
        .log
        .iter()
        .filter(|m| {
            m.get("event").and_then(Value::as_str) == Some("output")
                && m.get("category").and_then(Value::as_str) == Some("console")
        })
        .map(|m| m["text"].as_str().unwrap_or("").trim().to_string())
        .collect();
    assert_eq!(texts.len(), 10, "log events: {texts:?}");
    for (n, text) in texts.iter().enumerate() {
        assert_eq!(text, &format!("i is {n}"));
    }

    session.request("quit", json!({}));
    let status = session.child.wait().expect("pit debug exits");
    assert!(status.success());
    std::fs::remove_file(&path).ok();
}
