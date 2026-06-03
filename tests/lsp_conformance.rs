//! Scripted end-to-end session against the real `pit lsp` binary over its
//! actual stdio transport (not the in-process handler used by the crate's
//! own unit tests): initialize -> didOpen -> publishDiagnostics -> hover ->
//! definition -> completion -> formatting -> shutdown -> exit. This is the
//! first stdin/stdout-streaming subprocess test in the repo (the existing
//! `differential_fuzz.rs` only does one-shot `.output()` calls), so it
//! establishes the pattern: write a framed request, read framed messages
//! until the response with a matching id shows up, skipping any
//! notification in between (the way a real client's request queue would).

use serde_json::{Value, json};
use std::io::{BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

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

/// Reads messages until one with `id == expected_id` shows up, returning it.
/// Notifications (no id, e.g. `publishDiagnostics`) are collected and
/// returned alongside so the caller can assert on them too.
fn read_response(reader: &mut BufReader<ChildStdout>, expected_id: i64) -> (Value, Vec<Value>) {
    let mut notifications = Vec::new();
    loop {
        let msg = read_message(reader);
        if msg.get("id").and_then(Value::as_i64) == Some(expected_id) {
            return (msg, notifications);
        }
        notifications.push(msg);
    }
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: i64,
}

impl Session {
    fn start() -> Self {
        let mut child = Command::new(pit_bin())
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn pit lsp");
        let stdin = child.stdin.take().unwrap();
        let reader = BufReader::new(child.stdout.take().unwrap());
        Session {
            child,
            stdin,
            reader,
            next_id: 1,
        }
    }

    fn request(&mut self, method: &str, params: Value) -> (Value, Vec<Value>) {
        let id = self.next_id;
        self.next_id += 1;
        write_message(
            &mut self.stdin,
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        );
        read_response(&mut self.reader, id)
    }

    fn notify(&mut self, method: &str, params: Value) {
        write_message(
            &mut self.stdin,
            &json!({"jsonrpc": "2.0", "method": method, "params": params}),
        );
    }
}

fn file_uri(path: &std::path::Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

#[test]
fn full_scripted_session_covers_e9_1_through_e9_3() {
    let mut session = Session::start();

    // -- E9.1: initialize/shutdown handshake --
    let (init_response, _) = session.request(
        "initialize",
        json!({"processId": null, "rootUri": null, "capabilities": {}}),
    );
    assert_eq!(
        init_response["result"]["capabilities"]["hoverProvider"],
        true
    );
    assert_eq!(
        init_response["result"]["capabilities"]["definitionProvider"],
        true
    );
    assert_eq!(
        init_response["result"]["capabilities"]["documentFormattingProvider"],
        true
    );
    session.notify("initialized", json!({}));

    // -- E9.2: didOpen with a broken program produces a diagnostic, then a
    // didChange to a fixed version clears it --
    let dir = std::env::temp_dir().join(format!("olive_lsp_conformance_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.liv");
    std::fs::write(&path, "").unwrap();
    let uri = file_uri(&path);

    let broken = "fn main():\n    print(nope)\n";
    session.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": uri, "languageId": "olive", "version": 1, "text": broken}}),
    );
    let diag_notif = read_message(&mut session.reader);
    assert_eq!(diag_notif["method"], "textDocument/publishDiagnostics");
    let diags = diag_notif["params"]["diagnostics"].as_array().unwrap();
    assert_eq!(
        diags.len(),
        1,
        "broken program should report one diagnostic"
    );
    assert_eq!(diags[0]["code"], "E0001");

    let fixed = "fn main():\n    let count = 42\n    print(count)\n";
    session.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": uri, "version": 2},
            "contentChanges": [{"text": fixed}],
        }),
    );
    let clean_notif = read_message(&mut session.reader);
    assert_eq!(clean_notif["method"], "textDocument/publishDiagnostics");
    assert_eq!(
        clean_notif["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "fixed program should clear the earlier diagnostic"
    );

    // -- E9.3: hover on a local --
    let count_offset = fixed.rfind("count)").unwrap() + 1;
    let count_line = fixed[..count_offset].matches('\n').count() as u32;
    let line_start = fixed[..count_offset]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let count_char = (count_offset - line_start) as u32;
    let (hover_response, _) = session.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": {"line": count_line, "character": count_char}}),
    );
    assert_eq!(hover_response["result"]["contents"]["value"], "int");

    // -- E9.3: definition on a function --
    let with_call = "fn helper() -> int:\n    return 1\n\nfn main():\n    print(helper())\n";
    session.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": uri, "version": 3},
            "contentChanges": [{"text": with_call}],
        }),
    );
    let _ = read_message(&mut session.reader); // publishDiagnostics for the change above

    let call_offset = with_call.rfind("helper()").unwrap() + 1;
    let call_line = with_call[..call_offset].matches('\n').count() as u32;
    let call_line_start = with_call[..call_offset]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let call_char = (call_offset - call_line_start) as u32;
    let (def_response, _) = session.request(
        "textDocument/definition",
        json!({"textDocument": {"uri": uri}, "position": {"line": call_line, "character": call_char}}),
    );
    assert_eq!(def_response["result"]["uri"], uri);
    assert_eq!(def_response["result"]["range"]["start"]["line"], 0);

    // -- E9.3: completion after a dot --
    let with_list = "fn main():\n    let xs = [1, 2, 3]\n    xs.\n";
    session.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": uri, "version": 4},
            "contentChanges": [{"text": with_list}],
        }),
    );
    let _ = read_message(&mut session.reader); // publishDiagnostics (syntax error mid-dot, expected)

    let dot_offset = with_list.find("xs.\n").unwrap() + 3;
    let dot_line = with_list[..dot_offset].matches('\n').count() as u32;
    let dot_line_start = with_list[..dot_offset]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let dot_char = (dot_offset - dot_line_start) as u32;
    let (completion_response, _) = session.request(
        "textDocument/completion",
        json!({"textDocument": {"uri": uri}, "position": {"line": dot_line, "character": dot_char}}),
    );
    let labels: Vec<String> = completion_response["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["label"].as_str().unwrap().to_string())
        .collect();
    assert!(labels.contains(&"append".to_string()));
    assert!(labels.contains(&"sort".to_string()));
    assert!(!labels.contains(&"upper".to_string()));

    // -- formatting --
    session.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": uri, "version": 5},
            "contentChanges": [{"text": "let   x=1\n"}],
        }),
    );
    let _ = read_message(&mut session.reader);
    let (fmt_response, _) = session.request(
        "textDocument/formatting",
        json!({"textDocument": {"uri": uri}, "options": {"tabSize": 4, "insertSpaces": true}}),
    );
    let edits = fmt_response["result"].as_array().unwrap();
    assert_eq!(edits.len(), 1);
    assert!(edits[0]["newText"].as_str().unwrap().contains("let x = 1"));

    // -- shutdown/exit handshake --
    let (shutdown_response, _) = session.request("shutdown", Value::Null);
    assert!(shutdown_response["result"].is_null());
    session.notify("exit", json!({}));

    let status = session.child.wait().expect("pit lsp exits");
    assert!(
        status.success(),
        "exit after shutdown should be a clean exit code"
    );

    std::fs::remove_dir_all(&dir).ok();
}
