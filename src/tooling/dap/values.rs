//! Variable decoding and rendering, against either a parked frame (DAP and
//! headless variable/evaluate requests) or a raw hook-captured frame
//! (`conditions.rs`, evaluated before any stop decision). Leaf text comes
//! from the runtime's own `olive_format_typed`, resolved through
//! `EngineShared::runtime_symbol`; the object-graph reads this needs
//! (list/set length, dict entries, enum tag/payload) go through the ABI
//! helpers in `std_lib/src/debug.rs` rather than duplicating heap layout
//! here. The one exception is a struct field, whose offset (`8 + idx*8`) is
//! fixed and already relied on elsewhere in the codebase (`translate.rs:545`).

use super::engine::EngineShared;
use crate::semantic::type_descriptor::{concrete_ty, type_descriptor};
use crate::semantic::types::Type;
use std::ffi::{CStr, CString};
use std::sync::Mutex;

pub struct Variable {
    pub name: String,
    pub type_name: String,
    pub value: String,
    pub reference: i64,
}

#[derive(Clone)]
struct VarNode {
    value: i64,
    ty: Type,
}

/// Per-stop handle table for lazy child expansion. Reference `0` always
/// means "no children"; live handles are 1-based positions into `nodes`,
/// cleared on every resume (`EngineShared::resume`) so a stale reference can
/// never outlive the stop it was minted for.
pub struct VarStore {
    nodes: Mutex<Vec<VarNode>>,
}

impl VarStore {
    pub(crate) fn new() -> Self {
        Self {
            nodes: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn clear(&self) {
        self.nodes.lock().unwrap().clear();
    }

    fn insert(&self, node: VarNode) -> i64 {
        let mut nodes = self.nodes.lock().unwrap();
        nodes.push(node);
        nodes.len() as i64
    }

    fn get(&self, reference: i64) -> Option<VarNode> {
        let idx = usize::try_from(reference - 1).ok()?;
        self.nodes.lock().unwrap().get(idx).cloned()
    }
}

/// Named locals of the frame at `frame_idx` (`0` = innermost, matching
/// `EngineShared::stack()`), in cell order. Empty while not parked.
pub fn frame_variables(session: &EngineShared, frame_idx: usize) -> Vec<Variable> {
    let Some((fn_id, cells)) = session.frame_cells(frame_idx) else {
        return Vec::new();
    };
    session
        .fn_cells(fn_id)
        .iter()
        .enumerate()
        .map(|(cell_idx, cell)| {
            let raw = cells.get(cell_idx).copied().unwrap_or(0);
            let desc = session.cell_desc(fn_id, cell_idx);
            describe_with(session, cell.name.clone(), &cell.ty, raw, desc)
        })
        .collect()
}

/// Lazy children of a previously returned `Variable.reference`. Empty once
/// the store has been cleared by a resume, or if `reference` never named a
/// node (unknown handle, leaf variable).
pub fn children(session: &EngineShared, reference: i64) -> Vec<Variable> {
    let Some(node) = session.var_store.get(reference) else {
        return Vec::new();
    };
    children_raw(session, node.value, &node.ty)
        .into_iter()
        .map(|(name, val, ty)| describe_fresh(session, name, &ty, val))
        .collect()
}

/// Type-directed child descent shared by `children` (DAP/headless lazy
/// expansion, wraps each result into a rendered `Variable`) and
/// `conditions.rs`'s path resolver (wants the raw word and static type only,
/// evaluated before any stop -- there is no `Variable`/handle table yet).
pub(crate) fn children_raw(
    session: &EngineShared,
    value: i64,
    ty: &Type,
) -> Vec<(String, i64, Type)> {
    match concrete_ty(ty) {
        Type::List(inner) | Type::Vector(inner, _) | Type::Set(inner) => {
            let n = seq_len(session, value);
            (0..n)
                .map(|i| (i.to_string(), seq_get(session, value, i), (**inner).clone()))
                .collect()
        }
        Type::Tuple(items) => items
            .iter()
            .enumerate()
            .map(|(i, item_ty)| {
                (
                    i.to_string(),
                    seq_get(session, value, i as i64),
                    item_ty.clone(),
                )
            })
            .collect(),
        Type::Dict(k, v) => {
            let n = dict_len(session, value);
            (0..n)
                .map(|i| {
                    let kraw = dict_key(session, value, i);
                    let vraw = dict_val(session, value, i);
                    (render_value(session, k, kraw), vraw, (**v).clone())
                })
                .collect()
        }
        Type::Struct(name, _, _) => {
            let Some(fields) = session.struct_fields().get(name) else {
                return Vec::new();
            };
            fields
                .iter()
                .enumerate()
                .map(|(i, fname)| {
                    let fty = session
                        .field_types()
                        .get(&(name.clone(), fname.clone()))
                        .cloned()
                        .unwrap_or(Type::Any);
                    let fval = if value == 0 {
                        0
                    } else {
                        unsafe { *(value as *const i64).add(1 + i) }
                    };
                    (fname.clone(), fval, fty)
                })
                .collect()
        }
        Type::Enum(name, _) => {
            if value == 0 {
                return Vec::new();
            }
            let Some(variants) = session.enum_defs().get(name) else {
                return Vec::new();
            };
            let tag = enum_tag(session, value);
            let Some((_, payload_tys)) = variants.get(tag as usize) else {
                return Vec::new();
            };
            payload_tys
                .iter()
                .enumerate()
                .map(|(i, pty)| {
                    (
                        i.to_string(),
                        enum_payload(session, value, i as i64),
                        pty.clone(),
                    )
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn describe_fresh(session: &EngineShared, name: String, ty: &Type, raw: i64) -> Variable {
    let desc = build_descriptor(session, concrete_ty(ty));
    describe_with(session, name, ty, raw, &desc)
}

fn describe_with(
    session: &EngineShared,
    name: String,
    ty: &Type,
    raw: i64,
    desc: &CStr,
) -> Variable {
    let concrete = concrete_ty(ty);
    let coerced = coerce_bits(concrete, raw);
    let value = format_typed(session, coerced, desc);
    let reference = if is_expandable(concrete) {
        session.var_store.insert(VarNode {
            value: raw,
            ty: concrete.clone(),
        })
    } else {
        0
    };
    Variable {
        name,
        type_name: ty.to_string(),
        value,
        reference,
    }
}

fn render_value(session: &EngineShared, ty: &Type, raw: i64) -> String {
    let concrete = concrete_ty(ty);
    let desc = build_descriptor(session, concrete);
    format_typed(session, coerce_bits(concrete, raw), &desc)
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

fn is_expandable(ty: &Type) -> bool {
    matches!(
        ty,
        Type::List(_)
            | Type::Vector(_, _)
            | Type::Set(_)
            | Type::Tuple(_)
            | Type::Dict(_, _)
            | Type::Struct(_, _, _)
            | Type::Enum(_, _)
    )
}

/// `F32` cells arrive zero-extended to i64 (raw 32-bit bits in the low
/// word), not a valid `f64` bit pattern, the one case where `cl_type`'s
/// narrower-than-i64 width breaks the assumption every other scalar leaf
/// relies on (bool/i8/i16/i32/u8/u16/u32 are already sign/zero-extended to
/// their correct numeric value by `translate_call.rs`'s call coercion, so
/// they need no adjustment). Widening here lets the same runtime formatter
/// handle both float widths.
fn coerce_bits(ty: &Type, raw: i64) -> i64 {
    if matches!(ty, Type::F32) {
        (f32::from_bits(raw as u32) as f64).to_bits() as i64
    } else {
        raw
    }
}

fn format_typed(session: &EngineShared, val: i64, desc: &CStr) -> String {
    let Some(ptr) = session.runtime_symbol("olive_format_typed") else {
        return String::new();
    };
    let f: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    read_rendered(f(val, desc.as_ptr() as i64))
}

/// Every `olive_format_typed` result is a freshly interned, tag-bit-set
/// olive string with no embedded NUL (`olive_str_internal` strips them) and
/// a guaranteed trailing one (`string_slab::str_alloc`), so a plain `CStr`
/// read is safe without duplicating the slab/literal layout distinction
/// `olive_str_to_bytes` makes.
fn read_rendered(ptr: i64) -> String {
    if ptr == 0 {
        return String::new();
    }
    let masked = (ptr & !1) as *const std::os::raw::c_char;
    unsafe { CStr::from_ptr(masked) }
        .to_string_lossy()
        .into_owned()
}

fn call1(session: &EngineShared, name: &str, a: i64) -> i64 {
    let Some(ptr) = session.runtime_symbol(name) else {
        return 0;
    };
    let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    f(a)
}

fn call2(session: &EngineShared, name: &str, a: i64, b: i64) -> i64 {
    let Some(ptr) = session.runtime_symbol(name) else {
        return 0;
    };
    let f: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    f(a, b)
}

fn seq_len(session: &EngineShared, val: i64) -> i64 {
    call1(session, "olive_debug_seq_len", val)
}

fn seq_get(session: &EngineShared, val: i64, idx: i64) -> i64 {
    call2(session, "olive_debug_seq_get", val, idx)
}

fn dict_len(session: &EngineShared, val: i64) -> i64 {
    call1(session, "olive_debug_dict_len", val)
}

fn dict_key(session: &EngineShared, val: i64, idx: i64) -> i64 {
    call2(session, "olive_debug_dict_key", val, idx)
}

fn dict_val(session: &EngineShared, val: i64, idx: i64) -> i64 {
    call2(session, "olive_debug_dict_val", val, idx)
}

fn enum_tag(session: &EngineShared, val: i64) -> i64 {
    call1(session, "olive_debug_enum_tag", val)
}

fn enum_payload(session: &EngineShared, val: i64, idx: i64) -> i64 {
    call2(session, "olive_debug_enum_payload", val, idx)
}

/// Raw UTF-8 text of a string value, for `conditions.rs`'s string literal
/// comparisons -- rendering through `format_typed` would add quotes and
/// escapes meant for display, not equality.
pub(crate) fn str_value(session: &EngineShared, raw: i64) -> String {
    let Some(ptr) = session.runtime_symbol("olive_debug_str_bytes") else {
        return String::new();
    };
    let f: extern "C" fn(i64, *mut i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    let mut len: i64 = 0;
    let data = f(raw, &mut len as *mut i64);
    if data == 0 || len <= 0 {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, len as usize) };
    String::from_utf8_lossy(bytes).into_owned()
}
