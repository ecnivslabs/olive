use crate::panic::abort_unwrap;
use crate::slab::GenSlab;
use crate::{olive_str_from_ptr, olive_str_internal};
use std::cell::UnsafeCell;

pub(crate) const KIND_RESULT: i64 = 9;

#[repr(C)]
pub struct OliveResult {
    pub kind: i64,
    pub tag: i64,
    pub payload: i64,
}

thread_local! {
    static RESULT_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::new(std::mem::size_of::<OliveResult>())) };
}

fn with_result_slab<T>(f: impl FnOnce(&mut GenSlab) -> T) -> T {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            f(&mut (*active).result)
        } else {
            RESULT_SLAB.with(|sl| f(&mut *sl.get()))
        }
    }
}

pub(crate) fn owns_result(v: i64) -> bool {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            if (*active).result.owns_addr(v as usize) {
                return true;
            }
            let source = crate::slab::SOURCE_SLABS.get();
            if !source.is_null() && (*source).result.owns_addr(v as usize) {
                return true;
            }
            if crate::slab::active_slab_is_global() {
                return RESULT_SLAB.with(|sl| (*sl.get()).owns_addr(v as usize));
            }
            return crate::slab::global_result_owns_addr(v as usize);
        }
        RESULT_SLAB.with(|sl| (*sl.get()).owns_addr(v as usize))
            || crate::slab::global_result_owns_addr(v as usize)
    }
}

fn make_result(ok: bool, payload: i64) -> i64 {
    with_result_slab(|sl| {
        let (body, _) = sl.alloc();
        unsafe {
            std::ptr::write(
                body as *mut OliveResult,
                OliveResult {
                    kind: KIND_RESULT,
                    tag: if ok { 1 } else { 0 },
                    payload,
                },
            );
        }
        body as i64
    })
}

/// Releases a result nobody consumed. `olive_free_any` classifies every wire
/// word (tagged string, immediate, slab body) and no-ops on what it doesn't
/// own, so the payload needs no gating of its own -- gating on
/// `is_active_object` here would leak `Err` message strings, whose tagged
/// pointers never classify as slab bodies.
#[unsafe(no_mangle)]
pub extern "C" fn olive_free_result(ptr: i64) {
    if !crate::is_kind(ptr, KIND_RESULT) {
        return;
    }
    let payload = unsafe { (*(ptr as *const OliveResult)).payload };
    free_slot(ptr);
    crate::olive_free_any(payload);
}

fn free_result_slot_local(ptr: i64) {
    with_result_slab(|sl| {
        sl.free(ptr as *mut u8);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_ok(val: i64) -> i64 {
    make_result(true, val)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_err(msg: i64) -> i64 {
    make_result(false, msg)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_is_ok(r: i64) -> i64 {
    if r == 0 {
        return 0;
    }
    if !crate::is_kind(r, KIND_RESULT) {
        return 0;
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    obj.tag
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_is_err(r: i64) -> i64 {
    if !crate::is_kind(r, KIND_RESULT) {
        return 1;
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    if obj.tag == 1 { 0 } else { 1 }
}

/// Takes the `Ok` payload and consumes the result. The payload is handed out
/// exactly once -- this is the single-owner handoff point -- so a stale
/// duplicate of the result word can never free the payload a second time.
#[unsafe(no_mangle)]
pub extern "C" fn olive_result_unwrap(r: i64) -> i64 {
    if r == 0 {
        abort_unwrap("unwrap called on null result");
    }
    if !crate::is_kind(r, KIND_RESULT) {
        abort_unwrap("unwrap called on invalid result");
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    let payload = obj.payload;
    if obj.tag != 1 {
        if payload == 0 {
            abort_unwrap("unwrap called on Err result");
        } else {
            let s = olive_str_from_ptr(payload);
            abort_unwrap(&format!("unwrap called on Err: {s}"));
        }
    }
    crate::panic::olive_set_fault_loc(0);
    free_slot(r);
    payload
}

/// Takes the `Err` payload and consumes the result, mirroring `unwrap`'s
/// single-owner handoff for the failure side.
#[unsafe(no_mangle)]
pub extern "C" fn olive_result_unwrap_err(r: i64) -> i64 {
    if r == 0 {
        abort_unwrap("unwrap_err called on null result");
    }
    if !crate::is_kind(r, KIND_RESULT) {
        abort_unwrap("unwrap_err called on invalid result");
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    let payload = obj.payload;
    if obj.tag == 1 {
        abort_unwrap("unwrap_err called on Ok result");
    }
    crate::panic::olive_set_fault_loc(0);
    free_slot(r);
    payload
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_unwrap_or(r: i64, default: i64) -> i64 {
    if !crate::is_kind(r, KIND_RESULT) {
        return default;
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    if obj.tag == 1 {
        let out = obj.payload;
        free_slot(r);
        if default != out {
            crate::olive_free_any(default);
        }
        out
    } else {
        olive_free_result(r);
        default
    }
}

/// Takes the `Err` message string and consumes the result. An unused `Ok`
/// payload is released with its result.
#[unsafe(no_mangle)]
pub extern "C" fn olive_result_err_msg(r: i64) -> i64 {
    if !crate::is_kind(r, KIND_RESULT) {
        return olive_str_internal("");
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    if obj.tag == 0 {
        let out = obj.payload;
        free_slot(r);
        out
    } else {
        olive_free_result(r);
        0
    }
}

/// Releases the result's slot without freeing its payload -- the consuming
/// accessor has already handed the payload to a new owner. A stale duplicate
/// of the same result word lands here as an absorbed double-free.
fn free_slot(ptr: i64) {
    match crate::slab::slab_membership(ptr) {
        None => {}
        Some(true) => crate::slab::with_escape_arena(|| free_result_slot_local(ptr)),
        Some(false) => free_result_slot_local(ptr),
    }
}

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;

    fn s(text: &str) -> i64 {
        olive_str_internal(text)
    }

    fn from_ptr(ptr: i64) -> String {
        crate::olive_str_from_ptr(ptr)
    }

    #[test]
    fn result_ok_is_ok() {
        let r = olive_result_ok(42);
        assert_eq!(olive_result_is_ok(r), 1);
        assert_eq!(olive_result_is_err(r), 0);
        assert_eq!(olive_result_unwrap(r), 42);
    }

    #[test]
    fn result_err_is_err() {
        let r = olive_result_err(s("something went wrong"));
        assert_eq!(olive_result_is_ok(r), 0);
        assert_eq!(olive_result_is_err(r), 1);
        let msg = from_ptr(olive_result_unwrap_err(r));
        assert_eq!(msg, "something went wrong");
    }

    #[test]
    fn result_unwrap_or() {
        let ok = olive_result_ok(99);
        let err = olive_result_err(s("fail"));
        assert_eq!(olive_result_unwrap_or(ok, 0), 99);
        assert_eq!(olive_result_unwrap_or(err, 0), 0);
        assert_eq!(olive_result_unwrap_or(0, 7), 7);
    }

    #[test]
    fn result_err_msg() {
        let r = olive_result_err(s("oops"));
        assert_eq!(from_ptr(olive_result_err_msg(r)), "oops");
    }

    #[test]
    fn result_rejects_raw_struct_with_matching_header() {
        let raw = crate::struct_obj::olive_struct_alloc(9);
        unsafe { *((raw + 8) as *mut i64) = 123 };
        assert_eq!(olive_result_is_ok(raw), 0);
        assert_eq!(olive_result_is_err(raw), 1);
        assert_eq!(olive_result_unwrap_or(raw, 7), 7);
        olive_free_result(raw);
        crate::struct_obj::olive_free_struct(raw);
    }

    #[test]
    fn result_cycle_frees_without_recursing_into_live_outer_slot() {
        let result = olive_result_ok(0);
        unsafe { (*(result as *mut OliveResult)).payload = result };
        olive_free_result(result);
        assert!(!crate::slab::slot_is_live(result));
    }
}
