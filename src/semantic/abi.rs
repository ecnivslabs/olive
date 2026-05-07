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
