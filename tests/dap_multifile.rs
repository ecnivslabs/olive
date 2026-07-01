//! Proves the file_id-packed breakpoint keys and per-frame `file_names`
//! resolution (engine.rs) actually hold for a program spanning two source
//! files, not just the single-file case every other dap_*.rs test exercises.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

/// Writes `stem.liv` verbatim, no filename mangling -- unlike the other
/// dap_*.rs harnesses' `write_program`, the stem here doubles as the
/// `from <stem> import ...` module name, so it has to stay a valid
/// identifier.
fn write_module(stem: &str, src: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("{stem}.liv"));
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
fn breakpoints_and_stepping_cross_a_local_import_boundary() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();

    let helper_stem = format!("dap_mf_helper_{pid}_{id}");
    let helper_path = write_module(
        &helper_stem,
        "fn add(a: int, b: int) -> int:\n    let s = a + b\n    return s\n",
    );

    let main_path = write_module(
        &format!("dap_mf_main_{pid}_{id}"),
        &format!(
            "from {helper_stem} import add\nfn main():\n    let r = add(2, 3)\n    print(r)\n"
        ),
    );

    let mut session = Session::start(&main_path);

    let resp = session.request(
        "launch",
        json!({"program": main_path.to_str().unwrap(), "stopOnEntry": false}),
    );
    assert_eq!(resp["ok"], true, "launch failed: {resp}");

    let resp = session.request(
        "break",
        json!({"source": main_path.to_str().unwrap(), "lines": [3]}),
    );
    assert_eq!(resp["ok"], true);
    assert_eq!(
        resp["lines"][0]["verified"], true,
        "main breakpoint: {resp}"
    );

    let resp = session.request(
        "break",
        json!({"source": helper_path.to_str().unwrap(), "lines": [3]}),
    );
    assert_eq!(resp["ok"], true);
    assert_eq!(
        resp["lines"][0]["verified"], true,
        "helper breakpoint: {resp}"
    );

    // Stop at the call site in the importing file.
    session.fire("continue");
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["reason"], "breakpoint");
    assert_eq!(stopped["fn"], "main");
    assert_eq!(stopped["line"], 3);

    let resp = session.request("stack", json!({}));
    let frames = resp["frames"].as_array().unwrap();
    assert_eq!(frames[0]["fn"], "main");
    assert_eq!(
        frames[0]["file"],
        main_path.to_str().unwrap(),
        "top frame should resolve to the main file, not the helper: {resp}"
    );

    // Step across the import boundary into the callee, landing on its first
    // statement (line 2) -- one line above the breakpoint set below (line 3),
    // so the two mechanisms are exercised independently, not conflated.
    session.fire("stepIn");
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["reason"], "step");
    assert_eq!(stopped["fn"], "add");
    assert_eq!(stopped["line"], 2);
    assert_eq!(
        stopped["file"],
        helper_path.to_str().unwrap(),
        "stepping into add() should resolve to the helper file, not main: {stopped}"
    );

    let resp = session.request("stack", json!({}));
    let frames = resp["frames"].as_array().unwrap();
    assert_eq!(frames[0]["fn"], "add");
    assert_eq!(
        frames[0]["file"],
        helper_path.to_str().unwrap(),
        "stepping into add() should resolve to the helper file, not main: {resp}"
    );
    assert_eq!(frames[1]["fn"], "main");
    assert_eq!(
        frames[1]["file"],
        main_path.to_str().unwrap(),
        "caller frame should still resolve to main, not the helper it stepped into: {resp}"
    );

    // The breakpoint set directly in the helper file also has to fire, as a
    // distinct stop from the step above.
    session.fire("continue");
    let stopped = session.read_event("stopped");
    assert_eq!(stopped["reason"], "breakpoint");
    assert_eq!(stopped["fn"], "add");
    assert_eq!(stopped["line"], 3);
    assert_eq!(stopped["file"], helper_path.to_str().unwrap());

    session.fire("continue");
    session.read_event("exited");
    let stdout = session.stdout_so_far();
    assert!(stdout.contains('5'), "stdout so far: {stdout:?}");

    let quit = session.request("quit", json!({}));
    assert_eq!(quit["ok"], true);
    let status = session.child.wait().expect("pit debug exits");
    assert!(status.success());

    std::fs::remove_file(&main_path).ok();
    std::fs::remove_file(&helper_path).ok();
}
