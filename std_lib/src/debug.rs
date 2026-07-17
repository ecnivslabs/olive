//! C ABI helpers for the debugger's lazy variable expansion (`pit dap` /
//! `pit debug`). Read-only, called only while the debuggee thread is parked
//! on a stop, so no locking is needed beyond what the underlying containers
//! already use. Element/count access mirrors `format.rs`'s decoders but
//! returns raw words instead of rendered text, since the caller (pit) walks
//! its own copy of the static `Type` to know what each word means.

use crate::{OliveEnum, OliveObj, StableVec};

/// Element count of a list or set. Not used for tuples: a tuple's fixed
/// arity comes from its static type, not the runtime value.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_seq_len(val: i64) -> i64 {
    if val == 0 {
        return 0;
    }
    unsafe { (*(val as *const StableVec)).len as i64 }
}

/// Element at `idx` of a list, set, or tuple; all three share the
/// `StableVec` layout. `0` on a null receiver or an out-of-range index.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_seq_get(val: i64, idx: i64) -> i64 {
    if val == 0 || idx < 0 {
        return 0;
    }
    unsafe {
        let v = &*(val as *const StableVec);
        if idx as usize >= v.len {
            return 0;
        }
        *v.ptr.add(idx as usize)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_dict_len(val: i64) -> i64 {
    if val == 0 {
        return 0;
    }
    unsafe { (*(val as *const OliveObj)).fields.len() as i64 }
}

/// Key word at position `idx` in the dict's iteration order. Paired calls to
/// `dict_key`/`dict_val` at the same `idx` on an unmodified map see the same
/// entry: an olive dict is only ever read while its owning frame is parked,
/// so nothing mutates it between the two calls.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_dict_key(val: i64, idx: i64) -> i64 {
    if val == 0 || idx < 0 {
        return 0;
    }
    unsafe {
        (*(val as *const OliveObj))
            .fields
            .keys()
            .nth(idx as usize)
            .map(|k| k.0)
            .unwrap_or(0)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_dict_val(val: i64, idx: i64) -> i64 {
    if val == 0 || idx < 0 {
        return 0;
    }
    unsafe {
        (*(val as *const OliveObj))
            .fields
            .values()
            .nth(idx as usize)
            .copied()
            .unwrap_or(0)
    }
}

/// Active variant index. Caller must not pass a null receiver: an enum-typed
/// cell holding `None` has no tag to read, and is handled before this call.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_enum_tag(val: i64) -> i64 {
    unsafe { (*(val as *const OliveEnum)).tag }
}

/// Payload word at position `idx` of the active variant. `0` on a null
/// receiver or an index past this variant's payload arity.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_enum_payload(val: i64, idx: i64) -> i64 {
    if val == 0 || idx < 0 {
        return 0;
    }
    unsafe {
        let e = &*(val as *const OliveEnum);
        if idx as usize >= e.payload_len {
            return 0;
        }
        *e.payload_ptr.add(idx as usize)
    }
}
