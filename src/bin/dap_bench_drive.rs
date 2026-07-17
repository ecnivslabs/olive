//! Compiled replacement for `dap_drive.py`. Same protocol, same behavior
//! (launch, optional single breakpoint, resume past every `stopped` until
//! `exited`, then quit) but with negligible process-startup cost, so
//! `dap_overhead.sh` measures Olive's own instrumentation tax instead of
//! an interpreter's startup jitter riding along in every sample.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn send(stdin: &mut impl Write, obj: &Value) {
    let mut line = serde_json::to_string(obj).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pit_bin = &args[1];
    let liv_path = &args[2];
    let bp_line: Option<i64> = args.get(3).map(|s| s.parse().unwrap());

    let mut child = Command::new(pit_bin)
        .arg("debug")
        .arg(liv_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pit debug");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        &json!({"id": 1, "cmd": "launch", "program": liv_path}),
    );
    if let Some(line) = bp_line {
        send(
            &mut stdin,
            &json!({"id": 2, "cmd": "break", "source": liv_path, "lines": [line]}),
        );
    }
    send(&mut stdin, &json!({"cmd": "continue"}));

    for line in stdout.lines() {
        let line = line.expect("read pit debug stdout");
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = serde_json::from_str(&line).expect("parse pit debug protocol line");
        match msg.get("event").and_then(Value::as_str) {
            Some("exited") => break,
            Some("stopped") => send(&mut stdin, &json!({"cmd": "continue"})),
            _ => {}
        }
    }

    send(&mut stdin, &json!({"id": 3, "cmd": "quit"}));
    let status = child.wait().expect("wait pit debug");
    std::process::exit(status.code().unwrap_or(1));
}
