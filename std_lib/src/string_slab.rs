//! Size-classed generational slab for heap strings. A slot's body holds the
//! nul-terminated bytes inline, so a string pointer is the body address with
//! the low tag bit set. Literals live in read-only data and never enter a
//! slab, so the chunk classifier frees and marks them as no-ops.

use crate::slab::{GenSlab, ptr_in_slab_span};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, Ordering};

/// Byte capacity class for `need` content-plus-nul bytes, one machine word min.
#[inline]
fn class_bytes(need: usize) -> usize {
    need.next_power_of_two().max(8)
}

#[inline]
fn class_index(cap: usize) -> usize {
    cap.trailing_zeros() as usize
}

struct StrSlabs {
    classes: [Option<GenSlab>; 32],
}

impl StrSlabs {
    const fn new() -> Self {
        Self {
            classes: [
                None, None, None, None, None, None, None, None, None, None, None, None, None, None,
                None, None, None, None, None, None, None, None, None, None, None, None, None, None,
                None, None, None, None,
            ],
        }
    }

    #[inline]
    fn slab(&mut self, cap: usize) -> &mut GenSlab {
        let idx = class_index(cap);
        assert!(idx < 32, "olive: string size class limit exceeded");
        if self.classes[idx].is_none() {
            self.classes[idx] = Some(GenSlab::new(cap));
        }
        unsafe { self.classes[idx].as_mut().unwrap_unchecked() }
    }
}

thread_local! {
    static STR_SLABS: UnsafeCell<StrSlabs> = const { UnsafeCell::new(StrSlabs::new()) };
}

/// Allocates a heap string from `bytes`, which must not contain an interior
/// nul. Stores capacity class and length at body-16 for O(1) free. Returns the
/// body pointer tagged with the low string bit.
pub fn str_alloc(bytes: &[u8]) -> i64 {
    let len = bytes.len();
    let cap = class_bytes(len + 1);
    let slab_alloc = |slab: &mut GenSlab| {
        let (body, _) = slab.alloc();
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), body, len);
            *body.add(len) = 0;
            let cap_idx = cap.trailing_zeros() as usize;
            let header_val = len | (cap_idx << 48);
            *(body as *mut usize).sub(2) = header_val;
            body as i64 | 1
        }
    };
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            let idx = class_index(cap);
            assert!(idx < 32, "olive: string size class limit exceeded");
            if (*active).str_slabs[idx].is_none() {
                (*active).str_slabs[idx] = Some(GenSlab::new(cap));
            }
            slab_alloc((*active).str_slabs[idx].as_mut().unwrap_unchecked())
        } else {
            STR_SLABS.with(|s| {
                let s = &mut *s.get();
                slab_alloc(s.slab(cap))
            })
        }
    }
}

/// Frees a tagged string pointer. O(1) — capacity read from header at body-16.
/// A literal (not in any chunk) and an already free slot are no-ops.
pub fn str_free(ptr: i64) {
    let body = ptr & !1;
    if body == 0 || !ptr_in_slab_span(body) {
        return;
    }
    if crate::slab::chunk_is_global(body as usize) {
        crate::slab::with_escape_arena(|| str_free_local(ptr));
    } else {
        str_free_local(ptr);
    }
}

fn str_free_local(ptr: i64) {
    let body = ptr & !1;
    let header_val = unsafe { *(body as *const usize).sub(2) };
    let cap_idx = header_val >> 48;
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            if cap_idx < 32
                && let Some(ref mut slab) = (*active).str_slabs[cap_idx]
            {
                slab.free(body as *mut u8);
            }
        } else {
            STR_SLABS.with(|s| {
                let s = &mut *s.get();
                if cap_idx < 32
                    && let Some(ref mut slab) = s.classes[cap_idx]
                {
                    slab.free(body as *mut u8);
                }
            });
        }
    }
}

/// Optimizes concatenation in-place when the buffer fits. Capacity and length
/// read from header at body-16; no strlen. No sharing exists (all escapes deep-copy).
pub fn str_concat_inplace(l: i64, l_bytes: &[u8], r_bytes: &[u8]) -> Option<i64> {
    let body = l & !1;
    if body == 0 || !ptr_in_slab_span(body) {
        return None;
    }
    let l_len = l_bytes.len();
    let r_len = r_bytes.len();
    let new_len = l_len + r_len;
    let header_val = unsafe { *(body as *const usize).sub(2) };
    let cap_idx = header_val >> 48;
    let old_cap = 1usize << cap_idx;
    let new_cap = class_bytes(new_len + 1);
    if new_cap <= old_cap {
        unsafe {
            std::ptr::copy_nonoverlapping(r_bytes.as_ptr(), (body as *mut u8).add(l_len), r_len);
            *(body as *mut u8).add(new_len) = 0;
            let new_header = new_len | (cap_idx << 48);
            *(body as *mut usize).sub(2) = new_header;
        }
        Some(l)
    } else {
        STR_SLABS.with(|s| {
            let s = unsafe { &mut *s.get() };
            let (new_body, _) = s.slab(new_cap).alloc();
            unsafe {
                std::ptr::copy_nonoverlapping(l_bytes.as_ptr(), new_body, l_len);
                std::ptr::copy_nonoverlapping(r_bytes.as_ptr(), new_body.add(l_len), r_len);
                *new_body.add(new_len) = 0;
                let new_cap_idx = new_cap.trailing_zeros() as usize;
                let new_header = new_len | (new_cap_idx << 48);
                *(new_body as *mut usize).sub(2) = new_header;
            }
            if cap_idx < 32
                && let Some(ref mut slab) = s.classes[cap_idx]
            {
                slab.free(body as *mut u8);
            }
            Some(new_body as i64 | 1)
        })
    }
}

/// Captures the slab generation of a heap string for a later staleness check.
/// A literal or foreign pointer has no generation word, so it returns the zero
/// sentinel; `olive_str_gen_stale` reads that back as "never stale".
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_gen_of(ptr: i64) -> i64 {
    let body = ptr & !1;
    if body == 0 || !ptr_in_slab_span(body) {
        return 0;
    }
    unsafe { (*((body - 8) as *const AtomicU64)).load(Ordering::Relaxed) as i64 }
}

/// Whether a heap string borrow captured at generation `gen` is now stale. A
/// literal, foreign, null, or sentinel-`gen` borrow is never stale; a slab body
/// is stale once its generation moved (ignoring the shared bit) or its slot died.
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_gen_stale(ptr: i64, generation: i64) -> i64 {
    let body = ptr & !1;
    if body == 0 || generation == 0 || !ptr_in_slab_span(body) {
        return 0;
    }
    let cur = unsafe { (*((body - 8) as *const AtomicU64)).load(Ordering::Relaxed) as i64 };
    (((cur ^ generation) << 1) != 0 || cur & 1 == 0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_roundtrips_bytes() {
        let p = str_alloc(b"hello");
        assert_eq!(crate::string::olive_str_from_ptr(p), "hello");
        assert_eq!(p & 1, 1);
        str_free(p);
    }

    #[test]
    fn empty_string_allocates() {
        let p = str_alloc(b"");
        assert_eq!(crate::string::olive_str_from_ptr(p), "");
        str_free(p);
    }

    #[test]
    fn free_recycles_same_class() {
        let a = str_alloc(b"abcd");
        str_free(a);
        let b = str_alloc(b"wxyz");
        assert_eq!(a & !1, b & !1, "same class slot recycles");
        str_free(b);
    }

    #[test]
    fn double_free_absorbed() {
        let p = str_alloc(b"xy");
        str_free(p);
        str_free(p);
    }

    #[test]
    fn free_ignores_foreign_pointer() {
        let c = std::ffi::CString::new("literal").unwrap();
        let raw = c.into_raw();
        str_free(raw as i64 | 1);
        let _ = unsafe { std::ffi::CString::from_raw(raw) };
    }

    #[test]
    fn gen_of_literal_is_zero() {
        let c = std::ffi::CString::new("rodata").unwrap();
        let raw = c.into_raw();
        assert_eq!(olive_str_gen_of(raw as i64 | 1), 0);
        let _ = unsafe { std::ffi::CString::from_raw(raw) };
    }

    #[test]
    fn gen_of_heap_is_live() {
        let p = str_alloc(b"live");
        let g = olive_str_gen_of(p);
        assert_ne!(g, 0);
        assert_eq!(g & 1, 1);
        str_free(p);
    }

    #[test]
    fn stale_on_recycle() {
        let a = str_alloc(b"aaaa");
        let g = olive_str_gen_of(a);
        str_free(a);
        let b = str_alloc(b"bbbb");
        assert_eq!(a & !1, b & !1, "same slot recycles");
        assert_eq!(olive_str_gen_stale(a, g), 1);
        str_free(b);
    }

    #[test]
    fn stale_on_free_without_recycle() {
        let a = str_alloc(b"gone");
        let g = olive_str_gen_of(a);
        str_free(a);
        assert_eq!(olive_str_gen_stale(a, g), 1);
    }

    #[test]
    fn live_borrow_not_stale() {
        let a = str_alloc(b"held");
        let g = olive_str_gen_of(a);
        assert_eq!(olive_str_gen_stale(a, g), 0);
        str_free(a);
    }

    #[test]
    fn stale_literal_never_fires() {
        let c = std::ffi::CString::new("lit").unwrap();
        let raw = c.into_raw();
        assert_eq!(olive_str_gen_stale(raw as i64 | 1, 0), 0);
        let _ = unsafe { std::ffi::CString::from_raw(raw) };
    }

    #[test]
    fn large_string_class() {
        let big = vec![b'z'; 5000];
        let p = str_alloc(&big);
        assert_eq!(crate::string::olive_str_from_ptr(p).len(), 5000);
        str_free(p);
    }

    #[test]
    fn concat_inplace_capacity_fits() {
        let a = str_alloc(b"abc");
        let res = str_concat_inplace(a, b"abc", b"def").unwrap();
        assert_eq!(a, res, "must mutate in-place when capacity fits");
        assert_eq!(crate::string::olive_str_from_ptr(res).len(), 6);
        unsafe {
            let s = std::ffi::CStr::from_ptr((res & !1) as *const std::ffi::c_char).to_bytes();
            assert_eq!(s, b"abcdef");
        }
        str_free(res);
    }

    #[test]
    fn concat_inplace_capacity_grows() {
        let a = str_alloc(b"abc");
        let res = str_concat_inplace(a, b"abc", b"defghi").unwrap();
        assert_ne!(a, res, "must reallocate when new capacity exceeds old");
        assert_eq!(crate::string::olive_str_from_ptr(res).len(), 9);
        unsafe {
            let s = std::ffi::CStr::from_ptr((res & !1) as *const std::ffi::c_char).to_bytes();
            assert_eq!(s, b"abcdefghi");
        }
        str_free(res);
    }

    #[test]
    fn realloc_cross_class_header_tracking() {
        let a = str_alloc(b"a");
        let b = str_alloc(b"b");
        let big = vec![b'x'; 200];
        let p = str_alloc(&big);
        let p2 = str_alloc(b"tiny");
        str_free(a);
        str_free(b);
        str_free(p);
        str_free(p2);
    }

    #[test]
    fn huge_string_free() {
        let big = vec![b'H'; 1_000_000];
        let p = str_alloc(&big);
        assert_eq!(crate::string::olive_str_from_ptr(p).len(), 1_000_000);
        str_free(p);
    }

    #[test]
    fn header_word_tracks_after_realloc() {
        let a = str_alloc(b"hello");
        let res = str_concat_inplace(a, b"hello", b" world this is a long tail").unwrap();
        assert_ne!(a, res, "must realloc across class boundary");
        assert_eq!(
            crate::string::olive_str_from_ptr(res),
            "hello world this is a long tail"
        );
        str_free(res);
    }
}
