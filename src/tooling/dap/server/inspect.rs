//! D7's data requests: `scopes`, `variables`, `evaluate`, `exceptionInfo`.
//! Split out of `server.rs` to keep that file under the line-count cap;
//! these handlers are as much a part of the request dispatch as anything
//! left there, just grouped separately.

use super::{ServerState, send_error, send_response};
use crate::tooling::dap::eval;
use crate::tooling::dap::values;
use serde_json::{Value, json};

/// One scope handle per stack frame, distinguished from a real `VarStore`
/// child handle (always >= 1) by an offset well above what a single frame's
/// locals could ever produce.
const SCOPE_REF_BASE: i64 = 1_000_000;

pub(super) fn handle_scopes(state: &ServerState, request_seq: i64, args: &Value) {
    let Some(_) = &state.session else {
        send_error(state, request_seq, "scopes", "no active session");
        return;
    };
    let frame_idx = args.get("frameId").and_then(Value::as_i64).unwrap_or(0);
    send_response(
        state,
        request_seq,
        "scopes",
        json!({"scopes": [{
            "name": "Locals",
            "variablesReference": SCOPE_REF_BASE + frame_idx,
            "expensive": false,
        }]}),
    );
}

pub(super) fn handle_variables(state: &ServerState, request_seq: i64, args: &Value) {
    let Some(session) = &state.session else {
        send_error(state, request_seq, "variables", "no active session");
        return;
    };
    let Some(reference) = args.get("variablesReference").and_then(Value::as_i64) else {
        send_error(
            state,
            request_seq,
            "variables",
            "missing variablesReference",
        );
        return;
    };
    let vars = if reference >= SCOPE_REF_BASE {
        values::frame_variables(session, (reference - SCOPE_REF_BASE) as usize)
    } else {
        values::children(session, reference)
    };
    let body: Vec<Value> = vars.iter().map(variable_json).collect();
    send_response(state, request_seq, "variables", json!({"variables": body}));
}

fn variable_json(v: &values::Variable) -> Value {
    json!({
        "name": v.name,
        "value": v.value,
        "type": v.type_name,
        "variablesReference": v.reference,
    })
}

pub(super) fn handle_evaluate(state: &ServerState, request_seq: i64, args: &Value) {
    let Some(session) = &state.session else {
        send_error(state, request_seq, "evaluate", "no active session");
        return;
    };
    let Some(expr) = args.get("expression").and_then(Value::as_str) else {
        send_error(state, request_seq, "evaluate", "missing expression");
        return;
    };
    let frame_idx = args.get("frameId").and_then(Value::as_i64).unwrap_or(0) as usize;
    match eval::evaluate(session, frame_idx, expr) {
        Ok(v) => send_response(
            state,
            request_seq,
            "evaluate",
            json!({"result": v.value, "type": v.type_name, "variablesReference": v.reference}),
        ),
        Err(msg) => send_error(state, request_seq, "evaluate", &msg),
    }
}

pub(super) fn handle_exception_info(state: &ServerState, request_seq: i64) {
    let Some(session) = &state.session else {
        send_error(state, request_seq, "exceptionInfo", "no active session");
        return;
    };
    let Some((code, message)) = state.last_exception.lock().unwrap().clone() else {
        send_error(
            state,
            request_seq,
            "exceptionInfo",
            "not stopped on an exception",
        );
        return;
    };
    let location = session
        .stack()
        .first()
        .map(|f| format!("{} ({}:{})", f.name, f.file, f.line))
        .unwrap_or_default();
    send_response(
        state,
        request_seq,
        "exceptionInfo",
        json!({
            "exceptionId": code,
            "description": message,
            "breakMode": "always",
            "details": {"message": message, "stackTrace": location},
        }),
    );
}
