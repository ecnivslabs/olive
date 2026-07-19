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

/// Boxes an owned struct pointer with its `D_STRUCT` descriptor. The box
/// takes ownership; freeing it deep-frees the struct through the descriptor.
/// Descriptor constants arrive as tagged string words; the tag bit is
/// stripped so the stored pointer reads as raw bytes.
#[unsafe(no_mangle)]
pub extern "C" fn olive_struct_box(ptr: i64, desc: i64) -> i64 {
    let desc = desc & !1;
    STRUCT_BOX_SLAB.with(|sl| {
        let (body, _) = unsafe { &mut *sl.get() }.alloc();
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
    let (desc, inner) = {
        let b = unsafe { &*(val as *const OliveStructBox) };
        (b.desc, b.ptr)
    };
    STRUCT_BOX_SLAB.with(|sl| unsafe { &mut *sl.get() }.free(val as *mut u8));
    crate::free_typed::olive_free_typed(inner, desc);
}

/// Allocates a box shell for the deep-copy walk; the inner pointer is patched
/// after the copy so cycles can resolve to the shell.
pub(crate) fn alloc_shell(desc: i64) -> i64 {
    olive_struct_box(0, desc)
}

pub(crate) fn set_inner(shell: i64, inner: i64) {
    unsafe { (*(shell as *mut OliveStructBox)).ptr = inner };
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
