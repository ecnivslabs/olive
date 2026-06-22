//! Write side of variable inspection: DAP `setVariable`/`setExpression` and
//! the headless `setVar` command. Resolves a target (a top-level named local
//! or a child reached through a `VarStore` reference/path) the same way
//! `eval.rs`/`conditions.rs` resolve a *read*, then either queues a write
//! into `EngineShared`'s mirror (a top-level local -- the debuggee thread's
//! real storage for it is otherwise unreachable from this thread, see
//! `mir::debug_hooks`'s reload) or writes straight into shared heap memory
//! (a list/dict/struct/enum child -- always real, always immediate, no
//! mirror involved).
//!
//! Scope is deliberately scalars-only (int/float/bool/str/`None`): replacing
//! a whole list/dict/struct/enum value would need a general expression
//! evaluator and heap allocator this debugger doesn't have, and guessing at
//! one would risk leaking or misrepresenting the old value. Setting a
//! scalar *inside* one of those, at any depth, is fully supported.

use super::engine::EngineShared;
use super::eval::{self, Token};
use super::values;
use crate::semantic::type_descriptor::concrete_ty;
use crate::semantic::types::Type;

pub(crate) enum WriteTarget {
    /// A top-level named local of a stack frame. Only ever writable when
    /// `frame_idx == 0`: that's the one frame guaranteed to pass back
    /// through its own `__olive_debug_load` reload before it next reads the
    /// local, since it's the frame actually blocked inside the `park()`
    /// call the write happens during. An outer frame is mid-call into
    /// whatever's currently innermost, not parked at a reload point of its
    /// own, so a write to one would sit in the mirror unobserved until
    /// (if ever) that frame becomes innermost again -- `write_value`
    /// rejects it outright rather than offer that half-true guarantee.
    Local { frame_idx: usize, cell_idx: usize },
    /// Element `idx` of a list, set, vector, or tuple.
    Seq { parent: i64, idx: i64 },
    /// The value half of the dict entry at position `idx`.
    DictVal { parent: i64, idx: i64 },
    /// Field `field_idx` of a struct, at its fixed `8 + idx*8` offset.
    StructField { parent: i64, field_idx: usize },
    /// Payload word `idx` of an enum's active variant.
    EnumPayload { parent: i64, idx: i64 },
}

/// Resolves a path expression (`setExpression`, headless `setVar`'s `expr`
/// form) to its write target: `ident` alone is a top-level local, anything
/// longer descends through `values::frame_variables`/`children` -- the same
/// reads `eval::evaluate` performs -- stopping one token short to apply the
/// last step as a write instead of a render.
pub(crate) fn resolve_lvalue(
    session: &EngineShared,
    frame_idx: usize,
    expr: &str,
) -> Result<(WriteTarget, Type), String> {
    let tokens = eval::tokenize(expr)?;
    let Some(Token::Ident(root)) = tokens.first() else {
        return Err(format!("'{expr}' is not a valid assignment target"));
    };
    if tokens.len() == 1 {
        return target_for_local(session, frame_idx, root);
    }

    let vars = values::frame_variables(session, frame_idx);
    let mut current = vars
        .into_iter()
        .find(|v| v.name == *root)
        .ok_or_else(|| format!("no such variable: {root}"))?;
    for tok in &tokens[1..tokens.len() - 1] {
        if current.reference == 0 {
            return Err(format!("{} has no fields or elements", current.name));
        }
        let key = eval::token_key(tok);
        current = values::children(session, current.reference)
            .into_iter()
            .find(|c| c.name == key)
            .ok_or_else(|| format!("no such member: {key}"))?;
    }
    if current.reference == 0 {
        return Err(format!("{} has no fields or elements", current.name));
    }
    let last_key = eval::token_key(tokens.last().expect("tokens.len() > 1"));
    target_for_child(session, current.reference, &last_key)
}

/// A top-level named local of `frame_idx`, by name -- the DAP `setVariable`
/// request's "scope" form (`variablesReference` names the frame itself) and
/// the headless `setVar` request's `frame`+`name` form both resolve here.
pub(crate) fn target_for_local(
    session: &EngineShared,
    frame_idx: usize,
    name: &str,
) -> Result<(WriteTarget, Type), String> {
    let Some((fn_id, _)) = session.frame_cells(frame_idx) else {
        return Err("not stopped".to_string());
    };
    let cells = session.fn_cells(fn_id);
    let cell_idx = cells
        .iter()
        .position(|c| c.name == name)
        .ok_or_else(|| format!("no such variable: {name}"))?;
    Ok((
        WriteTarget::Local {
            frame_idx,
            cell_idx,
        },
        cells[cell_idx].ty.clone(),
    ))
}

/// A named child of a `VarStore`-referenced container -- the DAP
/// `setVariable` request's "child" form, where `variablesReference` names a
/// previously returned container rather than a frame scope. `name` matches
/// the same string `values::children_raw` renders for that member (a
/// struct field verbatim, a sequence/enum position as its decimal index, a
/// dict key quoted the way `values.rs` quotes it), so its position in that
/// listing is exactly the index/offset the write needs.
pub(crate) fn target_for_child(
    session: &EngineShared,
    reference: i64,
    name: &str,
) -> Result<(WriteTarget, Type), String> {
    let Some((parent_raw, parent_ty)) = values::node_of(session, reference) else {
        return Err("stale or unknown variable reference".to_string());
    };
    let concrete = concrete_ty(&parent_ty).clone();
    let children = values::children_raw(session, parent_raw, &concrete);
    let pos = children
        .iter()
        .position(|(n, _, _)| n == name)
        .ok_or_else(|| format!("no such member: {name}"))?;
    let child_ty = children[pos].2.clone();
    let target = match &concrete {
        Type::List(_) | Type::Vector(_, _) | Type::Set(_) | Type::Tuple(_) => WriteTarget::Seq {
            parent: parent_raw,
            idx: pos as i64,
        },
        Type::Dict(_, _) => WriteTarget::DictVal {
            parent: parent_raw,
            idx: pos as i64,
        },
        Type::Struct(_, _, _) => WriteTarget::StructField {
            parent: parent_raw,
            field_idx: pos,
        },
        Type::Enum(_, _) => WriteTarget::EnumPayload {
            parent: parent_raw,
            idx: pos as i64,
        },
        other => return Err(format!("{other} has no writable members")),
    };
    Ok((target, child_ty))
}

/// Parses `text` into a raw word for `ty`, target-type-directed since the
/// caller already knows what the destination expects. Grammar is the
/// debugger's own (`true`/`false`, `None`, a quoted string with `conditions.
/// rs`'s single-char backslash escape, or the bare text as a literal string
/// when it isn't quoted), not Olive source syntax: this mini-language
/// already exists for breakpoint conditions and log messages, and staying
/// consistent with it matters more here than matching the compiler's own
/// `True`/`False`/`None` casing.
pub(crate) fn encode_literal(session: &EngineShared, ty: &Type, text: &str) -> Result<i64, String> {
    let text = text.trim();
    // Tag-encoded unions: parse per the debugger grammar, then encode
    // through the runtime so a written 0 stays distinct from None. A str
    // word is already a valid tag-encoded member, no re-encode needed.
    if ty.is_tag_encoded_union() {
        let (kind, payload) = if text == "None" {
            (0, 0)
        } else if text == "true" || text == "false" {
            (2, (text == "true") as i64)
        } else if let Ok(n) = text.parse::<i64>() {
            (1, n)
        } else if let Ok(f) = text.parse::<f64>() {
            (3, f.to_bits() as i64)
        } else if matches!(ty, Type::Union(members) if members.contains(&Type::Str)) {
            return encode_str(session, text);
        } else {
            return Err(format!("invalid literal '{text}' for {ty}"));
        };
        return values::any_encode(session, kind, payload)
            .ok_or_else(|| "runtime encoder unavailable".to_string());
    }
    match concrete_ty(ty) {
        Type::Bool => match text {
            "true" => Ok(1),
            "false" => Ok(0),
            _ => Err(format!("expected true or false, found '{text}'")),
        },
        Type::Null => {
            if text == "None" {
                Ok(0)
            } else {
                Err(format!("expected None, found '{text}'"))
            }
        }
        int_ty @ (Type::Int
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize) => {
            let n: i64 = text
                .parse()
                .map_err(|_| format!("invalid integer literal '{text}'"))?;
            check_int_range(int_ty, n)?;
            Ok(n)
        }
        Type::Float => {
            let f: f64 = text
                .parse()
                .map_err(|_| format!("invalid float literal '{text}'"))?;
            Ok(f.to_bits() as i64)
        }
        Type::F32 => {
            let f: f64 = text
                .parse()
                .map_err(|_| format!("invalid float literal '{text}'"))?;
            Ok((f as f32).to_bits() as i64)
        }
        Type::Str => encode_str(session, text),
        other => Err(format!(
            "cannot set a value of type {other}; only int/float/bool/string/None are directly editable"
        )),
    }
}

fn check_int_range(ty: &Type, n: i64) -> Result<(), String> {
    let (min, max): (i64, i64) = match ty {
        Type::I8 => (i8::MIN as i64, i8::MAX as i64),
        Type::I16 => (i16::MIN as i64, i16::MAX as i64),
        Type::I32 => (i32::MIN as i64, i32::MAX as i64),
        Type::U8 => (0, u8::MAX as i64),
        Type::U16 => (0, u16::MAX as i64),
        Type::U32 => (0, u32::MAX as i64),
        // Int/U64/Usize: every representable i64 bit pattern is valid.
        _ => return Ok(()),
    };
    if n < min || n > max {
        return Err(format!("{n} is out of range for {ty}"));
    }
    Ok(())
}

fn encode_str(session: &EngineShared, text: &str) -> Result<i64, String> {
    let content = parse_quoted(text).unwrap_or_else(|| text.to_string());
    let bytes = content.as_bytes();
    let Some(ptr) = session.runtime_symbol("olive_debug_str_new") else {
        return Err("runtime symbol olive_debug_str_new unavailable".to_string());
    };
    let f: extern "C" fn(*const u8, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    Ok(f(bytes.as_ptr(), bytes.len() as i64))
}

/// A complete `"..."` literal, `\` escaping the next character exactly like
/// `conditions.rs::Parser::parse_string_literal`. `None` if `text` isn't
/// one (no leading quote, or trailing text after the closing quote) -- the
/// caller then uses `text` verbatim, so both `"hello"` and a bare `hello`
/// set the same string.
fn parse_quoted(text: &str) -> Option<String> {
    let mut chars = text.chars();
    if chars.next() != Some('"') {
        return None;
    }
    let mut out = String::new();
    loop {
        match chars.next() {
            None => return None,
            Some('"') => return chars.next().is_none().then_some(out),
            Some('\\') => out.push(chars.next()?),
            Some(c) => out.push(c),
        }
    }
}

/// Writes `raw` to `target`. `Local` queues the write for the debuggee
/// thread to apply on its own next reload (see `WriteTarget::Local`'s own
/// doc for why that's `frame_idx == 0` only); every other target is a
/// direct, immediate write into heap memory the debuggee and controller
/// threads already share, no queueing needed.
pub(crate) fn write_value(
    session: &EngineShared,
    target: WriteTarget,
    raw: i64,
) -> Result<(), String> {
    match target {
        WriteTarget::Local {
            frame_idx,
            cell_idx,
        } => {
            if frame_idx != 0 {
                return Err(
                    "only the topmost (currently running) frame's locals can be edited; \
                     step or continue into an outer frame before editing its locals"
                        .to_string(),
                );
            }
            if session.set_local_cell(frame_idx, cell_idx, raw) {
                Ok(())
            } else {
                Err("not stopped".to_string())
            }
        }
        WriteTarget::Seq { parent, idx } => {
            call_setter(session, "olive_debug_seq_set", parent, idx, raw)
        }
        WriteTarget::DictVal { parent, idx } => {
            call_setter(session, "olive_debug_dict_set", parent, idx, raw)
        }
        WriteTarget::EnumPayload { parent, idx } => {
            call_setter(session, "olive_debug_enum_set", parent, idx, raw)
        }
        WriteTarget::StructField { parent, field_idx } => {
            if parent == 0 {
                return Err("cannot set a field on a None value".to_string());
            }
            unsafe { *(parent as *mut i64).add(1 + field_idx) = raw };
            Ok(())
        }
    }
}

fn call_setter(
    session: &EngineShared,
    name: &str,
    parent: i64,
    idx: i64,
    raw: i64,
) -> Result<(), String> {
    if parent == 0 {
        return Err("cannot set an element on a None value".to_string());
    }
    let Some(ptr) = session.runtime_symbol(name) else {
        return Err(format!("runtime symbol {name} unavailable"));
    };
    let f: extern "C" fn(i64, i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    if f(parent, idx, raw) == 0 {
        return Err("index out of range".to_string());
    }
    Ok(())
}
