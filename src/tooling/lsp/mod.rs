//! `pit lsp`: JSON-RPC session layer over `compile::diagnose`. Full doc sync only.

mod completion;
mod definition;
mod diagnostics;
mod formatting;
mod hover;
mod locate;
mod position;
mod protocol;
mod scope;
mod state;
mod uri;

use crate::compile::diagnose::DiagnoseOutput;
use position::{LineIndex, LspPosition};
use protocol::{read_message, write_message};
use serde_json::{Value, json};
use state::ServerState;
use std::io::{self, Write};
use std::path::Path;

pub fn run_lsp() {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let mut state = ServerState::default();

    loop {
        let msg = match read_message(&mut reader) {
            Ok(Some(v)) => v,
            Ok(None) => break,
            Err(e) => {
                eprintln!("pit lsp: transport error: {e}");
                break;
            }
        };
        if !handle_message(&mut state, &mut writer, msg) {
            break;
        }
    }

    std::process::exit(if state.shutdown_requested { 0 } else { 1 });
}

/// Returns `false` to end the session loop (an `exit` notification).
fn handle_message<W: Write>(state: &mut ServerState, writer: &mut W, msg: Value) -> bool {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = msg.get("id").cloned();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => {
            respond(
                writer,
                id,
                json!({
                    "capabilities": {
                        "textDocumentSync": 1,
                        "hoverProvider": true,
                        "definitionProvider": true,
                        "completionProvider": { "triggerCharacters": ["."] },
                        "documentFormattingProvider": true,
                    },
                    "serverInfo": { "name": "pit", "version": env!("CARGO_PKG_VERSION") },
                }),
            );
            state.initialized = true;
        }
        "initialized" => {}
        "shutdown" => {
            state.shutdown_requested = true;
            respond(writer, id, Value::Null);
        }
        "exit" => return false,
        "textDocument/didOpen" => on_did_open(state, writer, &params),
        "textDocument/didChange" => on_did_change(state, writer, &params),
        "textDocument/didClose" => on_did_close(state, &params),
        "textDocument/hover" => on_hover(state, writer, id, &params),
        "textDocument/definition" => on_definition(state, writer, id, &params),
        "textDocument/completion" => on_completion(state, writer, id, &params),
        "textDocument/formatting" => on_formatting(state, writer, id, &params),
        "$/cancelRequest" => {}
        _ => {
            if id.is_some() {
                respond_error(writer, id, -32601, &format!("method not found: {method}"));
            }
        }
    }
    true
}

fn respond<W: Write>(writer: &mut W, id: Option<Value>, result: Value) {
    let msg = json!({"jsonrpc": "2.0", "id": id, "result": result});
    let _ = write_message(writer, &msg);
}

fn respond_error<W: Write>(writer: &mut W, id: Option<Value>, code: i64, message: &str) {
    let msg = json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}});
    let _ = write_message(writer, &msg);
}

fn notify<W: Write>(writer: &mut W, method: &str, params: Value) {
    let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
    let _ = write_message(writer, &msg);
}

fn doc_uri(params: &Value) -> Option<&str> {
    params.get("textDocument")?.get("uri")?.as_str()
}

fn position_of(params: &Value) -> Option<LspPosition> {
    let pos = params.get("position")?;
    Some(LspPosition {
        line: pos.get("line")?.as_u64()? as u32,
        character: pos.get("character")?.as_u64()? as u32,
    })
}

fn publish<W: Write>(writer: &mut W, out: &DiagnoseOutput, path: &Path) {
    for (_uri, params) in diagnostics::publish_params(out, path) {
        notify(writer, "textDocument/publishDiagnostics", params);
    }
}

fn on_did_open<W: Write>(state: &mut ServerState, writer: &mut W, params: &Value) {
    let Some(td) = params.get("textDocument") else {
        return;
    };
    let Some(uri) = td.get("uri").and_then(Value::as_str) else {
        return;
    };
    let Some(path) = uri::uri_to_path(uri) else {
        return;
    };
    let text = td
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let version = td.get("version").and_then(Value::as_i64).unwrap_or(0);
    let out = state.open(path.clone(), text, version);
    publish(writer, &out, &path);
}

fn on_did_change<W: Write>(state: &mut ServerState, writer: &mut W, params: &Value) {
    let Some(uri) = doc_uri(params) else {
        return;
    };
    let Some(path) = uri::uri_to_path(uri) else {
        return;
    };
    let version = params
        .get("textDocument")
        .and_then(|t| t.get("version"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let Some(changes) = params.get("contentChanges").and_then(Value::as_array) else {
        return;
    };
    // Full sync: the client sends exactly one entry, the whole new document.
    let Some(text) = changes
        .last()
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
    else {
        return;
    };
    if let Some(out) = state.change(&path, text.to_string(), version) {
        publish(writer, &out, &path);
    }
}

fn on_did_close(state: &mut ServerState, params: &Value) {
    if let Some(uri) = doc_uri(params)
        && let Some(path) = uri::uri_to_path(uri)
    {
        state.close(&path);
    }
}

/// A document's own file id within its last-good snapshot's `Sources` table.
fn file_id_of(out: &DiagnoseOutput, path: &Path) -> Option<usize> {
    out.sources
        .iter()
        .find(|(_, (p, _))| Path::new(p) == path)
        .map(|(id, _)| *id)
}

fn on_hover<W: Write>(state: &ServerState, writer: &mut W, id: Option<Value>, params: &Value) {
    let result = (|| -> Option<Value> {
        let uri = doc_uri(params)?;
        let path = uri::uri_to_path(uri)?;
        let pos = position_of(params)?;
        let doc = state.documents.get(&path)?;
        let out = doc.last_good.as_ref()?;
        let line_index = LineIndex::new(&doc.text);
        let offset = line_index.to_char_offset(pos);
        let file_id = file_id_of(out, &path)?;
        let (ty, span) = hover::hover_at(out, file_id, offset)?;
        Some(json!({
            "contents": { "kind": "plaintext", "value": ty },
            "range": line_index.span_to_range(span).to_json(),
        }))
    })();
    respond(writer, id, result.unwrap_or(Value::Null));
}

fn on_definition<W: Write>(state: &ServerState, writer: &mut W, id: Option<Value>, params: &Value) {
    let result = (|| -> Option<Value> {
        let uri = doc_uri(params)?;
        let path = uri::uri_to_path(uri)?;
        let pos = position_of(params)?;
        let doc = state.documents.get(&path)?;
        let out = doc.last_good.as_ref()?;
        let line_index = LineIndex::new(&doc.text);
        let offset = line_index.to_char_offset(pos);
        let file_id = file_id_of(out, &path)?;
        let span = definition::definition_at(out, file_id, offset)?;
        let (target_path, target_text) = out.sources.get(&span.file_id)?.clone();
        let target_range = LineIndex::new(&target_text).span_to_range(span);
        Some(json!({
            "uri": uri::path_to_uri(Path::new(&target_path)),
            "range": target_range.to_json(),
        }))
    })();
    respond(writer, id, result.unwrap_or(Value::Null));
}

fn completion_item_kind(kind: completion::CompletionKind) -> i64 {
    use completion::CompletionKind::*;
    match kind {
        Method => 2,
        Field => 5,
        Function => 3,
        Variable => 6,
        Enum => 13,
        Keyword => 14,
        Struct => 22,
        Module => 9,
    }
}

fn on_completion<W: Write>(state: &ServerState, writer: &mut W, id: Option<Value>, params: &Value) {
    let items = (|| -> Option<Vec<Value>> {
        let uri = doc_uri(params)?;
        let path = uri::uri_to_path(uri)?;
        let pos = position_of(params)?;
        let doc = state.documents.get(&path)?;
        let items = completion::completions_at(&path, &doc.text, pos, doc.last_good.as_deref());
        Some(
            items
                .into_iter()
                .map(|c| json!({"label": c.label, "kind": completion_item_kind(c.kind)}))
                .collect(),
        )
    })()
    .unwrap_or_default();
    respond(writer, id, json!(items));
}

fn on_formatting<W: Write>(state: &ServerState, writer: &mut W, id: Option<Value>, params: &Value) {
    let edits = (|| -> Option<Vec<Value>> {
        let uri = doc_uri(params)?;
        let path = uri::uri_to_path(uri)?;
        let doc = state.documents.get(&path)?;
        let formatted = formatting::format_document(&doc.text)?;
        if formatted == doc.text {
            return Some(Vec::new());
        }
        let line_index = LineIndex::new(&doc.text);
        let end = line_index.to_lsp(doc.text.chars().count());
        Some(vec![json!({
            "range": {
                "start": {"line": 0, "character": 0},
                "end": end.to_json(),
            },
            "newText": formatted,
        })])
    })()
    .unwrap_or_default();
    respond(writer, id, json!(edits));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_msg(id: i64) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {"processId": null, "rootUri": null, "capabilities": {}}})
    }

    #[test]
    fn initialize_responds_with_capabilities() {
        let mut state = ServerState::default();
        let mut out = Vec::new();
        assert!(handle_message(&mut state, &mut out, init_msg(1)));
        assert!(state.initialized);

        let mut cursor = std::io::Cursor::new(out);
        let response = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["capabilities"]["hoverProvider"], true);
        assert_eq!(
            response["result"]["capabilities"]["completionProvider"]["triggerCharacters"][0],
            "."
        );
    }

    #[test]
    fn shutdown_then_exit_ends_the_loop() {
        let mut state = ServerState::default();
        let mut out = Vec::new();
        let shutdown = json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"});
        assert!(handle_message(&mut state, &mut out, shutdown));
        assert!(state.shutdown_requested);

        let exit = json!({"jsonrpc": "2.0", "method": "exit"});
        assert!(!handle_message(&mut state, &mut out, exit));
    }

    #[test]
    fn unknown_request_gets_method_not_found_error() {
        let mut state = ServerState::default();
        let mut out = Vec::new();
        let msg = json!({"jsonrpc": "2.0", "id": 5, "method": "textDocument/bogus", "params": {}});
        assert!(handle_message(&mut state, &mut out, msg));

        let mut cursor = std::io::Cursor::new(out);
        let response = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(response["error"]["code"], -32601);
    }

    #[test]
    fn did_open_publishes_diagnostics_notification() {
        let mut state = ServerState::default();
        let mut out = Vec::new();
        let path =
            std::env::temp_dir().join(format!("olive_lsp_mod_test_{}.liv", std::process::id()));
        std::fs::write(&path, "").unwrap();
        let uri = uri::path_to_uri(&path);
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": { "uri": uri, "languageId": "olive", "version": 1, "text": "print(nope)\n" }
            }
        });
        assert!(handle_message(&mut state, &mut out, msg));

        let mut cursor = std::io::Cursor::new(out);
        let notif = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(notif["method"], "textDocument/publishDiagnostics");
        assert_eq!(notif["params"]["diagnostics"][0]["code"], "E0001");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn hover_end_to_end_after_did_open() {
        let mut state = ServerState::default();
        let mut out = Vec::new();
        let path =
            std::env::temp_dir().join(format!("olive_lsp_hover_e2e_{}.liv", std::process::id()));
        std::fs::write(&path, "").unwrap();
        let uri = uri::path_to_uri(&path);
        let text = "fn main():\n    let count = 42\n    print(count)\n";
        let open = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {"uri": uri, "languageId": "olive", "version": 1, "text": text}}
        });
        handle_message(&mut state, &mut out, open);
        out.clear();

        let line_index = LineIndex::new(text);
        let char_offset = text.rfind("count)").unwrap() + 1;
        let pos = line_index.to_lsp(char_offset);
        let hover_req = json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "textDocument/hover",
            "params": {"textDocument": {"uri": uri}, "position": {"line": pos.line, "character": pos.character}}
        });
        handle_message(&mut state, &mut out, hover_req);

        let mut cursor = std::io::Cursor::new(out);
        let response = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(response["result"]["contents"]["value"], "int");
        std::fs::remove_file(&path).ok();
    }
}
