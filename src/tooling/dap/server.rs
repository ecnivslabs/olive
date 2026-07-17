//! `pit dap`: request dispatch loop for a real DAP client (VS Code or any
//! other). Requests arrive on real stdin; every protocol frame this process
//! writes, whether a response, an event, or captured debuggee output, goes
//! through one dup'd descriptor (`ServerState::proto`) so debuggee prints
//! (redirected onto fd 1/2, see `redirect.rs`) never interleave with it.

mod inspect;

use super::engine::{BpSpec, DebugEvent, FrameSnapshot, StopReason};
use super::launch::{self, DebugSession};
use super::protocol::{Seq, error_response, event, read_message, response, write_message};
use super::redirect::Redirect;
use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use std::fs::File;
use std::io;
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub(crate) struct ServerState {
    pub(crate) proto: Arc<Mutex<File>>,
    pub(crate) seq: Arc<Seq>,
    pub(crate) session: Option<DebugSession>,
    monitor: Option<JoinHandle<()>>,
    files: FxHashMap<PathBuf, usize>,
    pub(crate) last_exception: Arc<Mutex<Option<(String, String)>>>,
    /// The arguments a `launch` succeeded with, reused by `restart` when the
    /// client doesn't resend them.
    last_launch_args: Value,
}

pub fn run_dap() {
    let proto_fd = unsafe { libc::dup(1) };
    if proto_fd < 0 {
        eprintln!("pit dap: failed to reserve the protocol descriptor");
        std::process::exit(1);
    }
    let proto = Arc::new(Mutex::new(unsafe { File::from_raw_fd(proto_fd) }));
    let mut state = ServerState {
        proto,
        seq: Arc::new(Seq::new()),
        session: None,
        monitor: None,
        files: FxHashMap::default(),
        last_exception: Arc::new(Mutex::new(None)),
        last_launch_args: Value::Null,
    };

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    loop {
        let msg = match read_message(&mut reader) {
            Ok(Some(v)) => v,
            Ok(None) => break,
            Err(e) => {
                eprintln!("pit dap: transport error: {e}");
                break;
            }
        };
        if !handle_message(&mut state, msg) {
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

fn send(state: &ServerState, msg: Value) {
    let mut w = state.proto.lock().unwrap();
    let _ = write_message(&mut *w, &msg);
}

pub(crate) fn send_response(state: &ServerState, request_seq: i64, command: &str, body: Value) {
    send(state, response(&state.seq, request_seq, command, body));
}

pub(crate) fn send_error(state: &ServerState, request_seq: i64, command: &str, message: &str) {
    send(
        state,
        error_response(&state.seq, request_seq, command, message),
    );
}

fn send_event(state: &ServerState, name: &str, body: Value) {
    send(state, event(&state.seq, name, body));
}

/// Returns `false` to end the session loop (a `disconnect` request).
fn handle_message(state: &mut ServerState, msg: Value) -> bool {
    let Some(request_seq) = msg.get("seq").and_then(Value::as_i64) else {
        return true;
    };
    let command = msg.get("command").and_then(Value::as_str).unwrap_or("");
    let args = msg.get("arguments").cloned().unwrap_or(Value::Null);

    match command {
        "initialize" => {
            send_response(
                state,
                request_seq,
                command,
                json!({
                    "supportsConfigurationDoneRequest": true,
                    "supportsEvaluateForHovers": true,
                    "supportsExceptionInfoRequest": true,
                    "supportsConditionalBreakpoints": true,
                    "supportsHitConditionalBreakpoints": true,
                    "supportsLogPoints": true,
                    "supportsBreakpointLocationsRequest": true,
                    "supportsTerminateRequest": true,
                    "supportsRestartRequest": true,
                    "supportsSetVariable": true,
                    "supportsSetExpression": true,
                    "exceptionBreakpointFilters": [
                        {"filter": "faults", "label": "Runtime Faults", "default": true}
                    ],
                }),
            );
        }
        "launch" => handle_launch(state, request_seq, command, &args),
        "restart" => handle_restart(state, request_seq, &args),
        "setBreakpoints" => handle_set_breakpoints(state, request_seq, &args),
        "breakpointLocations" => handle_breakpoint_locations(state, request_seq, &args),
        "configurationDone" => {
            send_response(state, request_seq, command, json!({}));
            if let Some(session) = &state.session {
                session.cont();
            }
        }
        "threads" => {
            send_response(
                state,
                request_seq,
                command,
                json!({"threads": [{"id": 1, "name": "main"}]}),
            );
        }
        "stackTrace" => handle_stack_trace(state, request_seq),
        "scopes" => inspect::handle_scopes(state, request_seq, &args),
        "variables" => inspect::handle_variables(state, request_seq, &args),
        "evaluate" => inspect::handle_evaluate(state, request_seq, &args),
        "setVariable" => inspect::handle_set_variable(state, request_seq, &args),
        "setExpression" => inspect::handle_set_expression(state, request_seq, &args),
        "exceptionInfo" => inspect::handle_exception_info(state, request_seq),
        "setExceptionBreakpoints" => {
            let stop_on_faults = args
                .get("filters")
                .and_then(Value::as_array)
                .map(|filters| filters.iter().any(|f| f.as_str() == Some("faults")))
                .unwrap_or(false);
            if let Some(session) = &state.session {
                session.set_stop_on_faults(stop_on_faults);
            }
            send_response(state, request_seq, command, json!({}));
        }
        "continue" => {
            if let Some(session) = &state.session {
                session.cont();
                send_response(
                    state,
                    request_seq,
                    command,
                    json!({"allThreadsContinued": true}),
                );
                send_event(
                    state,
                    "continued",
                    json!({"threadId": 1, "allThreadsContinued": true}),
                );
            } else {
                send_error(state, request_seq, command, "no active session");
            }
        }
        "next" => step(state, request_seq, command, |s| s.next()),
        "stepIn" => step(state, request_seq, command, |s| s.step_in()),
        "stepOut" => step(state, request_seq, command, |s| s.step_out()),
        "pause" => {
            if let Some(session) = &state.session {
                session.pause();
                send_response(state, request_seq, command, json!({}));
            } else {
                send_error(state, request_seq, command, "no active session");
            }
        }
        "terminate" => {
            if let Some(session) = &state.session {
                session.detach();
                send_response(state, request_seq, command, json!({}));
            } else {
                send_error(state, request_seq, command, "no active session");
            }
        }
        "disconnect" => {
            if let Some(session) = state.session.take() {
                drop(session);
            }
            if let Some(m) = state.monitor.take() {
                let _ = m.join();
            }
            send_response(state, request_seq, command, json!({}));
            return false;
        }
        _ => send_error(
            state,
            request_seq,
            command,
            &format!("unsupported request: {command}"),
        ),
    }
    true
}

fn step(state: &ServerState, request_seq: i64, command: &str, run: impl FnOnce(&DebugSession)) {
    if let Some(session) = &state.session {
        run(session);
        send_response(state, request_seq, command, json!({}));
        send_event(
            state,
            "continued",
            json!({"threadId": 1, "allThreadsContinued": true}),
        );
    } else {
        send_error(state, request_seq, command, "no active session");
    }
}

fn handle_restart(state: &mut ServerState, request_seq: i64, args: &Value) {
    let relaunch_args = args
        .get("arguments")
        .cloned()
        .filter(|v| !v.is_null())
        .unwrap_or_else(|| state.last_launch_args.clone());
    if let Some(session) = state.session.take() {
        drop(session);
    }
    if let Some(m) = state.monitor.take() {
        let _ = m.join();
    }
    handle_launch(state, request_seq, "restart", &relaunch_args);
}

fn handle_launch(state: &mut ServerState, request_seq: i64, command: &str, args: &Value) {
    let Some(program) = args.get("program").and_then(Value::as_str) else {
        send_error(state, request_seq, command, "missing 'program' argument");
        return;
    };
    let stop_on_entry = args
        .get("stopOnEntry")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let redirect = {
        let proto = state.proto.clone();
        let seq = state.seq.clone();
        Redirect::install(move |category, text| {
            let msg = event(
                &seq,
                "output",
                json!({"category": category, "output": text}),
            );
            let mut w = proto.lock().unwrap();
            let _ = write_message(&mut *w, &msg);
        })
    };
    let redirect = match redirect {
        Ok(r) => r,
        Err(e) => {
            send_error(
                state,
                request_seq,
                command,
                &format!("redirect setup failed: {e}"),
            );
            return;
        }
    };

    let mut session = match launch::launch(program, stop_on_entry) {
        Ok(s) => s,
        Err(e) => {
            redirect.restore();
            send_error(state, request_seq, command, &e.to_string());
            return;
        }
    };

    state.files = file_map(session.file_names());
    let events_rx = session.take_events();
    let proto = state.proto.clone();
    let seq = state.seq.clone();
    let last_exception = state.last_exception.clone();
    let monitor =
        std::thread::spawn(move || run_monitor(events_rx, redirect, proto, seq, last_exception));

    state.session = Some(session);
    state.monitor = Some(monitor);
    state.last_launch_args = args.clone();
    send_response(state, request_seq, command, json!({}));
    send_event(state, "initialized", json!({}));
}

/// Canonicalized source path -> file id, built once per launch from the
/// compiled program's own file table so `setBreakpoints` can map a client's
/// path back to the id the engine indexes breakpoints by.
fn file_map(file_names: &FxHashMap<usize, String>) -> FxHashMap<PathBuf, usize> {
    file_names
        .iter()
        .filter_map(|(&id, path)| Path::new(path).canonicalize().ok().map(|p| (p, id)))
        .collect()
}

fn file_id_for(state: &ServerState, path: &str) -> Option<usize> {
    let canon = Path::new(path).canonicalize().ok()?;
    state.files.get(&canon).copied()
}

fn handle_set_breakpoints(state: &mut ServerState, request_seq: i64, args: &Value) {
    let Some(session) = &state.session else {
        send_error(state, request_seq, "setBreakpoints", "no active session");
        return;
    };
    let Some(path) = args
        .get("source")
        .and_then(|s| s.get("path"))
        .and_then(Value::as_str)
    else {
        send_error(state, request_seq, "setBreakpoints", "missing source.path");
        return;
    };
    let specs: Vec<BpSpec> = args
        .get("breakpoints")
        .and_then(Value::as_array)
        .map(|bps| bps.iter().filter_map(bp_spec).collect())
        .unwrap_or_default();

    let Some(file_id) = file_id_for(state, path) else {
        // Unknown source file (e.g. a library the client set a breakpoint
        // in before it was ever loaded): every requested line is unverified.
        let body: Vec<Value> = specs
            .iter()
            .map(|s| json!({"line": s.line, "verified": false}))
            .collect();
        send_response(
            state,
            request_seq,
            "setBreakpoints",
            json!({"breakpoints": body}),
        );
        return;
    };

    let resolved = session.set_breakpoints_with(file_id, &specs);
    let body: Vec<Value> = resolved
        .into_iter()
        .map(|(line, verified)| json!({"line": line, "verified": verified}))
        .collect();
    send_response(
        state,
        request_seq,
        "setBreakpoints",
        json!({"breakpoints": body}),
    );
}

fn bp_spec(bp: &Value) -> Option<BpSpec> {
    Some(BpSpec {
        line: bp.get("line").and_then(Value::as_u64)? as u32,
        condition: bp
            .get("condition")
            .and_then(Value::as_str)
            .map(str::to_string),
        hit_condition: bp
            .get("hitCondition")
            .and_then(Value::as_str)
            .map(str::to_string),
        log_message: bp
            .get("logMessage")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn handle_breakpoint_locations(state: &ServerState, request_seq: i64, args: &Value) {
    let Some(session) = &state.session else {
        send_error(
            state,
            request_seq,
            "breakpointLocations",
            "no active session",
        );
        return;
    };
    let Some(path) = args
        .get("source")
        .and_then(|s| s.get("path"))
        .and_then(Value::as_str)
    else {
        send_error(
            state,
            request_seq,
            "breakpointLocations",
            "missing source.path",
        );
        return;
    };
    let Some(line) = args.get("line").and_then(Value::as_u64) else {
        send_error(state, request_seq, "breakpointLocations", "missing line");
        return;
    };
    let end_line = args.get("endLine").and_then(Value::as_u64).unwrap_or(line);

    let Some(file_id) = file_id_for(state, path) else {
        send_response(
            state,
            request_seq,
            "breakpointLocations",
            json!({"breakpoints": []}),
        );
        return;
    };
    let mut lines = session.lines_in_range(file_id, line as u32, end_line as u32);
    lines.sort_unstable();
    let body: Vec<Value> = lines.into_iter().map(|l| json!({"line": l})).collect();
    send_response(
        state,
        request_seq,
        "breakpointLocations",
        json!({"breakpoints": body}),
    );
}

fn handle_stack_trace(state: &ServerState, request_seq: i64) {
    let Some(session) = &state.session else {
        send_error(state, request_seq, "stackTrace", "no active session");
        return;
    };
    let frames = session.stack();
    let body: Vec<Value> = frames
        .iter()
        .enumerate()
        .map(|(idx, f)| stack_frame(idx, f))
        .collect();
    send_response(
        state,
        request_seq,
        "stackTrace",
        json!({"stackFrames": body, "totalFrames": frames.len()}),
    );
}

fn stack_frame(idx: usize, f: &FrameSnapshot) -> Value {
    let name = Path::new(&f.file)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| f.file.clone());
    json!({
        "id": idx,
        "name": f.name,
        "source": {"path": f.file, "name": name},
        "line": f.line,
        "column": 1,
    })
}

fn stopped_body(reason: &StopReason) -> Value {
    let (reason_str, text) = match reason {
        StopReason::Entry => ("entry", None),
        StopReason::Breakpoint => ("breakpoint", None),
        StopReason::Step => ("step", None),
        StopReason::Pause => ("pause", None),
        StopReason::Fault { message, .. } => ("exception", Some(message.clone())),
    };
    let mut body = json!({"reason": reason_str, "threadId": 1, "allThreadsStopped": true});
    if let Some(t) = text {
        body["text"] = json!(t);
    }
    body
}

/// Owns the event stream and the fd redirect for one launched session: reads
/// `DebugEvent`s until `Exited`, translating each into DAP events, then
/// restores fd 1/2 (unblocking the redirect's pump threads) and sends the
/// final `exited`/`terminated` pair.
fn run_monitor(
    events_rx: Receiver<DebugEvent>,
    redirect: Redirect,
    proto: Arc<Mutex<File>>,
    seq: Arc<Seq>,
    last_exception: Arc<Mutex<Option<(String, String)>>>,
) {
    let emit = |name: &str, body: Value| {
        let mut w = proto.lock().unwrap();
        let _ = write_message(&mut *w, &event(&seq, name, body));
    };
    for ev in events_rx.iter() {
        match ev {
            DebugEvent::Stopped { reason, .. } => {
                if let StopReason::Fault { code, message } = &reason {
                    *last_exception.lock().unwrap() = Some((code.clone(), message.clone()));
                }
                emit("stopped", stopped_body(&reason));
            }
            DebugEvent::Output(text) => {
                emit("output", json!({"category": "console", "output": text}));
            }
            DebugEvent::Exited(code) => {
                redirect.restore();
                emit("exited", json!({"exitCode": code}));
                emit("terminated", json!({}));
                break;
            }
        }
    }
}
