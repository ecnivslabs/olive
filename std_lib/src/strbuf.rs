use crate::slab::GenSlab;
use crate::{olive_str_to_bytes, string_slab};
use std::cell::UnsafeCell;

pub(crate) const KIND_STRBUF: i64 = 20;

thread_local! {
    static STRBUF_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::new(std::mem::size_of::<OliveStrBuf>())) };
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
    STRBUF_SLAB.with(|sl| {
        let sl = unsafe { &mut *sl.get() };
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
    let sb = unsafe { &mut *(buf_ptr as *mut OliveStrBuf) };
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
    let sb = unsafe { &*(buf_ptr as *const OliveStrBuf) };
    sb.buf.len() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_strbuf_char_len(buf_ptr: i64) -> i64 {
    if buf_ptr == 0 {
        return 0;
    }
    let sb = unsafe { &*(buf_ptr as *const OliveStrBuf) };
    sb.char_count
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_strbuf_build(buf_ptr: i64) -> i64 {
    if buf_ptr == 0 {
        return string_slab::str_alloc(b"");
    }
    let sb = unsafe { &*(buf_ptr as *const OliveStrBuf) };
    string_slab::str_alloc(&sb.buf)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_strbuf_clear(buf_ptr: i64) {
    if buf_ptr == 0 {
        return;
    }
    let sb = unsafe { &mut *(buf_ptr as *mut OliveStrBuf) };
    sb.buf.clear();
    sb.char_count = 0;
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_strbuf_free(buf_ptr: i64) {
    if buf_ptr == 0 {
        return;
    }
    if crate::slab::slot_is_live(buf_ptr) {
        unsafe {
            let sb = &mut *(buf_ptr as *mut OliveStrBuf);
            drop(std::mem::take(&mut sb.buf));
            sb.char_count = 0;
        }
    }
    STRBUF_SLAB.with(|sl| {
        unsafe { &mut *sl.get() }.free(buf_ptr as *mut u8);
    });
}
