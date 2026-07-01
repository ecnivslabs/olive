//! `pit dap`: request dispatch loop for a real DAP client (VS Code or any
//! other). Requests arrive on real stdin; every protocol frame this process
//! writes, whether a response, an event, or captured debuggee output, goes
//! through one dup'd descriptor (`ServerState::proto`) so debuggee prints
//! (redirected onto fd 1/2, see `redirect.rs`) never interleave with it.

mod inspect;

use super::engine::{BpSpec, DebugEvent, FrameSnapshot, StopReason, WatchSpec};
use super::launch::{self, DebugSession};
use super::protocol::{Seq, error_response, event, read_message, response, write_message};
use super::redirect::{FdFile, Redirect};
use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub(crate) struct ServerState {
    pub(crate) proto: Arc<Mutex<FdFile>>,
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
    let proto = Arc::new(Mutex::new(unsafe { FdFile::new(proto_fd) }));
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
                    "supportsCompletionsRequest": true,
                    "supportsDataBreakpoints": true,
                    "exceptionBreakpointFilters": [
                        {"filter": "faults", "label": "Runtime Faults", "default": true}
                    ],
                }),
            );
        }
        "launch" => handle_launch(state, request_seq, command, &args),
        "restart" => handle_restart(state, request_seq, &args),
        "setBreakpoints" => handle_set_breakpoints(state, request_seq, &args),
        "dataBreakpointInfo" => handle_data_breakpoint_info(state, request_seq, &args),
        "setDataBreakpoints" => handle_set_data_breakpoints(state, request_seq, &args),
        "breakpointLocations" => handle_breakpoint_locations(state, request_seq, &args),
        "configurationDone" => {
            send_response(state, request_seq, command, json!({}));
            if let Some(session) = &state.session {
                // Starting a launched program runs its main thread, always
                // id 1; configurationDone carries no threadId of its own.
                session.cont(1);
            }
        }
        "threads" => {
            let threads = state
                .session
                .as_ref()
                .map(|s| s.threads_snapshot())
                .unwrap_or_default();
            let body: Vec<Value> = threads
                .into_iter()
                .map(|(id, name)| json!({"id": id, "name": name}))
                .collect();
            send_response(state, request_seq, command, json!({"threads": body}));
        }
        "stackTrace" => handle_stack_trace(state, request_seq, &args),
        "scopes" => inspect::handle_scopes(state, request_seq, &args),
        "variables" => inspect::handle_variables(state, request_seq, &args),
        "evaluate" => inspect::handle_evaluate(state, request_seq, &args),
        "setVariable" => inspect::handle_set_variable(state, request_seq, &args),
        "setExpression" => inspect::handle_set_expression(state, request_seq, &args),
        "completions" => inspect::handle_completions(state, request_seq, &args),
        "exceptionInfo" => inspect::handle_exception_info(state, request_seq, &args),
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
                let thread_id = thread_id_of(&args);
                // `continued` has to be on the wire before `cont()` can wake
                // the debuggee: a store-granularity stop (a data breakpoint
                // on a tight loop, in particular) can re-park within
                // microseconds, and `resume()`'s own `notify_all()` races a
                // send on this thread that hasn't happened yet -- sending
                // first makes the ordering deterministic, not probabilistic.
                // Stopping is per-thread (`allThreadsStopped: false`), so
                // continuing is too: only `thread_id` was ever parked.
                send_response(
                    state,
                    request_seq,
                    command,
                    json!({"allThreadsContinued": false}),
                );
                send_event(
                    state,
                    "continued",
                    json!({"threadId": thread_id, "allThreadsContinued": false}),
                );
                session.cont(thread_id);
            } else {
                send_error(state, request_seq, command, "no active session");
            }
        }
        "next" => step(state, request_seq, command, &args, |s, t| s.next(t)),
        "stepIn" => step(state, request_seq, command, &args, |s, t| s.step_in(t)),
        "stepOut" => step(state, request_seq, command, &args, |s, t| s.step_out(t)),
        "pause" => {
            if let Some(session) = &state.session {
                session.pause(thread_id_of(&args));
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

/// Same ordering constraint as the `continue` handler: `continued` must be
/// sent before `run` can wake the debuggee, not after.
fn step(
    state: &ServerState,
    request_seq: i64,
    command: &str,
    args: &Value,
    run: impl FnOnce(&DebugSession, i64),
) {
    if let Some(session) = &state.session {
        let thread_id = thread_id_of(args);
        send_response(state, request_seq, command, json!({}));
        send_event(
            state,
            "continued",
            json!({"threadId": thread_id, "allThreadsContinued": false}),
        );
        run(session, thread_id);
    } else {
        send_error(state, request_seq, command, "no active session");
    }
}

/// `threadId` from a request's arguments, defaulting to the main thread --
/// always id 1 -- since every non-thread-aware caller (headless, most test
/// harnesses, a client that never sends one) means exactly that.
fn thread_id_of(args: &Value) -> i64 {
    args.get("threadId").and_then(Value::as_i64).unwrap_or(1)
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

/// `dataBreakpointInfo` only resolves a top-level named local of a frame
/// (a bare `name`, no `variablesReference`) -- `debug_store` only ever
/// fires for a named cell's own slot, so a container child genuinely has
/// no watchable target; spec allows `dataId: null` for exactly this case.
fn handle_data_breakpoint_info(state: &ServerState, request_seq: i64, args: &Value) {
    let Some(session) = &state.session else {
        send_error(
            state,
            request_seq,
            "dataBreakpointInfo",
            "no active session",
        );
        return;
    };
    let Some(name) = args.get("name").and_then(Value::as_str) else {
        send_error(state, request_seq, "dataBreakpointInfo", "missing name");
        return;
    };
    if args
        .get("variablesReference")
        .and_then(Value::as_i64)
        .is_some()
    {
        send_response(
            state,
            request_seq,
            "dataBreakpointInfo",
            json!({
                "dataId": Value::Null,
                "description": "only a top-level local can be watched, not a container's field or element",
            }),
        );
        return;
    }
    let frame_idx = args.get("frameId").and_then(Value::as_i64).unwrap_or(0) as usize;
    match session.data_id_for(frame_idx, name) {
        Some(data_id) => send_response(
            state,
            request_seq,
            "dataBreakpointInfo",
            json!({
                "dataId": data_id,
                "description": format!("{name} (write)"),
                "accessTypes": ["write"],
            }),
        ),
        None => send_response(
            state,
            request_seq,
            "dataBreakpointInfo",
            json!({"dataId": Value::Null, "description": format!("no such variable: {name}")}),
        ),
    }
}

fn handle_set_data_breakpoints(state: &ServerState, request_seq: i64, args: &Value) {
    let Some(session) = &state.session else {
        send_error(
            state,
            request_seq,
            "setDataBreakpoints",
            "no active session",
        );
        return;
    };
    let specs: Vec<WatchSpec> = args
        .get("breakpoints")
        .and_then(Value::as_array)
        .map(|bps| bps.iter().filter_map(watch_spec).collect())
        .unwrap_or_default();
    let results = session.set_data_breakpoints(&specs);
    let body: Vec<Value> = results
        .into_iter()
        .map(|(verified, message)| {
            let mut v = json!({"verified": verified});
            if let Some(m) = message {
                v["message"] = json!(m);
            }
            v
        })
        .collect();
    send_response(
        state,
        request_seq,
        "setDataBreakpoints",
        json!({"breakpoints": body}),
    );
}

fn watch_spec(bp: &Value) -> Option<WatchSpec> {
    Some(WatchSpec {
        data_id: bp.get("dataId").and_then(Value::as_str)?.to_string(),
        condition: bp
            .get("condition")
            .and_then(Value::as_str)
            .map(str::to_string),
        hit_condition: bp
            .get("hitCondition")
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

fn handle_stack_trace(state: &ServerState, request_seq: i64, args: &Value) {
    let Some(session) = &state.session else {
        send_error(state, request_seq, "stackTrace", "no active session");
        return;
    };
    let frames = session.stack(thread_id_of(args));
    let body: Vec<Value> = frames.iter().map(stack_frame).collect();
    send_response(
        state,
        request_seq,
        "stackTrace",
        json!({"stackFrames": body, "totalFrames": frames.len()}),
    );
}

fn stack_frame(f: &FrameSnapshot) -> Value {
    let name = Path::new(&f.file)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| f.file.clone());
    json!({
        "id": f.frame_id,
        "name": f.name,
        "source": {"path": f.file, "name": name},
        "line": f.line,
        "column": 1,
    })
}

/// `allThreadsStopped` is always `false`: a stop only ever parks the thread
/// that hit it (`EngineShared::park_on`), never the rest of the process, so
/// nothing else is actually stopped for the client to reflect.
fn stopped_body(reason: &StopReason, thread_id: i64) -> Value {
    let (reason_str, text) = match reason {
        StopReason::Entry => ("entry", None),
        StopReason::Breakpoint => ("breakpoint", None),
        StopReason::Step => ("step", None),
        StopReason::Pause => ("pause", None),
        StopReason::DataBreakpoint => ("data breakpoint", None),
        StopReason::Fault { message, .. } => ("exception", Some(message.clone())),
    };
    let mut body = json!({
        "reason": reason_str,
        "threadId": thread_id,
        "allThreadsStopped": false,
    });
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
    proto: Arc<Mutex<FdFile>>,
    seq: Arc<Seq>,
    last_exception: Arc<Mutex<Option<(String, String)>>>,
) {
    let emit = |name: &str, body: Value| {
        let mut w = proto.lock().unwrap();
        let _ = write_message(&mut *w, &event(&seq, name, body));
    };
    for ev in events_rx.iter() {
        match ev {
            DebugEvent::Stopped {
                reason, thread_id, ..
            } => {
                if let StopReason::Fault { code, message } = &reason {
                    *last_exception.lock().unwrap() = Some((code.clone(), message.clone()));
                }
                emit("stopped", stopped_body(&reason, thread_id));
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
            DebugEvent::ThreadStarted { id } => {
                emit("thread", json!({"reason": "started", "threadId": id}));
            }
            DebugEvent::ThreadExited { id } => {
                emit("thread", json!({"reason": "exited", "threadId": id}));
            }
        }
    }
}
