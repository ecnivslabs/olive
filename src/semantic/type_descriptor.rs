//! Type-to-byte-descriptor encoding, shared by codegen's typed free/copy/print
//! dispatch and the MIR builder (closure-record capture layout, E5.2). Pure
//! `Type` logic with no Cranelift dependency, which is why it lives here
//! rather than under `codegen`: both layers need it without one depending on
//! the other's internals.

use crate::semantic::types::Type as OliveType;
use rustc_hash::FxHashMap as HashMap;

/// Peels `Ref`/`MutRef` wrappers and reduces a `T | None` union to its single
/// non-null member `T`, since that member's raw representation is exactly
/// what gets stored at runtime (`None` is the null-pointer sentinel). Every
/// site that dispatches a runtime call by matching an operand's static type
/// must go through this first, or a unioned value picks the wrong dispatch. A
/// union with more than one non-null member has no single raw layout and is
/// left as `Union` for the boxed-`Any` dispatch paths.
pub(crate) fn concrete_ty(ty: &OliveType) -> &OliveType {
    let mut ty = ty;
    loop {
        match ty {
            OliveType::Ref(inner) | OliveType::MutRef(inner) => ty = inner,
            OliveType::Union(members) => {
                let non_null: Vec<&OliveType> = members
                    .iter()
                    .filter(|m| !matches!(m, OliveType::Null))
                    .collect();
                match non_null.as_slice() {
                    [single] => ty = single,
                    _ => break,
                }
            }
            _ => break,
        }
    }
    ty
}

/// Collections store elements raw, so `print`/`str` need the static type to
/// render them. These are the types routed through the typed formatter.
pub(crate) fn needs_type_descriptor(ty: &OliveType) -> bool {
    let ty = concrete_ty(ty);
    matches!(
        ty,
        OliveType::List(_)
            | OliveType::Set(_)
            | OliveType::Tuple(_)
            | OliveType::Dict(_, _)
            | OliveType::Struct(_, _, _)
            | OliveType::Enum(_, _)
            | OliveType::Bytes
    )
}

type StructFields = HashMap<String, Vec<String>>;
type FieldTypes = HashMap<(String, String), OliveType>;
type EnumDefs = HashMap<String, Vec<(String, Vec<OliveType>)>>;

/// Encodes a type as the byte descriptor consumed by `olive_format_typed`,
/// `olive_free_typed`, and `olive_copy_typed`. All bytes are non-zero so the
/// descriptor interns as a NUL-terminated string; length bytes are biased by
/// 13 to clear the tag range and the NUL.
pub(crate) fn type_descriptor(
    ty: &OliveType,
    struct_fields: &StructFields,
    field_types: &FieldTypes,
    enum_defs: &EnumDefs,
) -> String {
    let mut out = Vec::new();
    let mut visiting = HashMap::default();
    encode_descriptor(
        ty,
        &mut out,
        struct_fields,
        field_types,
        enum_defs,
        &mut visiting,
    );
    out.into_iter().map(|b| b as char).collect()
}

fn push_len_prefixed(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    out.push((bytes.len().min(242) + 13) as u8);
    out.extend_from_slice(&bytes[..bytes.len().min(242)]);
}

fn encode_descriptor(
    ty: &OliveType,
    out: &mut Vec<u8>,
    struct_fields: &StructFields,
    field_types: &FieldTypes,
    enum_defs: &EnumDefs,
    visiting: &mut HashMap<String, usize>,
) {
    let mut ty = ty;
    while let OliveType::Ref(inner) | OliveType::MutRef(inner) | OliveType::Ptr(inner) = ty {
        ty = inner;
    }
    let enc = |t: &OliveType, out: &mut Vec<u8>, v: &mut HashMap<String, usize>| {
        encode_descriptor(t, out, struct_fields, field_types, enum_defs, v);
    };
    match ty {
        OliveType::Float | OliveType::F32 | OliveType::FloatLiteral(_) => out.push(2),
        OliveType::Bool => out.push(3),
        OliveType::Str => out.push(4),
        OliveType::Null => out.push(5),
        OliveType::Bytes => out.push(15),
        // `T | None` is not a boxed, kind-tagged `Any`: it stores `T`'s own
        // raw representation with `0` as the `None` sentinel (see
        // translate.rs). Descriptor-wise that means encoding `T` directly;
        // every `copy_typed`/`free_typed` leaf already treats a `0` operand
        // as a no-op, so the sentinel needs no descriptor bits of its own.
        // A union with more than one non-null member has no single raw
        // representation to fall back on, so it stays boxed-`Any`-style.
        OliveType::Union(members) => {
            let non_null: Vec<&OliveType> =
                members.iter().filter(|m| **m != OliveType::Null).collect();
            if non_null.len() == 1 {
                enc(non_null[0], out, visiting);
            } else {
                out.push(6)
            }
        }
        OliveType::Any | OliveType::PyObject | OliveType::PyNamed(_, _) => out.push(6),
        OliveType::List(inner) | OliveType::Vector(inner, _) => {
            out.push(7);
            enc(inner, out, visiting);
        }
        OliveType::Set(inner) => {
            out.push(8);
            enc(inner, out, visiting);
        }
        OliveType::Dict(k, v) => {
            out.push(9);
            enc(k, out, visiting);
            enc(v, out, visiting);
        }
        OliveType::Tuple(items) => {
            out.push(10);
            out.push((items.len() + 1) as u8);
            for it in items {
                enc(it, out, visiting);
            }
        }
        OliveType::Struct(name, _, _)
            if struct_fields.contains_key(name) && visiting.contains_key(name) =>
        {
            // Recursion detected, emit a back-reference offset.
            let target_idx = visiting[name];
            out.push(14);
            out.push((target_idx >> 8) as u8);
            out.push((target_idx & 0xff) as u8);
        }
        OliveType::Struct(name, _, _) if struct_fields.contains_key(name) => {
            let start_idx = out.len();
            visiting.insert(name.clone(), start_idx);
            let fields = &struct_fields[name];
            out.push(12);
            push_len_prefixed(out, name);
            out.push((fields.len() + 13) as u8);
            for f in fields {
                push_len_prefixed(out, f);
                let fty = field_types
                    .get(&(name.clone(), f.clone()))
                    .cloned()
                    .unwrap_or(OliveType::Any);
                enc(&fty, out, visiting);
            }
            visiting.remove(name);
        }
        OliveType::Enum(name, _) if enum_defs.contains_key(name) && visiting.contains_key(name) => {
            // Recursion detected, emit a back-reference offset.
            let target_idx = visiting[name];
            out.push(14);
            out.push((target_idx >> 8) as u8);
            out.push((target_idx & 0xff) as u8);
        }
        OliveType::Enum(name, _) if enum_defs.contains_key(name) => {
            let start_idx = out.len();
            visiting.insert(name.clone(), start_idx);
            let variants = &enum_defs[name];
            out.push(13);
            push_len_prefixed(out, name);
            out.push((variants.len() + 13) as u8);
            for (v_name, payloads) in variants {
                push_len_prefixed(out, v_name);
                out.push((payloads.len() + 13) as u8);
                for pty in payloads {
                    enc(pty, out, visiting);
                }
            }
            visiting.remove(name);
        }
        OliveType::Struct(_, _, _) | OliveType::Enum(_, _) => out.push(11),
        // An inference variable that never got bound could hold anything;
        // claiming a scalar here would skip frees and print raw words.
        OliveType::Var(_) => out.push(6),
        _ => out.push(1),
    }
}
