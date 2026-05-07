//! Boxed scalars for `Any` containers. Inside a list/dict/set every slot is a
//! raw `i64`, so a float (bits look like an int), a bool, or `null` (looks like
//! int 0) is boxed behind a kind header to stay self-describing. Ints and
//! pointers already are, so they're never boxed.

use crate::{
    KIND_BOOL, KIND_FLOAT, KIND_NULL, is_active_object, olive_str_from_ptr, register_object,
};

/// A scalar inside an `Any`. `bits` is the float bit pattern (`KIND_FLOAT`),
/// `0`/`1` (`KIND_BOOL`), or unused (`KIND_NULL`).
#[repr(C)]
pub struct OliveBoxed {
    pub kind: i64,
    pub bits: i64,
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_box_float(f: f64) -> i64 {
    let res = Box::into_raw(Box::new(OliveBoxed {
        kind: KIND_FLOAT,
        bits: f.to_bits() as i64,
    })) as i64;
    register_object(res);
    res
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_box_bool(b: i64) -> i64 {
    let res = Box::into_raw(Box::new(OliveBoxed {
        kind: KIND_BOOL,
        bits: (b != 0) as i64,
    })) as i64;
    register_object(res);
    res
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_box_null() -> i64 {
    let res = Box::into_raw(Box::new(OliveBoxed {
        kind: KIND_NULL,
        bits: 0,
    })) as i64;
    register_object(res);
    res
}

/// Whether an `Any` is `null`. A bare `0` is the integer zero; `null` is always
/// boxed.
#[unsafe(no_mangle)]
pub extern "C" fn olive_any_is_null(v: i64) -> i64 {
    (is_active_object(v) && unsafe { *(v as *const i64) } == KIND_NULL) as i64
}

/// `float()` of an `Any`: unbox, parse a string, or widen a raw int.
#[unsafe(no_mangle)]
pub extern "C" fn olive_unbox_float(v: i64) -> f64 {
    if let Some(b) = as_boxed(v) {
        return match b.kind {
            KIND_FLOAT => f64::from_bits(b.bits as u64),
            _ => b.bits as f64,
        };
    }
    if is_str(v) {
        return olive_str_from_ptr(v).trim().parse::<f64>().unwrap_or(0.0);
    }
    v as f64
}

/// `int()` of an `Any`: unbox (float truncates), parse a string, or pass a raw
/// int through.
#[unsafe(no_mangle)]
pub extern "C" fn olive_unbox_int(v: i64) -> i64 {
    if let Some(b) = as_boxed(v) {
        return match b.kind {
            KIND_FLOAT => f64::from_bits(b.bits as u64) as i64,
            _ => b.bits,
        };
    }
    if is_str(v) {
        return olive_str_from_ptr(v).trim().parse::<i64>().unwrap_or(0);
    }
    v
}

/// Tagged Olive string pointer, not a raw scalar.
fn is_str(v: i64) -> bool {
    v & 1 == 1 && (v & !1) > 0x10000
}

/// Truthiness of an `Any`: by value for boxed bool/float, non-empty for strings,
/// present otherwise. `0` is false.
#[unsafe(no_mangle)]
pub extern "C" fn olive_any_truthy(v: i64) -> i64 {
    if v == 0 {
        return 0;
    }
    if let Some(b) = as_boxed(v) {
        let t = match b.kind {
            KIND_FLOAT => f64::from_bits(b.bits as u64) != 0.0,
            _ => b.bits != 0,
        };
        return t as i64;
    }
    if is_str(v) {
        return (!olive_str_from_ptr(v).is_empty()) as i64;
    }
    1
}

/// Frees a scalar from one of the `olive_box_*` constructors.
pub fn olive_free_boxed(ptr: i64) {
    if ptr != 0 {
        unsafe {
            drop(Box::from_raw(ptr as *mut OliveBoxed));
        }
    }
}

/// Borrows `v` as a boxed scalar if it is one.
fn as_boxed(v: i64) -> Option<&'static OliveBoxed> {
    if !is_active_object(v) {
        return None;
    }
    let b = unsafe { &*(v as *const OliveBoxed) };
    matches!(b.kind, KIND_FLOAT | KIND_BOOL | KIND_NULL).then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_unbox_float_roundtrip() {
        let p = olive_box_float(2.5);
        assert_eq!(olive_unbox_float(p), 2.5);
        olive_free_boxed(p);
    }

    #[test]
    fn box_unbox_bool_roundtrip() {
        let t = olive_box_bool(1);
        let f = olive_box_bool(0);
        assert_eq!(olive_unbox_int(t), 1);
        assert_eq!(olive_unbox_int(f), 0);
        olive_free_boxed(t);
        olive_free_boxed(f);
    }

    #[test]
    fn unbox_raw_passthrough() {
        assert_eq!(olive_unbox_int(42), 42);
        assert_eq!(olive_unbox_float(42), 42.0);
    }

    #[test]
    fn boxed_null_is_distinct_from_int_zero() {
        let n = olive_box_null();
        assert_eq!(olive_any_is_null(n), 1, "boxed null is null");
        assert_eq!(
            olive_any_is_null(0),
            0,
            "bare 0 is the integer zero, not null"
        );
        assert_eq!(olive_any_is_null(42), 0);
        assert_eq!(olive_any_truthy(n), 0, "null is falsy");
        olive_free_boxed(n);
    }

    #[test]
    fn boxed_bool_truthiness() {
        let t = olive_box_bool(1);
        let f = olive_box_bool(0);
        assert_eq!(olive_any_truthy(t), 1);
        assert_eq!(olive_any_truthy(f), 0);
        olive_free_boxed(t);
        olive_free_boxed(f);
    }
}
