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
//! A scalar target (`int`/`float`/`f32`/`bool`/`str`/`None`, or a
//! tag-encoded union of those) is parsed by `encode_literal`'s own small
//! grammar -- not Olive source syntax, see its doc comment. Everything else
//! (list/vector/set/tuple/dict/struct/enum) is a whole-value replacement:
//! `encode_value` parses `text` as a real olive expression (`eval.rs`'s
//! `AExpr`, arithmetic and aggregate literals both) and `build_value` walks
//! it against the target's static type, allocating fresh heap values with
//! the same runtime constructors codegen itself uses for a literal
//! (`olive_list_new`, `olive_obj_new`, `olive_struct_alloc`,
//! `olive_enum_new`) and filling them with the already-wired element/field
//! setters. A struct/enum constructor is resolved against
//! `struct_fields()`/`enum_defs()`, never an arbitrary function call.

use super::engine::EngineShared;
use super::eval::{self, AExpr, Token, Value};
use super::values;
use crate::semantic::type_descriptor::{concrete_ty, type_descriptor};
use crate::semantic::types::Type;
use std::ffi::CString;

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

/// Whether `ty` is one `encode_literal`'s own text grammar can produce
/// directly -- everything else is a whole-value replacement, built by
/// `build_value` from a parsed expression instead.
fn is_scalar_settable(ty: &Type) -> bool {
    ty.is_tag_encoded_union()
        || matches!(
            concrete_ty(ty),
            Type::Bool
                | Type::Null
                | Type::Int
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Usize
                | Type::Float
                | Type::F32
                | Type::Str
        )
}

/// Entry point for a `setVariable`/`setExpression` write whose target may be
/// a whole aggregate, not just a scalar: `text` for a scalar-settable type
/// still goes through `encode_literal`'s own grammar unchanged; anything
/// else parses as a real olive expression and builds a fresh value against
/// `ty`. `frame_idx` resolves any path operand the expression references
/// (`[n, n + 1]` reading a live local), the same frame the write itself is
/// happening in.
pub(crate) fn encode_value(
    session: &EngineShared,
    frame_idx: usize,
    ty: &Type,
    text: &str,
) -> Result<i64, String> {
    if is_scalar_settable(ty) {
        return encode_literal(session, ty, text);
    }
    let expr = eval::parse_arith(text.trim())?;
    build_value(session, frame_idx, ty, &expr)
}

/// Builds a fresh value of type `ty` from a parsed expression: an aggregate
/// literal shape matching `ty` recurses field/element-wise, anything else is
/// a scalar leaf evaluated against the live frame and converted to `ty`'s
/// raw representation.
fn build_value(
    session: &EngineShared,
    frame_idx: usize,
    ty: &Type,
    expr: &AExpr,
) -> Result<i64, String> {
    if is_scalar_settable(ty) {
        let Some((fn_id, cells)) = session.frame_cells(frame_idx) else {
            return Err("not stopped".to_string());
        };
        let value = eval::eval_arith(session, fn_id, &cells, expr)?;
        return value_to_raw(session, ty, value);
    }
    match concrete_ty(ty) {
        Type::List(elem_ty) | Type::Vector(elem_ty, _) | Type::Set(elem_ty) => {
            let AExpr::List(items) = expr else {
                return Err(format!("expected a list literal for {ty}"));
            };
            build_seq(session, frame_idx, elem_ty, items)
        }
        Type::Tuple(item_tys) => {
            let AExpr::Tuple(items) = expr else {
                return Err(format!("expected a tuple literal for {ty}"));
            };
            if items.len() != item_tys.len() {
                return Err(format!(
                    "{ty} has {} elements, found {}",
                    item_tys.len(),
                    items.len()
                ));
            }
            let ptr = call_alloc1(session, "olive_list_new", items.len() as i64)?;
            for (i, (item_ty, item)) in item_tys.iter().zip(items).enumerate() {
                let raw = build_value(session, frame_idx, item_ty, item)?;
                call_setter(session, "olive_debug_seq_set", ptr, i as i64, raw)?;
            }
            Ok(ptr)
        }
        Type::Dict(key_ty, val_ty) => {
            let AExpr::Dict(pairs) = expr else {
                return Err(format!("expected a dict literal for {ty}"));
            };
            let ptr = call_alloc0(session, "olive_obj_new")?;
            for (key_expr, val_expr) in pairs {
                let kraw = build_value(session, frame_idx, key_ty, key_expr)?;
                let vraw = build_value(session, frame_idx, val_ty, val_expr)?;
                if needs_structural_key(key_ty) {
                    let desc = build_descriptor(session, key_ty);
                    obj_set_typed(session, ptr, kraw, vraw, &desc)?;
                } else {
                    obj_set(session, ptr, kraw, vraw)?;
                }
            }
            Ok(ptr)
        }
        Type::Struct(name, _, _) => {
            let AExpr::Construct(ctor_name, args) = expr else {
                return Err(format!("expected a {name}(...) constructor"));
            };
            if ctor_name != name {
                return Err(format!(
                    "expected a {name}(...) constructor, found {ctor_name}(...)"
                ));
            }
            let Some(fields) = session.struct_fields().get(name).cloned() else {
                return Err(format!("no such struct: {name}"));
            };
            if args.len() != fields.len() {
                return Err(format!(
                    "{name} has {} fields, found {}",
                    fields.len(),
                    args.len()
                ));
            }
            // A recycled slab slot isn't zeroed (`olive_struct_alloc` only
            // writes the header), so every field must be written -- the
            // `args.len() != fields.len()` check above guarantees that.
            let ptr = call_alloc1(session, "olive_struct_alloc", fields.len() as i64)?;
            for (i, (fname, arg)) in fields.iter().zip(args).enumerate() {
                let fty = session
                    .field_types()
                    .get(&(name.clone(), fname.clone()))
                    .cloned()
                    .unwrap_or(Type::Any);
                let raw = build_value(session, frame_idx, &fty, arg)?;
                unsafe { *(ptr as *mut i64).add(1 + i) = raw };
            }
            Ok(ptr)
        }
        Type::Enum(name, _) => {
            let AExpr::Construct(variant_name, args) = expr else {
                return Err(format!("expected a variant constructor for {name}"));
            };
            let Some(variants) = session.enum_defs().get(name).cloned() else {
                return Err(format!("no such enum: {name}"));
            };
            let Some((tag, (_, payload_tys))) = variants
                .iter()
                .enumerate()
                .find(|(_, (vname, _))| vname == variant_name)
            else {
                return Err(format!("{name} has no variant '{variant_name}'"));
            };
            if args.len() != payload_tys.len() {
                return Err(format!(
                    "{name}.{variant_name} takes {} values, found {}",
                    payload_tys.len(),
                    args.len()
                ));
            }
            let type_id = crate::mir::enum_type_id(name);
            let ptr = call_alloc3(
                session,
                "olive_enum_new",
                type_id,
                tag as i64,
                payload_tys.len() as i64,
            )?;
            for (i, (pty, arg)) in payload_tys.iter().zip(args).enumerate() {
                let raw = build_value(session, frame_idx, pty, arg)?;
                call_setter(session, "olive_debug_enum_set", ptr, i as i64, raw)?;
            }
            Ok(ptr)
        }
        other => Err(format!("cannot construct a value of type {other}")),
    }
}

fn build_seq(
    session: &EngineShared,
    frame_idx: usize,
    elem_ty: &Type,
    items: &[AExpr],
) -> Result<i64, String> {
    let ptr = call_alloc1(session, "olive_list_new", items.len() as i64)?;
    for (i, item) in items.iter().enumerate() {
        let raw = build_value(session, frame_idx, elem_ty, item)?;
        call_setter(session, "olive_debug_seq_set", ptr, i as i64, raw)?;
    }
    Ok(ptr)
}

/// Mirrors `codegen::cranelift::imports::needs_structural_key` -- a dict key
/// of one of these types hashes/compares by value, not by pointer, so the
/// insert needs `olive_obj_set_typed`'s descriptor rather than plain
/// `olive_obj_set`. Duplicated rather than shared: that module tree is
/// private to `codegen::cranelift`, and this is a stable, six-variant match,
/// not an algorithm that could drift.
fn needs_structural_key(ty: &Type) -> bool {
    matches!(
        concrete_ty(ty),
        Type::Struct(..)
            | Type::Enum(..)
            | Type::Tuple(_)
            | Type::List(_)
            | Type::Set(_)
            | Type::Dict(_, _)
    )
}

fn build_descriptor(session: &EngineShared, ty: &Type) -> CString {
    let bytes = type_descriptor(
        ty,
        session.struct_fields(),
        session.field_types(),
        session.enum_defs(),
    );
    CString::new(bytes.into_bytes()).expect("type descriptor bytes are non-zero by construction")
}

/// Converts an evaluated scalar `Value` into `ty`'s raw word -- the
/// aggregate-literal counterpart to `encode_literal`'s text parsing, used
/// for every leaf `build_value` reaches (a list element, a dict key/value, a
/// struct field, an enum payload word).
fn value_to_raw(session: &EngineShared, ty: &Type, value: Value) -> Result<i64, String> {
    if ty.is_tag_encoded_union() {
        return match value {
            Value::Bool(b) => values::any_encode(session, 2, b as i64)
                .ok_or_else(|| "runtime encoder unavailable".to_string()),
            Value::Int(n) => values::any_encode(session, 1, n)
                .ok_or_else(|| "runtime encoder unavailable".to_string()),
            Value::Float(f) => values::any_encode(session, 3, f.to_bits() as i64)
                .ok_or_else(|| "runtime encoder unavailable".to_string()),
            Value::Str(s) => encode_str_bytes(session, s.as_bytes()),
        };
    }
    match concrete_ty(ty) {
        Type::Bool => match value {
            Value::Bool(b) => Ok(b as i64),
            other => Err(format!("expected a bool, found {other}")),
        },
        int_ty @ (Type::Int
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize) => match value {
            Value::Int(n) => {
                check_int_range(int_ty, n)?;
                Ok(n)
            }
            other => Err(format!("expected an integer, found {other}")),
        },
        Type::Float => match value {
            Value::Int(n) => Ok((n as f64).to_bits() as i64),
            Value::Float(f) => Ok(f.to_bits() as i64),
            other => Err(format!("expected a number, found {other}")),
        },
        Type::F32 => match value {
            Value::Int(n) => Ok((n as f32).to_bits() as i64),
            Value::Float(f) => Ok((f as f32).to_bits() as i64),
            other => Err(format!("expected a number, found {other}")),
        },
        Type::Str => match value {
            Value::Str(s) => encode_str_bytes(session, s.as_bytes()),
            other => Err(format!("expected a string, found {other}")),
        },
        other => Err(format!("cannot set a value of type {other}")),
    }
}

fn call_alloc0(session: &EngineShared, name: &str) -> Result<i64, String> {
    let ptr = session
        .runtime_symbol(name)
        .ok_or_else(|| format!("runtime symbol {name} unavailable"))?;
    let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(ptr) };
    Ok(f())
}

fn call_alloc1(session: &EngineShared, name: &str, a: i64) -> Result<i64, String> {
    let ptr = session
        .runtime_symbol(name)
        .ok_or_else(|| format!("runtime symbol {name} unavailable"))?;
    let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    Ok(f(a))
}

fn call_alloc3(session: &EngineShared, name: &str, a: i64, b: i64, c: i64) -> Result<i64, String> {
    let ptr = session
        .runtime_symbol(name)
        .ok_or_else(|| format!("runtime symbol {name} unavailable"))?;
    let f: extern "C" fn(i64, i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    Ok(f(a, b, c))
}

fn obj_set(session: &EngineShared, obj: i64, key: i64, val: i64) -> Result<(), String> {
    let ptr = session
        .runtime_symbol("olive_obj_set")
        .ok_or_else(|| "runtime symbol olive_obj_set unavailable".to_string())?;
    let f: extern "C" fn(i64, i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    f(obj, key, val);
    Ok(())
}

fn obj_set_typed(
    session: &EngineShared,
    obj: i64,
    key: i64,
    val: i64,
    desc: &CString,
) -> Result<(), String> {
    let ptr = session
        .runtime_symbol("olive_obj_set_typed")
        .ok_or_else(|| "runtime symbol olive_obj_set_typed unavailable".to_string())?;
    let f: extern "C" fn(i64, i64, i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    f(obj, key, val, desc.as_ptr() as i64);
    Ok(())
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
    encode_str_bytes(session, content.as_bytes())
}

fn encode_str_bytes(session: &EngineShared, bytes: &[u8]) -> Result<i64, String> {
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
            if !session.is_innermost_frame(frame_idx) {
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
