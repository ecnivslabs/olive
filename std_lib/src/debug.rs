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
use std::sync::atomic::{AtomicI64, Ordering};

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
///
/// Every construct below runs inside `with_escape_arena`, and that is not an
/// optimization: these calls arrive on the *controller* thread (the DAP
/// request loop), while the value they build is handed to the *debuggee*
/// thread, which reads it and later frees it from its own arena context. A
/// slot in the controller thread's own pool is invisible to both: the
/// debuggee's ownership validation rejects it as a foreign word, and no
/// debuggee-side free can reclaim it. The process-lifetime arena is the one
/// place both threads can read, validate, and free the same slot, which is
/// what `olive_relocate_typed` uses for values crossing a task boundary for
/// exactly the same reason.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_str_new(bytes: *const u8, len: i64) -> i64 {
    if bytes.is_null() || len < 0 {
        crate::slab::with_escape_arena(|| crate::string::olive_str_internal(""))
    } else {
        let slice = unsafe { std::slice::from_raw_parts(bytes, len as usize) };
        let text = String::from_utf8_lossy(slice).into_owned();
        crate::slab::with_escape_arena(|| crate::string::olive_str_internal(&text))
    }
}

/// Arena-resident dict for a debugger-built `{...}` literal.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_obj_new() -> i64 {
    crate::slab::with_escape_arena(|| crate::obj::olive_obj_new())
}

/// Arena-resident typed dict insert: the key copy a string key needs is
/// allocated by this call, so it belongs in the arena beside its dict.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_obj_set_owned_typed(
    obj_ptr: i64,
    attr: i64,
    val: i64,
    key_desc: i64,
    val_desc: i64,
) -> i64 {
    crate::slab::with_escape_arena(|| {
        crate::obj::olive_obj_set_owned_typed(obj_ptr, attr, val, key_desc, val_desc)
    })
}

/// Arena-resident list, tuple, or vector for a debugger-built literal.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_list_new(capacity: i64) -> i64 {
    crate::slab::with_escape_arena(|| crate::list::olive_list_new(capacity))
}

/// Arena-resident set for a debugger-built `{...}` set literal.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_set_new(capacity: i64) -> i64 {
    crate::slab::with_escape_arena(|| crate::set::olive_set_new(capacity))
}

/// Arena-resident typed set insert: `add` can copy the element and owns the
/// word either way, so the copy and the possible release both need the arena.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_set_add_typed(set_ptr: i64, val: i64, key_desc: i64) {
    crate::slab::with_escape_arena(|| {
        crate::hash_typed::olive_set_add_typed(set_ptr, val, key_desc)
    })
}

/// Arena-resident struct record for a debugger-built `Name(...)` constructor.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_struct_alloc(n_fields: i64) -> i64 {
    crate::slab::with_escape_arena(|| crate::olive_struct_alloc(n_fields))
}

/// Arena-resident enum record for a debugger-built `Name.Variant(...)`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_enum_new(type_id: i64, tag: i64, arg_count: i64, desc: i64) -> i64 {
    crate::slab::with_escape_arena(|| crate::olive_enum_new(type_id, tag, arg_count, desc))
}

/// Decodes a tag-encoded union word for the debugger: writes the payload
/// through `out` and returns the kind (0 = None, 1 = int, 2 = bool,
/// 3 = float where `out` holds the f64 bit pattern, 4 = str where `out`
/// holds the tagged str word itself, 5 = opaque heap member such as a boxed
/// struct or container where `out` holds the word). Layout knowledge stays
/// here, next to boxed.rs, instead of leaking into pit.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_any_decode(val: i64, out: *mut i64) -> i64 {
    let (kind, payload) = match val & crate::boxed::TAG_MASK {
        crate::boxed::TAG_INT => (1, val >> 3),
        crate::boxed::TAG_BOOL => (2, val >> 3),
        crate::boxed::TAG_NULL => (0, 0),
        _ if crate::string::is_live_str_word(val) => (4, val),
        _ if !crate::is_active_object(val) => (5, val),
        _ => {
            let kind = unsafe { *(val as *const i64) };
            match kind {
                crate::KIND_FLOAT if crate::is_kind(val, kind) => (3, unsafe {
                    (*(val as *const crate::boxed::OliveBoxed)).bits
                }),
                crate::KIND_INT | crate::KIND_U64 if crate::is_kind(val, kind) => (1, unsafe {
                    (*(val as *const crate::boxed::OliveBoxed)).bits
                }),
                _ => (5, val),
            }
        }
    };
    unsafe { *out = payload };
    kind
}

/// Encodes a scalar payload into a tag-encoded scalar-union word, the
/// inverse of `olive_debug_any_decode` for `setVariable` writes. `kind`
/// follows the same numbering; for kind 3 `payload` is the f64 bit pattern.
///
/// Arena-resident for the same reason as `olive_debug_str_new`: a written
/// union word lives on in the debuggee's frame, which a slot in the
/// controller thread's pool cannot serve.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_any_encode(kind: i64, payload: i64) -> i64 {
    crate::slab::with_escape_arena(|| match kind {
        0 => crate::boxed::olive_box_null(),
        2 => crate::boxed::olive_box_bool(payload),
        3 => crate::boxed::olive_box_float(f64::from_bits(payload as u64)),
        _ => crate::boxed::olive_box_int(payload),
    })
}

/// Raw `extern "C" fn(name_ptr: i64)` / `extern "C" fn()` pair the debugger
/// installs at `pit debug`/`pit dap` launch (`olive_debug_set_thread_hooks`)
/// so a thread spawned through `spawn_traced` can register itself with the
/// active session. Zero (unset) outside a debug session, so `spawn_traced`
/// costs exactly the two relaxed loads below either way.
static THREAD_START_HOOK: AtomicI64 = AtomicI64::new(0);
static THREAD_END_HOOK: AtomicI64 = AtomicI64::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_set_thread_hooks(start: i64, end: i64) {
    THREAD_START_HOOK.store(start, Ordering::Relaxed);
    THREAD_END_HOOK.store(end, Ordering::Relaxed);
}

/// Spawns `f` on a named OS thread, registering it with an active debug
/// session (if any) so breakpoints/stepping/inspection reach code running on
/// it. `aio`'s executor pool, spawned tasks, and `pool_run`(`_sync`) all
/// execute real olive-compiled code on their own thread and are otherwise
/// invisible to the debugger the way the main thread never is. A plain
/// `std::thread::spawn` outside a debug session either way: both hooks stay
/// zero, so this costs exactly the checks below and nothing more.
pub fn spawn_traced<F>(name: &'static str, f: F) -> std::thread::JoinHandle<()>
where
    F: FnOnce() + Send + 'static,
{
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let start = THREAD_START_HOOK.load(Ordering::Relaxed);
            if start != 0 {
                let cname = std::ffi::CString::new(name).unwrap_or_default();
                let hook: extern "C" fn(i64) = unsafe { std::mem::transmute(start as *const ()) };
                hook(cname.as_ptr() as i64);
            }
            f();
            let end = THREAD_END_HOOK.load(Ordering::Relaxed);
            if end != 0 {
                let hook: extern "C" fn() = unsafe { std::mem::transmute(end as *const ()) };
                hook();
            }
        })
        .expect("failed to spawn traced thread")
}
