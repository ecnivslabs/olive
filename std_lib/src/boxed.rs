//! Scalar representation inside an `Any` slot. A bare `i64` is ambiguous once
//! its static type is erased: a large odd int is bit-identical to a low-bit
//! tagged string pointer. Heap objects are 8-aligned (low 3 bits clear) and
//! strings carry bit 0, which leaves three low-bit patterns free for inline
//! immediates that need no allocation:
//!
//! * `TAG_INT`  (`& 7 == 2`): a 61-bit signed integer in the high bits.
//! * `TAG_BOOL` (`& 7 == 4`): `0` or `1` in the high bits.
//! * `TAG_NULL` (`& 7 == 6`): the sole `null` word, exactly `TAG_NULL`.
//!
//! An int outside the 61-bit range falls back to a heap `OliveBoxed` (`KIND_INT`)
//! and a float always uses one (`KIND_FLOAT`), since neither fits beside a tag.
//! Only these heap forms and ordinary pointers are tracked; an immediate is a
//! plain register value with no lifetime.

use crate::slab::GenSlab;
use crate::{KIND_FLOAT, KIND_INT, is_active_object, olive_str_from_ptr};
use std::cell::UnsafeCell;

thread_local! {
    static BOXED_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::new(std::mem::size_of::<OliveBoxed>())) };
}

/// Low-bit tag selecting an inline immediate. Heap pointers use `0`, strings
/// use bit `0`; these three are the remaining even, non-zero patterns.
pub const TAG_INT: i64 = 2;
pub const TAG_BOOL: i64 = 4;
pub const TAG_NULL: i64 = 6;
pub const TAG_MASK: i64 = 7;

/// Inclusive bounds of an inline `TAG_INT` payload (61-bit signed). Anything
/// wider is heap-boxed so no value is silently truncated.
const INT_MIN: i64 = -(1 << 60);
const INT_MAX: i64 = (1 << 60) - 1;

/// A scalar too wide to inline. `bits` is the integer (`KIND_INT`) or the float
/// bit pattern (`KIND_FLOAT`).
#[repr(C)]
pub struct OliveBoxed {
    pub kind: i64,
    pub bits: i64,
}

fn heap_box(kind: i64, bits: i64) -> i64 {
    BOXED_SLAB.with(|sl| {
        let sl = unsafe { &mut *sl.get() };
        let (body, _) = sl.alloc();
        unsafe {
            std::ptr::write(body as *mut OliveBoxed, OliveBoxed { kind, bits });
        }
        body as i64
    })
}

/// Encodes an integer for an `Any` slot: inline when it fits 61 bits, otherwise
/// a heap `KIND_INT` box.
#[unsafe(no_mangle)]
pub extern "C" fn olive_box_int(i: i64) -> i64 {
    if (INT_MIN..=INT_MAX).contains(&i) {
        (i << 3) | TAG_INT
    } else {
        heap_box(KIND_INT, i)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_box_float(f: f64) -> i64 {
    heap_box(KIND_FLOAT, f.to_bits() as i64)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_box_bool(b: i64) -> i64 {
    (((b != 0) as i64) << 3) | TAG_BOOL
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_box_null() -> i64 {
    TAG_NULL
}

/// Whether an `Any` is `null`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_any_is_null(v: i64) -> i64 {
    (v == TAG_NULL) as i64
}

/// `float()` of an `Any`: unbox, parse a string, or widen an integer.
#[unsafe(no_mangle)]
pub extern "C" fn olive_unbox_float(v: i64) -> f64 {
    match v & TAG_MASK {
        TAG_INT | TAG_BOOL => return (v >> 3) as f64,
        TAG_NULL => return 0.0,
        _ => {}
    }
    if let Some(b) = as_boxed(v) {
        return match b.kind {
            KIND_FLOAT => f64::from_bits(b.bits as u64),
            _ => b.bits as f64,
        };
    }
    if is_str(v) {
        return olive_str_from_ptr(v).trim().parse::<f64>().unwrap_or(0.0);
    }
    if is_pyobject(v) {
        return crate::python::olive_py_to_float(v as *mut std::os::raw::c_void);
    }
    v as f64
}

/// `int()` of an `Any`: unbox (float truncates), parse a string, or pass an
/// integer through.
#[unsafe(no_mangle)]
pub extern "C" fn olive_unbox_int(v: i64) -> i64 {
    match v & TAG_MASK {
        TAG_INT | TAG_BOOL => return v >> 3,
        TAG_NULL => return 0,
        _ => {}
    }
    if let Some(b) = as_boxed(v) {
        return match b.kind {
            KIND_FLOAT => f64::from_bits(b.bits as u64) as i64,
            _ => b.bits,
        };
    }
    if is_str(v) {
        return olive_str_from_ptr(v).trim().parse::<i64>().unwrap_or(0);
    }
    if is_pyobject(v) {
        return crate::python::olive_py_to_int(v as *mut std::os::raw::c_void);
    }
    v
}

/// Tagged Olive string pointer, not a raw scalar.
fn is_str(v: i64) -> bool {
    v & 1 == 1 && (v & !1) > 0x10000
}

/// True when an Any slot holds a Python object handle that must be unwrapped
/// before reading as a number.
fn is_pyobject(v: i64) -> bool {
    is_active_object(v) && unsafe { *(v as *const i64) } == crate::KIND_PYOBJECT
}

/// Truthiness of an `Any`: by value for an inline scalar or boxed float,
/// non-empty for strings, present otherwise. `null` and `0` are false.
#[unsafe(no_mangle)]
pub extern "C" fn olive_any_truthy(v: i64) -> i64 {
    match v & TAG_MASK {
        TAG_INT | TAG_BOOL => return (v >> 3 != 0) as i64,
        TAG_NULL => return 0,
        _ => {}
    }
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

/// Frees a heap scalar from `heap_box`; inline immediates own nothing.
pub fn olive_free_boxed(ptr: i64) {
    if ptr != 0 && ptr & TAG_MASK == 0 && crate::slab::ptr_in_slab_span(ptr) {
        BOXED_SLAB.with(|sl| {
            unsafe { &mut *sl.get() }.free(ptr as *mut u8);
        });
    }
}

/// Borrows `v` as a heap-boxed scalar if it is one.
fn as_boxed(v: i64) -> Option<&'static OliveBoxed> {
    if !is_active_object(v) {
        return None;
    }
    let b = unsafe { &*(v as *const OliveBoxed) };
    matches!(b.kind, KIND_FLOAT | KIND_INT).then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_int_roundtrip() {
        for i in [0, 1, -1, 42, -42, 1 << 40, -(1 << 40), INT_MAX, INT_MIN] {
            let p = olive_box_int(i);
            assert_eq!(p & TAG_MASK, TAG_INT, "{i} should be inline");
            assert_eq!(olive_unbox_int(p), i);
            assert_eq!(olive_unbox_float(p), i as f64);
            assert_eq!(olive_any_is_null(p), 0);
        }
    }

    #[test]
    fn big_int_falls_back_to_heap() {
        let big = INT_MAX + 1;
        let p = olive_box_int(big);
        assert_eq!(p & TAG_MASK, 0, "out-of-range int should be heap-boxed");
        assert_eq!(olive_unbox_int(p), big);
        olive_free_boxed(p);
    }

    #[test]
    fn large_odd_int_is_never_a_string() {
        // The bug inline tagging exists to kill: a large odd int read as a
        // tagged string pointer. As an immediate it carries `TAG_INT`, so its
        // low bit is clear and the string heuristic can never match it.
        let big = 200_000_001;
        let p = olive_box_int(big);
        assert_eq!(p & 1, 0);
        assert_eq!(olive_unbox_int(p), big);
    }

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
        assert_eq!(t & TAG_MASK, TAG_BOOL);
        assert_eq!(olive_unbox_int(t), 1);
        assert_eq!(olive_unbox_int(f), 0);
        assert_eq!(olive_any_truthy(t), 1);
        assert_eq!(olive_any_truthy(f), 0);
    }

    #[test]
    fn null_is_distinct_from_int_zero() {
        let n = olive_box_null();
        let z = olive_box_int(0);
        assert_eq!(olive_any_is_null(n), 1);
        assert_eq!(olive_any_is_null(z), 0);
        assert_eq!(olive_any_is_null(0), 0, "bare 0 is the integer zero");
        assert_eq!(olive_any_truthy(n), 0, "null is falsy");
        assert_eq!(olive_any_truthy(z), 0, "zero is falsy");
    }
}
