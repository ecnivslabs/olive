//! Variable inspection and mutation requests: `scopes`, `variables`,
//! `evaluate`, `exceptionInfo`, `setVariable`, `setExpression`. Split out of
//! `server.rs` to keep that file under the line-count cap; these handlers
//! are as much a part of the request dispatch as anything left there, just
//! grouped separately.

use super::{ServerState, send_error, send_response};
use crate::tooling::dap::eval;
use crate::tooling::dap::setvar;
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

pub(super) fn handle_set_variable(state: &ServerState, request_seq: i64, args: &Value) {
    let Some(session) = &state.session else {
        send_error(state, request_seq, "setVariable", "no active session");
        return;
    };
    let Some(reference) = args.get("variablesReference").and_then(Value::as_i64) else {
        send_error(
            state,
            request_seq,
            "setVariable",
            "missing variablesReference",
        );
        return;
    };
    let Some(name) = args.get("name").and_then(Value::as_str) else {
        send_error(state, request_seq, "setVariable", "missing name");
        return;
    };
    let Some(value_text) = args.get("value").and_then(Value::as_str) else {
        send_error(state, request_seq, "setVariable", "missing value");
        return;
    };
    let is_scope = reference >= SCOPE_REF_BASE;

    let resolved = if is_scope {
        setvar::target_for_local(session, (reference - SCOPE_REF_BASE) as usize, name)
    } else {
        setvar::target_for_child(session, reference, name)
    };
    let (target, ty) = match resolved {
        Ok(t) => t,
        Err(msg) => {
            send_error(state, request_seq, "setVariable", &msg);
            return;
        }
    };
    // The "child" form (a container's `variablesReference`) carries no
    // frame of its own in the DAP protocol; frame 0 is only consulted for a
    // path operand nested inside the new value's expression (`[n, n + 1]`),
    // never for a pure literal (`[1, 2, 3]`, `Point(1, 2)`) -- the common
    // case this feature exists for.
    let value_frame = if is_scope {
        (reference - SCOPE_REF_BASE) as usize
    } else {
        0
    };
    let raw = match setvar::encode_value(session, value_frame, &ty, value_text) {
        Ok(r) => r,
        Err(msg) => {
            send_error(state, request_seq, "setVariable", &msg);
            return;
        }
    };
    if let Err(msg) = setvar::write_value(session, target, raw) {
        send_error(state, request_seq, "setVariable", &msg);
        return;
    }

    let updated = if is_scope {
        values::frame_variables(session, (reference - SCOPE_REF_BASE) as usize)
    } else {
        values::children(session, reference)
    };
    let Some(v) = updated.into_iter().find(|v| v.name == name) else {
        send_error(
            state,
            request_seq,
            "setVariable",
            "write succeeded but the new value could not be re-read",
        );
        return;
    };
    send_response(
        state,
        request_seq,
        "setVariable",
        json!({"value": v.value, "type": v.type_name, "variablesReference": v.reference}),
    );
}

pub(super) fn handle_set_expression(state: &ServerState, request_seq: i64, args: &Value) {
    let Some(session) = &state.session else {
        send_error(state, request_seq, "setExpression", "no active session");
        return;
    };
    let Some(expr) = args.get("expression").and_then(Value::as_str) else {
        send_error(state, request_seq, "setExpression", "missing expression");
        return;
    };
    let Some(value_text) = args.get("value").and_then(Value::as_str) else {
        send_error(state, request_seq, "setExpression", "missing value");
        return;
    };
    let frame_idx = args.get("frameId").and_then(Value::as_i64).unwrap_or(0) as usize;

    let (target, ty) = match setvar::resolve_lvalue(session, frame_idx, expr) {
        Ok(t) => t,
        Err(msg) => {
            send_error(state, request_seq, "setExpression", &msg);
            return;
        }
    };
    let raw = match setvar::encode_value(session, frame_idx, &ty, value_text) {
        Ok(r) => r,
        Err(msg) => {
            send_error(state, request_seq, "setExpression", &msg);
            return;
        }
    };
    if let Err(msg) = setvar::write_value(session, target, raw) {
        send_error(state, request_seq, "setExpression", &msg);
        return;
    }

    match eval::evaluate(session, frame_idx, expr) {
        Ok(v) => send_response(
            state,
            request_seq,
            "setExpression",
            json!({"value": v.value, "type": v.type_name, "variablesReference": v.reference}),
        ),
        Err(msg) => send_error(state, request_seq, "setExpression", &msg),
    }
}

/// `completions` request: reuses `eval::evaluate`'s path resolver for the
/// base expression before a trailing `.`, then lists that value's children
/// the same way `variables` would -- no separate type-resolution path.
pub(super) fn handle_completions(state: &ServerState, request_seq: i64, args: &Value) {
    let Some(session) = &state.session else {
        send_error(state, request_seq, "completions", "no active session");
        return;
    };
    let Some(text) = args.get("text").and_then(Value::as_str) else {
        send_error(state, request_seq, "completions", "missing text");
        return;
    };
    let frame_idx = args.get("frameId").and_then(Value::as_i64).unwrap_or(0) as usize;
    let chars: Vec<char> = text.chars().collect();
    let column = args
        .get("column")
        .and_then(Value::as_i64)
        .unwrap_or(chars.len() as i64 + 1);
    let cursor = (column - 1).clamp(0, chars.len() as i64) as usize;

    let mut prefix_start = cursor;
    while prefix_start > 0 && eval::is_ident_char(chars[prefix_start - 1]) {
        prefix_start -= 1;
    }
    let prefix: String = chars[prefix_start..cursor].iter().collect();

    let items: Vec<(String, &str)> = if prefix_start > 0 && chars[prefix_start - 1] == '.' {
        let base_expr: String = chars[..prefix_start - 1].iter().collect();
        match eval::evaluate(session, frame_idx, &base_expr) {
            Ok(v) if v.reference != 0 => values::children(session, v.reference)
                .into_iter()
                .map(|c| (c.name, "field"))
                .collect(),
            _ => Vec::new(),
        }
    } else {
        values::frame_variables(session, frame_idx)
            .into_iter()
            .map(|v| (v.name, "variable"))
            .collect()
    };

    let targets: Vec<Value> = items
        .into_iter()
        .filter(|(name, _)| name.starts_with(&prefix))
        .map(|(name, kind)| {
            json!({
                "label": name,
                "type": kind,
                "start": prefix_start,
                "length": prefix.chars().count(),
            })
        })
        .collect();

    send_response(
        state,
        request_seq,
        "completions",
        json!({"targets": targets}),
    );
}

pub(super) fn handle_exception_info(state: &ServerState, request_seq: i64, args: &Value) {
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
        .stack(super::thread_id_of(args))
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
