//! Struct erasure into `Any` and tag-encoded union slots. A user struct's
//! header word is its field count, not a kind tag, so a raw struct pointer is
//! ambiguous once its static type is erased: a 1-field struct reads as
//! `KIND_LIST`, a 2-field one as `KIND_OBJ`. Erasure wraps the pointer in a
//! slab box carrying a real kind and the struct's type descriptor, mirroring
//! how floats box on entry to `Any`. Concrete struct code never pays for it.

use crate::slab::GenSlab;
use std::cell::UnsafeCell;

pub(crate) const KIND_STRUCT_BOX: i64 = 16;

#[repr(C)]
pub struct OliveStructBox {
    pub kind: i64,
    pub desc: i64,
    pub ptr: i64,
}

thread_local! {
    static STRUCT_BOX_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::new(std::mem::size_of::<OliveStructBox>())) };
}

fn with_struct_box_slab<T>(f: impl FnOnce(&mut GenSlab) -> T) -> T {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            f(&mut (*active).struct_box)
        } else {
            STRUCT_BOX_SLAB.with(|sl| f(&mut *sl.get()))
        }
    }
}

/// Boxes an owned struct pointer with its `D_STRUCT` descriptor. The box
/// takes ownership; freeing it deep-frees the struct through the descriptor.
/// Descriptor constants arrive as tagged string words; the tag bit is
/// stripped so the stored pointer reads as raw bytes.
#[unsafe(no_mangle)]
pub extern "C" fn olive_struct_box(ptr: i64, desc: i64) -> i64 {
    let desc = crate::string_slab::str_body(desc);
    with_struct_box_slab(|sl| {
        let (body, _) = sl.alloc();
        unsafe {
            std::ptr::write(
                body as *mut OliveStructBox,
                OliveStructBox {
                    kind: KIND_STRUCT_BOX,
                    desc,
                    ptr,
                },
            );
        }
        body as i64
    })
}

/// Frees a box whose kind was already verified by the caller. The box slot is
/// released before the inner struct is walked so a data cycle terminates at
/// the generation guard.
pub(crate) fn free_struct_box(val: i64) {
    if !crate::slab::slot_is_live(val) {
        return;
    }
    let (desc, inner) = {
        let b = unsafe { &*(val as *const OliveStructBox) };
        (b.desc, b.ptr)
    };
    match crate::slab::slab_membership(val) {
        Some(true) => crate::slab::with_escape_arena(|| free_struct_box_local(val)),
        _ => free_struct_box_local(val),
    }
    crate::free_typed::olive_free_typed(inner, desc);
}

fn free_struct_box_local(val: i64) {
    with_struct_box_slab(|sl| sl.free(val as *mut u8));
}

/// Releases just the box shell, leaving the inner struct alone. The
/// generation check inside `slab::free` absorbs a stale double free.
fn free_struct_box_shell(val: i64) {
    match crate::slab::slab_membership(val) {
        Some(true) => crate::slab::with_escape_arena(|| free_struct_box_local(val)),
        _ => free_struct_box_local(val),
    }
}

/// Allocates a box shell for the deep-copy walk; the inner pointer is patched
/// after the copy so cycles can resolve to the shell.
pub(crate) fn alloc_shell(desc: i64) -> i64 {
    olive_struct_box(0, desc)
}

pub(crate) fn set_inner(shell: i64, inner: i64) {
    unsafe { (*(shell as *mut OliveStructBox)).ptr = inner };
}

/// Narrowing a tag-encoded union back to its struct member: peel the box off
/// and hand back the raw struct pointer, the same non-consuming peek
/// `olive_unbox_int`/`olive_unbox_float` do for their own member types. The
/// box itself is left alone; it drops normally through the union local that
/// still owns it.
///
/// The peel is verified: narrowing lets a union flow into struct-typed code
/// on the promise that only the sentinel inhabits the other member, so a
/// non-sentinel value (or any other member) reaching here is a violated
/// assumption, not a struct box. Dereferencing it as `OliveStructBox` would
/// be a misaligned-pointer trap; it faults cleanly (E0715) instead.
#[unsafe(no_mangle)]
pub extern "C" fn olive_struct_unbox(val: i64) -> i64 {
    if val == 0 {
        return 0;
    }
    peel_struct_box(val)
}

/// Consuming unbox: releases the box shell and hands ownership of the inner
/// struct to the caller. Used when narrowing transfers the value onward (the
/// `try` success path) rather than borrowing it in place; the union local's
/// later generation-guarded drop then sees a dead slot and does nothing.
/// Verified like the peeking unbox above: a non-box faults before the shell
/// is touched, so a violated assumption can neither free nor hand out.
#[unsafe(no_mangle)]
pub extern "C" fn olive_struct_unbox_take(val: i64) -> i64 {
    if val == 0 {
        return 0;
    }
    let inner = peel_struct_box(val);
    free_struct_box_shell(val);
    inner
}

/// Verifies `val` is a live struct box and returns its inner struct pointer,
/// faulting (E0715) otherwise. Guards mirror `olive_any_is_struct_box` so
/// every union member classifies without a dereference; the kind word is
/// read only on a live slab slot, which keeps even wild words total.
fn peel_struct_box(val: i64) -> i64 {
    if crate::slab::slot_is_live(val) {
        let unboxed = unsafe { &*(val as *const OliveStructBox) };
        if unboxed.kind == KIND_STRUCT_BOX {
            return unboxed.ptr;
        }
    }
    crate::panic::abort_unbox(&format!(
        "narrowed union holds {} where a struct was expected",
        rejected_member_name(val)
    ))
}

/// Names the non-struct member a narrowing check just rejected, so the E0715
/// fault reads like the value it found instead of a bare address. Reads at
/// most the kind word of a live slab slot; everything else classifies by
/// tag and magnitude alone.
fn rejected_member_name(val: i64) -> &'static str {
    // Same magnitude heuristic the string classifier uses (`boxed::is_str`):
    // a bare bit-0 test would misread small odd words like -1 as strings.
    if val & 1 == 1 && (val & !1) > 0x10000 {
        return "a string";
    }
    match val & crate::boxed::TAG_MASK {
        crate::boxed::TAG_INT => return "an integer",
        crate::boxed::TAG_BOOL => return "a boolean",
        crate::boxed::TAG_NULL => return "null",
        _ => {}
    }
    if val < 0x1000 {
        return "a small integer";
    }
    if crate::slab::slot_is_live(val) {
        match unsafe { *(val as *const i64) } {
            crate::KIND_INT => return "an integer",
            crate::KIND_FLOAT => return "a float",
            _ => {}
        }
    } else if crate::slab::ptr_in_slab_span(val) {
        return "a freed value";
    }
    "a value of another type"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_roundtrip_and_kind() {
        let s = crate::olive_struct_alloc(2);
        unsafe {
            *((s + 8) as *mut i64) = 10;
            *((s + 16) as *mut i64) = 20;
        }
        // D_STRUCT "P" with two int fields "a", "b".
        let desc: Vec<u8> = vec![12, 14, b'P', 15, 14, b'a', 1, 14, b'b', 1, 0];
        let b = olive_struct_box(s, desc.as_ptr() as i64);
        assert_eq!(b & 7, 0);
        let bx = unsafe { &*(b as *const OliveStructBox) };
        assert_eq!(bx.kind, KIND_STRUCT_BOX);
        assert_eq!(bx.ptr, s);
        free_struct_box(b);
    }

    #[test]
    fn plain_struct_header_is_field_count() {
        let s = crate::olive_struct_alloc(3);
        assert_eq!(unsafe { *(s as *const i64) }, 3);
        crate::struct_obj::olive_free_struct(s);
    }
}
