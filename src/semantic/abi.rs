//! Whether a type can cross the C FFI boundary. A foreign function declared in
//! a native import must take and return things C can actually represent: scalar
//! integers and floats, booleans, pointers, strings (as `char*`), byte buffers,
//! and C structs. Olive's managed types (lists, dicts, sets, tuples, enums,
//! closures, Python values) carry runtime headers and ownership that no C ABI
//! understands, so declaring one at the boundary is always a mistake.
//!
//! The check is a deliberate blocklist: only the types that are unambiguously
//! non-representable are rejected, so a legitimate (if unusual) declaration is
//! never flagged by accident.

use super::types::Type;

/// Why a type is not FFI-safe, phrased for the diagnostic that reports it.
pub fn ffi_unsafe_reason(ty: &Type) -> Option<&'static str> {
    match ty {
        Type::List(_) => Some("a list is a managed, growable container with no C layout"),
        Type::Dict(_, _) => Some("a dict is a managed hash table with no C layout"),
        Type::Set(_) => Some("a set is a managed hash table with no C layout"),
        Type::Tuple(_) => Some("a tuple is a managed Olive value, not a C struct"),
        Type::Enum(_, _) => {
            Some("an Olive enum is a tagged value, not a C enum (use `const` ints)")
        }
        Type::Union(_) => Some("a union of Olive types has no single C representation"),
        Type::TraitObject(_, _) => Some("a trait object is a fat pointer with an Olive vtable"),
        Type::Fn(_, _, _) => {
            Some("an Olive closure carries captured state; pass a raw `ptr` instead")
        }
        Type::Vector(_, _) => Some("a SIMD vector has no portable C parameter ABI"),
        Type::Future(_) => Some("a future is an Olive runtime value with no C representation"),
        Type::PyObject | Type::PyNamed(_, _) => {
            Some("a Python value is owned by the interpreter and cannot cross to C")
        }
        Type::Param(_) => Some("a generic type parameter has no fixed C layout"),
        Type::Ref(inner) | Type::MutRef(inner) => ffi_unsafe_reason(inner),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safe(ty: &Type) -> bool {
        ffi_unsafe_reason(ty).is_none()
    }

    #[test]
    fn scalars_and_pointers_are_safe() {
        assert!(safe(&Type::Int));
        assert!(safe(&Type::F32));
        assert!(safe(&Type::Bool));
        assert!(safe(&Type::Str));
        assert!(safe(&Type::Bytes));
        assert!(safe(&Type::Ptr(Box::new(Type::Int))));
        assert!(safe(&Type::Null));
    }

    #[test]
    fn managed_types_are_unsafe() {
        assert!(!safe(&Type::List(Box::new(Type::Int))));
        assert!(!safe(&Type::Dict(Box::new(Type::Str), Box::new(Type::Int))));
        assert!(!safe(&Type::Set(Box::new(Type::Int))));
        assert!(!safe(&Type::Tuple(vec![Type::Int, Type::Int])));
        assert!(!safe(&Type::PyObject));
        assert!(!safe(&Type::Param("T".into())));
    }

    #[test]
    fn pointer_to_managed_is_explicit_and_allowed() {
        // A raw pointer is an address; the programmer owns its meaning.
        assert!(safe(&Type::Ptr(Box::new(Type::List(Box::new(Type::Int))))));
    }

    #[test]
    fn reference_to_managed_is_unsafe() {
        assert!(!safe(&Type::Ref(Box::new(Type::List(Box::new(Type::Int))))));
    }

    #[test]
    fn reason_is_descriptive() {
        assert!(
            ffi_unsafe_reason(&Type::List(Box::new(Type::Int)))
                .unwrap()
                .contains("list")
        );
        assert!(ffi_unsafe_reason(&Type::Int).is_none());
    }
}

/// One field's C layout: (field name, byte offset, primitive type name,
/// optional bitfield `(bit offset, bit width)`). Shared by the type checker
/// (validating construction) and codegen (field access and by-value passing).
pub type FfiFieldLayout = (String, i32, String, Option<(u8, u8)>);

fn c_prim_layout(ty: &str) -> (i32, i32) {
    match ty {
        "f64" | "i64" | "u64" | "ptr" => (8, 8),
        "f32" | "i32" | "u32" => (4, 4),
        "i16" | "u16" => (2, 2),
        "i8" | "u8" | "bool" => (1, 1),
        _ if ty.starts_with('[') => {
            if let Some(semi) = ty.find(';') {
                let elem = &ty[1..semi];
                let n: i32 = ty[semi + 1..ty.len() - 1].parse().unwrap_or(1);
                let (elem_size, elem_align) = c_prim_layout(elem);
                (elem_size * n, elem_align)
            } else {
                (8, 8)
            }
        }
        _ => (8, 8),
    }
}

pub fn type_expr_to_name(t: &crate::parser::ast::TypeExpr) -> String {
    use crate::parser::ast::TypeExprKind;
    match &t.kind {
        TypeExprKind::Name(n) => n.clone(),
        TypeExprKind::Ref(inner) | TypeExprKind::MutRef(inner) => type_expr_to_name(inner),
        TypeExprKind::Ptr(_) => "ptr".to_string(),
        TypeExprKind::FixedArray(inner, n) => format!("[{};{}]", type_expr_to_name(inner), n),
        _ => "int".to_string(),
    }
}

/// Offsets and total size of an import-block struct per the platform C ABI.
/// Bitfields pack left-to-right into words of their declared width, matching
/// GCC/Clang for the scalar bitfields Olive allows.
pub fn c_abi_layout(
    fields: &[crate::parser::ast::FfiStructField],
    is_union: bool,
) -> (Vec<FfiFieldLayout>, i64) {
    if is_union {
        let mut max_size = 0i32;
        let mut max_align = 1i32;
        let mut layout = Vec::new();
        for field in fields {
            let ty = type_expr_to_name(&field.ty);
            let (size, align) = c_prim_layout(&ty);
            max_align = max_align.max(align);
            max_size = max_size.max(size);
            layout.push((field.name.clone(), 0, ty.clone(), None));
        }
        let total = if max_align > 0 {
            let r = max_size % max_align;
            if r == 0 {
                max_size
            } else {
                max_size + max_align - r
            }
        } else {
            max_size
        };
        return (layout, total as i64);
    }
    let mut offset = 0i32;
    let mut layout = Vec::new();
    let mut max_align = 1i32;
    let mut current_bit_offset = 0i32;
    let mut last_bitfield_size = 0i32;

    for field in fields {
        let ty = type_expr_to_name(&field.ty);
        let (size, align) = c_prim_layout(&ty);
        max_align = max_align.max(align);

        if let Some(bits) = field.bits {
            if current_bit_offset == 0
                || (current_bit_offset + (bits as i32) > last_bitfield_size * 8)
                || size != last_bitfield_size
            {
                let padding = (align - (offset % align)) % align;
                offset += padding;
                layout.push((field.name.clone(), offset, ty.clone(), Some((0u8, bits))));
                last_bitfield_size = size;
                current_bit_offset = bits as i32;
                offset += size;
            } else {
                let word_offset = offset - last_bitfield_size;
                let bit_off = current_bit_offset as u8;
                layout.push((
                    field.name.clone(),
                    word_offset,
                    ty.clone(),
                    Some((bit_off, bits)),
                ));
                current_bit_offset += bits as i32;
            }
        } else {
            current_bit_offset = 0;
            last_bitfield_size = 0;
            let padding = (align - (offset % align)) % align;
            offset += padding;
            layout.push((field.name.clone(), offset, ty.clone(), None));
            offset += size;
        }
    }
    let total = if max_align > 0 {
        let r = offset % max_align;
        if r == 0 {
            offset
        } else {
            offset + max_align - r
        }
    } else {
        offset
    };
    (layout, total as i64)
}

pub(crate) fn is_float_name(ty: &str) -> bool {
    ty == "float" || ty == "f32" || ty == "f64" || ty == "double"
}

/// SysV AMD64 classification of one eightbyte of an aggregate passed by
/// value: whether it goes in an SSE register (`true`) or a general-purpose
/// register (`false`). Every member overlapping the eightbyte contributes
/// its class and INTEGER wins over SSE, so `union {long; double}` is
/// INTEGER while `struct {double}` is SSE -- matching GCC/Clang for all the
/// scalar members Olive's import blocks support.
pub fn eightbyte_is_sse(layout: &[FfiFieldLayout], eightbyte_index: usize) -> bool {
    let start = (eightbyte_index * 8) as i32;
    let end = start + 8;
    let mut overlapped = false;
    for &(_, off, ref ty, _) in layout {
        if off < end && off + c_prim_layout(ty).0 > start {
            overlapped = true;
            if !is_float_name(ty) {
                return false;
            }
        }
    }
    overlapped
}

/// Register width an SSE-classified eightbyte occupies: the byte span its
/// members actually cover inside the eightbyte. `Vec2 {f32, f32}` covers all
/// 8 bytes (GCC packs both into the low half of one XMM register via a 64-bit
/// move) while a lone `union {f32}` covers 4 and travels in the low 32 bits.
pub(crate) fn c_abi_eightbyte_size(layout: &[FfiFieldLayout], eightbyte_index: usize) -> i32 {
    let start = (eightbyte_index * 8) as i32;
    let end = start + 8;
    layout
        .iter()
        .filter(|&&(_, off, ref ty, _)| off < end && off + c_prim_layout(ty).0 > start)
        .map(|&(_, off, ref ty, _)| off + c_prim_layout(ty).0 - start)
        .max()
        .unwrap_or(8)
}

#[cfg(test)]
mod eightbyte_tests {
    use super::*;

    fn layout(fields: &[(&str, &str)]) -> Vec<FfiFieldLayout> {
        fields
            .iter()
            .map(|&(n, t)| (n.to_string(), 0, t.to_string(), None))
            .collect()
    }

    #[test]
    fn pure_float_union_is_sse() {
        let l = vec![
            ("d".to_string(), 0, "f64".to_string(), None),
        ];
        assert!(eightbyte_is_sse(&l, 0));
        assert_eq!(c_abi_eightbyte_size(&l, 0), 8);
    }

    #[test]
    fn mixed_union_is_integer_integer_wins_over_sse() {
        let l = vec![
            ("l".to_string(), 0, "i64".to_string(), None),
            ("d".to_string(), 0, "f64".to_string(), None),
        ];
        assert!(!eightbyte_is_sse(&l, 0));
    }

    #[test]
    fn float_pair_spans_full_eightbyte() {
        let l = vec![
            ("x".to_string(), 0, "f32".to_string(), None),
            ("y".to_string(), 4, "f32".to_string(), None),
        ];
        assert!(eightbyte_is_sse(&l, 0));
        assert_eq!(c_abi_eightbyte_size(&l, 0), 8);
    }

    #[test]
    fn lone_f32_uses_low_half_only() {
        let l = vec![("d".to_string(), 0, "f32".to_string(), None)];
        assert!(eightbyte_is_sse(&l, 0));
        assert_eq!(c_abi_eightbyte_size(&l, 0), 4);
    }

    #[test]
    fn second_eightbyte_classified_independently() {
        let l = vec![
            ("t".to_string(), 0, "i64".to_string(), None),
            ("v".to_string(), 8, "f64".to_string(), None),
        ];
        assert!(!eightbyte_is_sse(&l, 0));
        assert!(eightbyte_is_sse(&l, 1));
        assert_eq!(c_abi_eightbyte_size(&l, 1), 8);
    }
}
