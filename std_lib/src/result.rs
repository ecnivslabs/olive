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
    if ptr == 0 || !crate::slab::ptr_in_slab_span(ptr) {
        return;
    }
    if crate::slab::slot_is_live(ptr) {
        let payload = unsafe { (*(ptr as *const OliveResult)).payload };
        crate::olive_free_any(payload);
    }
    match crate::slab::slab_membership(ptr) {
        None => {}
        Some(true) => crate::slab::with_escape_arena(|| free_result_slot_local(ptr)),
        Some(false) => free_result_slot_local(ptr),
    }
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
    let obj = unsafe { &*(r as *const OliveResult) };
    obj.tag
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_is_err(r: i64) -> i64 {
    if r == 0 {
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
    if r == 0 {
        return default;
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    let out = if obj.tag == 1 { obj.payload } else { default };
    free_slot(r);
    out
}

/// Takes the `Err` message string and consumes the result. An `Ok` result
/// owns nothing on this branch (its payload belongs to whoever unwraps it),
/// so only the slot is released there.
#[unsafe(no_mangle)]
pub extern "C" fn olive_result_err_msg(r: i64) -> i64 {
    if r == 0 {
        return olive_str_internal("");
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    let out = if obj.tag == 0 { obj.payload } else { 0 };
    free_slot(r);
    out
}

/// Releases the result's slot without freeing its payload -- the consuming
/// accessor has already handed the payload to a new owner. A stale duplicate
/// of the same result word lands here as an absorbed double-free.
fn free_slot(ptr: i64) {
    RESULT_SLAB.with(|sl| unsafe { &mut *sl.get() }.free(ptr as *mut u8));
}

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
    fn result_ok_err_msg_zero() {
        let r = olive_result_ok(1);
        assert_eq!(olive_result_err_msg(r), 0);
    }
}
