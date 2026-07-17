//! C ABI helpers for the debugger's lazy variable expansion and `setVariable`
//! writes (`pit dap` / `pit debug`). Called only while the debuggee thread is
//! parked on a stop, so no locking is needed beyond what the underlying
//! containers already use. Element/count access mirrors `format.rs`'s
//! decoders but returns raw words instead of rendered text, since the caller
//! (pit) walks its own copy of the static `Type` to know what each word
//! means. The setters never free the value they overwrite: pit has no
//! move/borrow analysis of its own, so freeing an old element it doesn't
//! know the ownership of risks a double free. A debugger-initiated string
//! replace leaks the old string instead -- bounded, rare, and safe.

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

/// Raw UTF-8 bytes of a string value: pointer returned, length written
/// through `out_len`. Delegates to the same decoder every runtime string op
/// uses, so slab vs. literal layout is never duplicated on the pit side.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_str_bytes(val: i64, out_len: *mut i64) -> i64 {
    let bytes = crate::string::olive_str_to_bytes(val);
    unsafe { *out_len = bytes.len() as i64 };
    bytes.as_ptr() as i64
}

/// Overwrites element `idx` of a list, set, or tuple (all three share the
/// `StableVec` layout). `0`/no-op on a null receiver or an out-of-range
/// index; `1` on success.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_seq_set(val: i64, idx: i64, new: i64) -> i64 {
    if val == 0 || idx < 0 {
        return 0;
    }
    unsafe {
        let v = &mut *(val as *mut StableVec);
        if idx as usize >= v.len {
            return 0;
        }
        *v.ptr.add(idx as usize) = new;
    }
    1
}

/// Overwrites the value half of the dict entry at position `idx` in
/// iteration order -- the same position `olive_debug_dict_val` reads, valid
/// as long as nothing mutated the map in between (true for a debugger
/// write immediately following the read that produced `idx`). `0`/no-op on
/// a null receiver or an out-of-range index; `1` on success.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_dict_set(val: i64, idx: i64, new: i64) -> i64 {
    if val == 0 || idx < 0 {
        return 0;
    }
    unsafe {
        let obj = &mut *(val as *mut OliveObj);
        let Some(slot) = obj.fields.values_mut().nth(idx as usize) else {
            return 0;
        };
        *slot = new;
    }
    1
}

/// Overwrites payload word `idx` of the active variant. `0`/no-op on a null
/// receiver or an index past this variant's payload arity; `1` on success.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_enum_set(val: i64, idx: i64, new: i64) -> i64 {
    if val == 0 || idx < 0 {
        return 0;
    }
    unsafe {
        let e = &mut *(val as *mut OliveEnum);
        if idx as usize >= e.payload_len {
            return 0;
        }
        *e.payload_ptr.add(idx as usize) = new;
    }
    1
}

/// Allocates a fresh heap-backed olive string from raw UTF-8 bytes, for a
/// `setVariable`/`setExpression` write of a `str`-typed slot. Interior nul
/// bytes are stripped by `olive_str_internal`, same as any other string
/// construction path.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_str_new(bytes: *const u8, len: i64) -> i64 {
    if bytes.is_null() || len < 0 {
        return crate::string::olive_str_internal("");
    }
    let slice = unsafe { std::slice::from_raw_parts(bytes, len as usize) };
    crate::string::olive_str_internal(&String::from_utf8_lossy(slice))
}
