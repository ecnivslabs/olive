//! `pit debug <file>`: newline-delimited JSON on stdio for AI agents,
//! skipping the DAP handshake ceremony. Drives the same engine as
//! `server.rs` and genuinely reuses `values`/`eval`/`redirect`; the schema
//! is just flatter -- no seq/type envelope, no Content-Length framing, and
//! only requests carrying an `id` get a response (`continue`/`next`/
//! `stepIn`/`stepOut`/`pause` are fire-and-forget, their effect observed
//! through the `stopped`/`exited` events instead).

use super::engine::{DebugEvent, StopReason};
use super::eval;
use super::launch::{self, DebugSession};
use super::redirect::Redirect;
use super::values;
use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use std::fs::File;
use std::io::{self, BufRead, Write};
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

struct HeadlessState {
    proto: Arc<Mutex<File>>,
    default_program: String,
    session: Option<DebugSession>,
    monitor: Option<JoinHandle<()>>,
    files: FxHashMap<PathBuf, usize>,
}

pub fn run(file: &str) {
    let proto_fd = unsafe { libc::dup(1) };
    if proto_fd < 0 {
        eprintln!("pit debug: failed to reserve the protocol descriptor");
        std::process::exit(1);
    }
    let proto = Arc::new(Mutex::new(unsafe { File::from_raw_fd(proto_fd) }));
    let mut state = HeadlessState {
        proto,
        default_program: file.to_string(),
        session: None,
        monitor: None,
        files: FxHashMap::default(),
    };

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                emit(
                    &state.proto,
                    &json!({"event": "output", "category": "stderr", "text": format!("malformed request: {e}\n")}),
                );
                continue;
            }
        };
        if !handle(&mut state, msg) {
            break;
        }
    }

    if let Some(session) = state.session.take() {
        drop(session);
    }
    if let Some(m) = state.monitor.take() {
        let _ = m.join();
    }
    std::process::exit(0);
}

fn emit(proto: &Arc<Mutex<File>>, v: &Value) {
    let mut w = proto.lock().unwrap();
    let _ = writeln!(w, "{v}");
    let _ = w.flush();
}

/// No-op when `id` is absent: a fire-and-forget command like `continue`
/// gets no response, its effect observed via events instead.
fn ok(state: &HeadlessState, id: Option<i64>, mut payload: Value) {
    let Some(id) = id else { return };
    payload["id"] = json!(id);
    payload["ok"] = json!(true);
    emit(&state.proto, &payload);
}

fn err(state: &HeadlessState, id: Option<i64>, message: &str) {
    let Some(id) = id else { return };
    emit(
        &state.proto,
        &json!({"id": id, "ok": false, "error": message}),
    );
}

/// Returns `false` to end the session loop (a `quit` command).
fn handle(state: &mut HeadlessState, msg: Value) -> bool {
    let id = msg.get("id").and_then(Value::as_i64);
    let cmd = msg.get("cmd").and_then(Value::as_str).unwrap_or("");

    match cmd {
        "launch" => handle_launch(state, id, &msg),
        "break" => handle_break(state, id, &msg),
        "continue" => run_control(state, id, |s| s.cont()),
        "next" => run_control(state, id, |s| s.next()),
        "stepIn" => run_control(state, id, |s| s.step_in()),
        "stepOut" => run_control(state, id, |s| s.step_out()),
        "pause" => run_control(state, id, |s| s.pause()),
        "stack" => handle_stack(state, id),
        "vars" => handle_vars(state, id, &msg),
        "eval" => handle_eval(state, id, &msg),
        "quit" => {
            if let Some(session) = state.session.take() {
                drop(session);
            }
            if let Some(m) = state.monitor.take() {
                let _ = m.join();
            }
            ok(state, id, json!({}));
            return false;
        }
        other => err(state, id, &format!("unknown command: {other}")),
    }
    true
}

fn run_control(state: &HeadlessState, id: Option<i64>, run: impl FnOnce(&DebugSession)) {
    let Some(session) = &state.session else {
        err(state, id, "no active session");
        return;
    };
    run(session);
    ok(state, id, json!({}));
}

fn handle_launch(state: &mut HeadlessState, id: Option<i64>, args: &Value) {
    let program = args
        .get("program")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| state.default_program.clone());
    let stop_on_entry = args
        .get("stopOnEntry")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let redirect = {
        let proto = state.proto.clone();
        Redirect::install(move |category, text| {
            emit(
                &proto,
                &json!({"event": "output", "category": category, "text": text}),
            );
        })
    };
    let redirect = match redirect {
        Ok(r) => r,
        Err(e) => {
            err(state, id, &format!("redirect setup failed: {e}"));
            return;
        }
    };

    let mut session = match launch::launch(&program, stop_on_entry) {
        Ok(s) => s,
        Err(e) => {
            redirect.restore();
            err(state, id, &e.to_string());
            return;
        }
    };

    state.files = file_map(session.file_names());
    let events_rx = session.take_events();
    let proto = state.proto.clone();
    let monitor = std::thread::spawn(move || run_monitor(events_rx, redirect, proto));

    state.session = Some(session);
    state.monitor = Some(monitor);
    ok(state, id, json!({}));
}

/// Canonicalized source path -> file id, built once per launch from the
/// compiled program's own file table so `break` can map a client's path
/// back to the id the engine indexes breakpoints by.
fn file_map(file_names: &FxHashMap<usize, String>) -> FxHashMap<PathBuf, usize> {
    file_names
        .iter()
        .filter_map(|(&id, path)| Path::new(path).canonicalize().ok().map(|p| (p, id)))
        .collect()
}

fn file_id_for(state: &HeadlessState, path: &str) -> Option<usize> {
    let canon = Path::new(path).canonicalize().ok()?;
    state.files.get(&canon).copied()
}

fn handle_break(state: &mut HeadlessState, id: Option<i64>, args: &Value) {
    let Some(session) = &state.session else {
        err(state, id, "no active session");
        return;
    };
    let Some(source) = args.get("source").and_then(Value::as_str) else {
        err(state, id, "missing 'source'");
        return;
    };
    let lines: Vec<u32> = args
        .get("lines")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_u64)
                .map(|l| l as u32)
                .collect()
        })
        .unwrap_or_default();

    let Some(file_id) = file_id_for(state, source) else {
        let body: Vec<Value> = lines
            .iter()
            .map(|&l| json!({"line": l, "verified": false}))
            .collect();
        ok(state, id, json!({"lines": body}));
        return;
    };
    let resolved = session.set_breakpoints(file_id, &lines);
    let body: Vec<Value> = resolved
        .into_iter()
        .map(|(line, verified)| json!({"line": line, "verified": verified}))
        .collect();
    ok(state, id, json!({"lines": body}));
}

fn handle_stack(state: &HeadlessState, id: Option<i64>) {
    let Some(session) = &state.session else {
        err(state, id, "no active session");
        return;
    };
    let frames: Vec<Value> = session
        .stack()
        .iter()
        .enumerate()
        .map(|(idx, f)| json!({"id": idx, "fn": f.name, "file": f.file, "line": f.line}))
        .collect();
    ok(state, id, json!({"frames": frames}));
}

fn handle_vars(state: &HeadlessState, id: Option<i64>, args: &Value) {
    let Some(session) = &state.session else {
        err(state, id, "no active session");
        return;
    };
    let frame = args.get("frame").and_then(Value::as_i64).unwrap_or(0) as usize;
    let reference = args.get("ref").and_then(Value::as_i64).unwrap_or(0);
    let vars = if reference == 0 {
        values::frame_variables(session, frame)
    } else {
        values::children(session, reference)
    };
    let body: Vec<Value> = vars
        .iter()
        .map(|v| json!({"name": v.name, "type": v.type_name, "value": v.value, "ref": v.reference}))
        .collect();
    ok(state, id, json!({"vars": body}));
}

fn handle_eval(state: &HeadlessState, id: Option<i64>, args: &Value) {
    let Some(session) = &state.session else {
        err(state, id, "no active session");
        return;
    };
    let Some(expr) = args.get("expr").and_then(Value::as_str) else {
        err(state, id, "missing 'expr'");
        return;
    };
    let frame = args.get("frame").and_then(Value::as_i64).unwrap_or(0) as usize;
    match eval::evaluate(session, frame, expr) {
        Ok(v) => ok(
            state,
            id,
            json!({"value": v.value, "type": v.type_name, "ref": v.reference}),
        ),
        Err(msg) => err(state, id, &msg),
    }
}

/// Owns the event stream and the fd redirect for one launched session, same
/// shape as `server::run_monitor`: reads `DebugEvent`s until `Exited`,
/// translating each into a headless event, then restores fd 1/2.
fn run_monitor(events_rx: Receiver<DebugEvent>, redirect: Redirect, proto: Arc<Mutex<File>>) {
    for ev in events_rx.iter() {
        match ev {
            DebugEvent::Stopped { reason, frame } => {
                let reason_str = match &reason {
                    StopReason::Entry => "entry",
                    StopReason::Breakpoint => "breakpoint",
                    StopReason::Step => "step",
                    StopReason::Pause => "pause",
                    StopReason::Fault { .. } => "exception",
                };
                emit(
                    &proto,
                    &json!({
                        "event": "stopped",
                        "reason": reason_str,
                        "fn": frame.name,
                        "file": frame.file,
                        "line": frame.line,
                    }),
                );
                if let StopReason::Fault { code, message } = &reason {
                    emit(
                        &proto,
                        &json!({
                            "event": "fault",
                            "code": code,
                            "message": message,
                            "file": frame.file,
                            "line": frame.line,
                        }),
                    );
                }
            }
            DebugEvent::Exited(code) => {
                redirect.restore();
                emit(&proto, &json!({"event": "exited", "code": code}));
                break;
            }
        }
    }
}
