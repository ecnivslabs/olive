use crate::slab::GenSlab;
use crate::{olive_str_to_bytes, string_slab};
use std::cell::UnsafeCell;

pub(crate) const KIND_STRBUF: i64 = 20;

thread_local! {
    static STRBUF_SLAB: UnsafeCell<GenSlab> = const {
        UnsafeCell::new(GenSlab::with_cleanup(
            std::mem::size_of::<OliveStrBuf>(),
            release_strbuf_storage,
        ))
    };
}

fn with_strbuf_slab<T>(f: impl FnOnce(&mut GenSlab) -> T) -> T {
    let active = crate::slab::ACTIVE_SLABS.get();
    if active.is_null() {
        STRBUF_SLAB.with(|sl| f(unsafe { &mut *sl.get() }))
    } else {
        f(unsafe { &mut (*active).strbuf })
    }
}

pub(crate) fn owns_strbuf(v: i64) -> bool {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            if (*active).strbuf.owns_addr(v as usize) {
                return true;
            }
            let source = crate::slab::SOURCE_SLABS.get();
            if !source.is_null() && (*source).strbuf.owns_addr(v as usize) {
                return true;
            }
            if crate::slab::active_slab_is_global() {
                return STRBUF_SLAB.with(|sl| (*sl.get()).owns_addr(v as usize));
            }
            return crate::slab::global_strbuf_owns_addr(v as usize);
        }
        STRBUF_SLAB.with(|sl| (*sl.get()).owns_addr(v as usize))
            || crate::slab::global_strbuf_owns_addr(v as usize)
    }
}

pub(crate) unsafe fn release_strbuf_storage(body: *mut u8) {
    unsafe {
        let sb = &mut *(body as *mut OliveStrBuf);
        drop(std::mem::take(&mut sb.buf));
        sb.char_count = 0;
    }
}

#[repr(C)]
pub struct OliveStrBuf {
    pub kind: i64,
    pub buf: Vec<u8>,
    pub char_count: i64,
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_strbuf_new(cap: i64) -> i64 {
    let initial_cap = if cap > 0 { cap as usize } else { 0 };
    let sb = OliveStrBuf {
        kind: KIND_STRBUF,
        buf: Vec::with_capacity(initial_cap),
        char_count: 0,
    };
    with_strbuf_slab(|sl| {
        let (body, _) = sl.alloc();
        unsafe {
            std::ptr::write(body as *mut OliveStrBuf, sb);
        }
        body as i64
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_strbuf_push(buf_ptr: i64, s: i64) -> i64 {
    if buf_ptr == 0 {
        return 0;
    }
    if !crate::slab::slot_is_live(buf_ptr) || !owns_strbuf(buf_ptr) {
        return 0;
    }
    let sb = unsafe { &mut *(buf_ptr as *mut OliveStrBuf) };
    if sb.kind != KIND_STRBUF {
        return 0;
    }
    let s_bytes = olive_str_to_bytes(s);
    if !s_bytes.is_empty() {
        let chars = s_bytes.iter().filter(|&&b| (b & 0xC0) != 0x80).count() as i64;
        sb.char_count += chars;
        sb.buf.extend_from_slice(s_bytes);
    }
    buf_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_strbuf_len(buf_ptr: i64) -> i64 {
    if buf_ptr == 0 {
        return 0;
    }
    if !crate::slab::slot_is_live(buf_ptr) || !owns_strbuf(buf_ptr) {
        return 0;
    }
    let sb = unsafe { &*(buf_ptr as *const OliveStrBuf) };
    if sb.kind != KIND_STRBUF {
        return 0;
    }
    sb.buf.len() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_strbuf_char_len(buf_ptr: i64) -> i64 {
    if buf_ptr == 0 {
        return 0;
    }
    if !crate::slab::slot_is_live(buf_ptr) || !owns_strbuf(buf_ptr) {
        return 0;
    }
    let sb = unsafe { &*(buf_ptr as *const OliveStrBuf) };
    if sb.kind != KIND_STRBUF {
        return 0;
    }
    sb.char_count
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_strbuf_build(buf_ptr: i64) -> i64 {
    if buf_ptr == 0 {
        return string_slab::str_alloc(b"");
    }
    if !crate::slab::slot_is_live(buf_ptr) || !owns_strbuf(buf_ptr) {
        return string_slab::str_alloc(b"");
    }
    let sb = unsafe { &*(buf_ptr as *const OliveStrBuf) };
    if sb.kind != KIND_STRBUF {
        return string_slab::str_alloc(b"");
    }
    string_slab::str_alloc(&sb.buf)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_strbuf_clear(buf_ptr: i64) {
    if buf_ptr == 0 {
        return;
    }
    if !crate::slab::slot_is_live(buf_ptr) || !owns_strbuf(buf_ptr) {
        return;
    }
    let sb = unsafe { &mut *(buf_ptr as *mut OliveStrBuf) };
    if sb.kind != KIND_STRBUF {
        return;
    }
    sb.buf.clear();
    sb.char_count = 0;
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_strbuf_free(buf_ptr: i64) {
    if buf_ptr == 0 {
        return;
    }
    let Some(is_global) = crate::slab::slab_membership(buf_ptr) else {
        return;
    };
    // The buffer detach plus the slot free run under the single arena lock
    // on the global path; the detached buffer drops after the lock is out.
    let detached = if is_global && !crate::slab::in_escape_arena() {
        crate::slab::with_escape_arena(|| free_strbuf_inner(buf_ptr))
    } else {
        free_strbuf_inner(buf_ptr)
    };
    drop(detached);
}

/// Triple-checked body: ownership then liveness, both before the kind word
/// is read. Detaches the buffer and frees the slot, handing the buffer out
/// for the caller to drop outside any arena lock.
fn free_strbuf_inner(buf_ptr: i64) -> Option<Vec<u8>> {
    if !crate::slab::check_owned_live(buf_ptr, owns_strbuf) {
        return None;
    }
    if unsafe { *(buf_ptr as *const i64) } != KIND_STRBUF {
        return None;
    }
    let taken = unsafe {
        let sb = &mut *(buf_ptr as *mut OliveStrBuf);
        let taken = std::mem::take(&mut sb.buf);
        sb.char_count = 0;
        taken
    };
    with_strbuf_slab(|sl| {
        sl.free(buf_ptr as *mut u8);
    });
    Some(taken)
}
