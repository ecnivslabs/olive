//! Size-classed generational slab for heap strings. A slot's body holds the
//! nul-terminated bytes inline, so a string pointer is the body address with
//! the string and heap tag bits set. Literals live in read-only data, never
//! enter a slab, and carry only the string bit, so freeing one is a no-op.

use crate::slab::GenSlab;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, Ordering};

/// Marks a word as an Olive string pointer rather than a raw scalar.
pub const STR_TAG: i64 = 1;

/// Marks a string pointer as slab-allocated, so it carries a header at
/// body-16 and frees through a slab. Stamped once by the allocator, which
/// already knows the answer, so the hot paths read a bit instead of running
/// a chunk-table lookup to rediscover it. Literals and foreign pointers keep
/// this clear: codegen aligns literal data to 4 and the interned char table
/// strides by 4, so no untagged string address can set it by accident.
pub const STR_HEAP: i64 = 2;

/// Strips the tag bits, yielding the address of the string bytes.
#[inline]
pub fn str_body(ptr: i64) -> i64 {
    ptr & !(STR_TAG | STR_HEAP)
}

/// Whether `ptr` is slab-allocated (has a header, frees through a slab).
#[inline]
pub fn str_is_heap(ptr: i64) -> bool {
    ptr & STR_HEAP != 0
}

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

/// Runs `f` against the `cap`-byte class slab for the current escape-arena
/// context: the shared global arena when a value is crossing a task/thread
/// boundary, this thread's own pool otherwise. Every heap-string allocation
/// site must go through this so a body's slab always matches its later free.
fn with_class_slab<T>(cap: usize, f: impl FnOnce(&mut GenSlab) -> T) -> T {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            let idx = class_index(cap);
            assert!(idx < 32, "olive: string size class limit exceeded");
            if (*active).str_slabs[idx].is_none() {
                (*active).str_slabs[idx] = Some(GenSlab::new(cap));
            }
            f((*active).str_slabs[idx].as_mut().unwrap_unchecked())
        } else {
            STR_SLABS.with(|s| f((&mut *s.get()).slab(cap)))
        }
    }
}

/// Allocates a heap string from `bytes`, which must not contain an interior
/// nul. Stores capacity class and length at body-16 for O(1) free. Returns the
/// body pointer tagged with the low string bit.
pub fn str_alloc(bytes: &[u8]) -> i64 {
    let len = bytes.len();
    let cap = class_bytes(len + 1);
    with_class_slab(cap, |slab| {
        let (body, _) = slab.alloc();
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), body, len);
            *body.add(len) = 0;
            let cap_idx = cap.trailing_zeros() as usize;
            let header_val = len | (cap_idx << 48);
            *(body as *mut usize).sub(2) = header_val;
            body as i64 | STR_TAG | STR_HEAP
        }
    })
}

/// Same as `str_alloc`, but writes two slices back to back without first
/// concatenating them into an intermediate buffer.
fn str_alloc_two(a: &[u8], b: &[u8]) -> i64 {
    let len = a.len() + b.len();
    let cap = class_bytes(len + 1);
    with_class_slab(cap, |slab| {
        let (body, _) = slab.alloc();
        unsafe {
            std::ptr::copy_nonoverlapping(a.as_ptr(), body, a.len());
            std::ptr::copy_nonoverlapping(b.as_ptr(), body.add(a.len()), b.len());
            *body.add(len) = 0;
            let cap_idx = cap.trailing_zeros() as usize;
            let header_val = len | (cap_idx << 48);
            *(body as *mut usize).sub(2) = header_val;
            body as i64 | STR_TAG | STR_HEAP
        }
    })
}

/// Frees a tagged string pointer. O(1) — capacity read from header at body-16.
/// A literal (not in any chunk) and an already free slot are no-ops.
pub fn str_free(ptr: i64) {
    let body = str_body(ptr);
    if body == 0 || !str_is_heap(ptr) {
        return;
    }
    // Which arena the body lives in is still a lookup: only the rare global
    // case needs it, and freeing is not on the hot path the tag bit targets.
    match crate::slab::slab_membership(body) {
        None => {}
        Some(true) => crate::slab::with_escape_arena(|| str_free_local(ptr)),
        Some(false) => str_free_local(ptr),
    }
}

fn str_free_local(ptr: i64) {
    let body = str_body(ptr);
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
    str_concat_inplace_with(l, l_bytes, r_bytes, None)
}

/// Same as `str_concat_inplace`, but takes the caller's already-known
/// heap-vs-literal answer for `l` when it has one.
pub fn str_concat_inplace_with(
    l: i64,
    l_bytes: &[u8],
    r_bytes: &[u8],
    known_heap: Option<bool>,
) -> Option<i64> {
    let body = str_body(l);
    if body == 0 || !known_heap.unwrap_or_else(|| str_is_heap(l)) {
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
        // The new buffer follows the current escape-arena context; the old
        // body must free through whichever arena it actually lives in, which
        // may differ from the current context (`l` can be a global-arena
        // string a relocated task is still holding). str_alloc_two/str_free
        // each resolve that independently instead of assuming they match.
        let new_ptr = str_alloc_two(l_bytes, r_bytes);
        str_free(l);
        Some(new_ptr)
    }
}

/// Captures the slab generation of a heap string for a later staleness check.
/// A literal or foreign pointer has no generation word, so it returns the zero
/// sentinel; `olive_str_gen_stale` reads that back as "never stale".
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_gen_of(ptr: i64) -> i64 {
    let body = str_body(ptr);
    if body == 0 || !str_is_heap(ptr) {
        return 0;
    }
    unsafe { (*((body - 8) as *const AtomicU64)).load(Ordering::Relaxed) as i64 }
}

/// Whether a heap string borrow captured at generation `gen` is now stale. A
/// literal, foreign, null, or sentinel-`gen` borrow is never stale; a slab body
/// is stale once its generation moved (ignoring the shared bit) or its slot died.
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_gen_stale(ptr: i64, generation: i64) -> i64 {
    let body = str_body(ptr);
    if body == 0 || generation == 0 || !str_is_heap(ptr) {
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
        assert_eq!(str_body(a), str_body(b), "same class slot recycles");
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
        assert_eq!(str_body(a), str_body(b), "same slot recycles");
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
            let s = std::ffi::CStr::from_ptr(crate::string_slab::str_body(res) as *const std::ffi::c_char).to_bytes();
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
            let s = std::ffi::CStr::from_ptr(crate::string_slab::str_body(res) as *const std::ffi::c_char).to_bytes();
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
    fn grow_concat_of_global_arena_string_does_not_corrupt_local_pool() {
        // Warm this thread's local class-8 slab so its free-list is live
        // before the escape-arena alloc below shares the same size class.
        let warm = str_alloc(b"w");
        str_free(warm);

        let a = crate::slab::with_escape_arena(|| str_alloc(b"x"));
        assert!(crate::slab::chunk_is_global(crate::string_slab::str_body(a) as usize));

        let tail = vec![b'y'; 64];
        let grown = str_concat_inplace(a, b"x", &tail).expect("capacity must grow");
        assert_ne!(a, grown);

        let probe = str_alloc(b"z");
        assert_ne!(str_body(probe), str_body(a), "local alloc must not recycle a global-arena slot");

        str_free(grown);
        str_free(probe);
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
