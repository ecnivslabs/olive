//! Size-classed generational slab for heap strings. A slot's body holds the
//! nul-terminated bytes inline, so a string pointer is the body address with
//! the string and heap tag bits set. Literals live in read-only data with a
//! length header and a distinct literal tag, so freeing one is a no-op.

use crate::slab::GenSlab;
use std::cell::UnsafeCell;

/// Marks a word as an Olive string pointer rather than a raw scalar.
pub const STR_TAG: i64 = 1;

/// Marks a string pointer as slab-allocated, so it carries a header at
/// body-16 and frees through a slab. Stamped once by the allocator, which
/// already knows the answer, so the hot paths read a bit instead of running
/// a chunk-table lookup to rediscover it. Literals and foreign pointers keep
/// this clear: codegen aligns literal data to 4 and the interned char table
/// strides by 4, so no untagged string address can set it by accident.
pub const STR_HEAP: i64 = 2;

/// Marks a read-only literal carrying its byte length in the word before its
/// body. Foreign C pointers are aligned to at least eight bytes, so bit two is
/// clear for those pointers.
pub const STR_LITERAL: i64 = 4;

/// Strips the tag bits, yielding the address of the string bytes.
#[inline]
pub fn str_body(ptr: i64) -> i64 {
    ptr & !(STR_TAG | STR_HEAP | STR_LITERAL)
}

/// Whether `ptr` is slab-allocated (has a header, frees through a slab).
#[inline]
pub fn str_is_heap(ptr: i64) -> bool {
    ptr & STR_HEAP != 0
}

/// Whether `ptr` addresses a length-bearing read-only literal.
#[inline]
pub fn str_is_literal(ptr: i64) -> bool {
    ptr & STR_LITERAL != 0
}

/// Upper bound for a literal byte length. Real literals are embedded program
/// text; anything larger is a forged header, and bounding keeps the mapped
/// range walk below from iterating an absurd page count.
pub(crate) const MAX_LITERAL_LEN: usize = 1 << 28;

/// Upper bound for the foreign C-string fallback scan. A longer NUL-free
/// run is treated as corrupt rather than walked without end.
pub(crate) const MAX_FOREIGN_LEN: usize = 1 << 24;

fn page_size() -> usize {
    static PS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *PS.get_or_init(|| unsafe {
        let ps = libc::sysconf(libc::_SC_PAGESIZE) as usize;
        if ps.is_power_of_two() && ps >= 4096 {
            ps
        } else {
            4096
        }
    })
}

/// Whether the page containing `addr` is mapped readable, probed with
/// `msync` so an unmapped forged address reports false instead of faulting.
/// Never dereferences the address itself.
pub(crate) fn page_mapped(addr: usize) -> bool {
    if addr < 0x1000 {
        return false;
    }
    let ps = page_size();
    let page = addr & !(ps - 1);
    unsafe {
        if libc::msync(page as *mut libc::c_void, 1, libc::MS_ASYNC) == 0 {
            true
        } else {
            false
        }
    }
}

/// Whether every page in `[start, start+len)` is mapped. Bounds `len` first
/// so a forged huge length fails fast without walking millions of pages.
fn range_mapped(start: usize, len: usize, cap: usize) -> bool {
    if len > cap {
        return false;
    }
    if start == 0 {
        return false;
    }
    let Some(end) = start.checked_add(len) else {
        return false;
    };
    if end < start {
        return false;
    }
    let ps = page_size();
    let mut page = start & !(ps - 1);
    let last = if end == start {
        page
    } else {
        (end - 1) & !(ps - 1)
    };
    loop {
        if !page_mapped(page) {
            return false;
        }
        if page == last {
            return true;
        }
        let Some(next) = page.checked_add(ps) else {
            return false;
        };
        page = next;
    }
}

/// Whether `body` belongs to any live string class slab: this thread's pool,
/// the active escape arena, its source, or the shared global arena. Tells a
/// heap string body from a list, box, or forged address that happens to sit
/// inside some other slab's chunk.
pub fn owns_str_body(body: i64) -> bool {
    if body == 0 {
        return false;
    }
    let addr = body as usize;
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            if (*active).str_slabs.iter().flatten().any(|s| s.owns_addr(addr)) {
                return true;
            }
            let source = crate::slab::SOURCE_SLABS.get();
            if !source.is_null()
                && source != active
                && (*source)
                    .str_slabs
                    .iter()
                    .flatten()
                    .any(|s| s.owns_addr(addr))
            {
                return true;
            }
        }
    }
    if STR_SLABS.with(|s| unsafe {
        (&*s.get())
            .classes
            .iter()
            .flatten()
            .any(|slab| slab.owns_addr(addr))
    }) {
        return true;
    }
    crate::slab::global_str_owns_addr(addr)
}

/// Validated heap-string length: membership, liveness, string-slab ownership,
/// and a header whose capacity class and length agree. Reads the header only
/// after the chunk lookup proves the slot address is mapped, so a forged word
/// returns `None` instead of faulting. Called before any header or body
/// dereference on the heap path.
pub(crate) fn live_heap_str_len(body: i64) -> Option<usize> {
    if body == 0 || body & 7 != 0 {
        return None;
    }
    if crate::slab::slab_membership(body).is_none() {
        return None;
    }
    if !crate::slab::slot_is_live(body) {
        return None;
    }
    if !owns_str_body(body) {
        return None;
    }
    let header_val = unsafe { *(body as *const usize).sub(2) };
    let cap_idx = header_val >> 48;
    if cap_idx >= 32 {
        return None;
    }
    let cap = 1usize.checked_shl(cap_idx as u32)?;
    if cap < 8 {
        return None;
    }
    let len = header_val & LEN_MASK;
    let need = len.checked_add(1)?;
    if need > cap {
        return None;
    }
    Some(len)
}

/// Whether a tagged word is a live heap string. Unified with
/// `live_heap_str_len` so every classifier agrees.
#[inline]
pub fn is_live_heap_str(ptr: i64) -> bool {
    if !str_is_heap(ptr) {
        return false;
    }
    live_heap_str_len(str_body(ptr)).is_some()
}

/// Validated literal length: header pages mapped, zero high header word,
/// sane length, and a fully mapped body. Returns `None` for any forged word
/// without faulting.
pub(crate) fn literal_str_len_validated(body: i64) -> Option<usize> {
    if body == 0 || body & 7 != 0 {
        return None;
    }
    if body <= 0x10000 {
        return None;
    }
    if crate::slab::ptr_in_slab_span(body) {
        return None;
    }
    let body_addr = body as usize;
    if !range_mapped(body_addr.wrapping_sub(16), 16, 16) {
        return None;
    }
    let (hi, len) = unsafe {
        (
            *((body_addr - 16) as *const u64),
            *((body_addr - 8) as *const usize),
        )
    };
    if hi != 0 {
        return None;
    }
    if len > MAX_LITERAL_LEN {
        return None;
    }
    if !range_mapped(body_addr, len.saturating_add(1), MAX_LITERAL_LEN + 1) {
        return None;
    }
    if len > 0 && unsafe { *((body_addr + len) as *const u8) } != 0 {
        return None;
    }
    Some(len)
}

/// Bounded foreign C-string length: per-page mapped probe with an explicit
/// cap, so an arbitrary forged pointer yields `None` instead of an unbounded
/// `strlen` fault.
pub(crate) fn foreign_str_len_bounded(body: i64) -> Option<usize> {
    if body == 0 || body & 7 != 0 {
        return None;
    }
    if body <= 0x10000 {
        return None;
    }
    if crate::slab::ptr_in_slab_span(body) {
        return None;
    }
    let mut addr = body as usize;
    let start = addr;
    let ps = page_size();
    let mut checked_page = usize::MAX;
    while addr.wrapping_sub(start) < MAX_FOREIGN_LEN {
        let page = addr & !(ps - 1);
        if page != checked_page {
            if !page_mapped(addr) {
                return None;
            }
            checked_page = page;
        }
        if unsafe { *(addr as *const u8) } == 0 {
            return Some(addr - start);
        }
        addr += 1;
    }
    None
}

/// Byte capacity class for `need` content-plus-nul bytes, one machine word min.
/// Faults instead of wrapping or panicking when `need` overflows the address
/// space or exceeds the 32 size classes. No silent truncation.
#[inline]
fn class_bytes(need: usize) -> usize {
    class_bytes_opt(need)
        .unwrap_or_else(|| crate::panic::abort("string length overflows address space", None))
}

pub(crate) fn class_bytes_opt(need: usize) -> Option<usize> {
    let pow = need.checked_next_power_of_two()?;
    let cap = pow.max(8);
    if cap.trailing_zeros() as usize >= 32 {
        return None;
    }
    Some(cap)
}

const LEN_MASK: usize = 0xFFFFFFFFFFFF;

/// Checked `a + b` for concatenation lengths. Faults, never wraps.
fn checked_concat_len(a_len: usize, b_len: usize) -> usize {
    concat_len_opt(a_len, b_len)
        .unwrap_or_else(|| crate::panic::abort("string concat overflows length", None))
}

pub(crate) fn concat_len_opt(a_len: usize, b_len: usize) -> Option<usize> {
    a_len.checked_add(b_len)
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
                (*active).str_slabs[idx] = Some(GenSlab::new(cap).with_global((*active).is_global));
            }
            f((*active).str_slabs[idx].as_mut().unwrap_unchecked())
        } else {
            STR_SLABS.with(|s| f((&mut *s.get()).slab(cap)))
        }
    }
}

/// Allocates a heap string from `bytes`. Stores capacity class and length at
/// body-16 for O(1) free, including strings with interior NUL bytes. Returns
/// the body pointer tagged with the low string bit.
pub fn str_alloc(bytes: &[u8]) -> i64 {
    let len = bytes.len();
    let need = len
        .checked_add(1)
        .unwrap_or_else(|| crate::panic::abort("string length overflows address space", None));
    let cap = class_bytes(need);
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

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_alloc(ptr: *const u8, len: usize) -> i64 {
    if ptr.is_null() {
        return 0;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    str_alloc(slice)
}

/// Same as `str_alloc`, but writes two slices back to back without first
/// concatenating them into an intermediate buffer.
pub fn str_alloc_two(a: &[u8], b: &[u8]) -> i64 {
    let len = checked_concat_len(a.len(), b.len());
    let need = len
        .checked_add(1)
        .unwrap_or_else(|| crate::panic::abort("string concat overflows length", None));
    let cap = class_bytes(need);
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
    // Which arena the body lives in decides which pool frees it. The ambient
    // escape-arena context must not redirect the free: a local body released
    // while a task runs inside the arena would otherwise land on the arena's
    // free-list, letting a later arena alloc hand out a slot the local pool
    // also owns.
    match crate::slab::slab_membership(body) {
        None => {}
        Some(is_global) => str_free_in(ptr, is_global),
    }
}

fn str_free_in(ptr: i64, _is_global: bool) {
    let body = str_body(ptr);
    let header_val = unsafe { *(body as *const usize).sub(2) };
    let cap_idx = header_val >> 48;
    if cap_idx >= 32 {
        return;
    }
    let body_addr = body as usize;

    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null()
            && let Some(slab) = (*active).str_slabs.get(cap_idx).and_then(Option::as_ref)
            && slab.owns_addr(body_addr)
        {
            if let Some(slab) = (*active)
                .str_slabs
                .get_mut(cap_idx)
                .and_then(Option::as_mut)
            {
                slab.free(body as *mut u8);
            }
            return;
        }

        let source = crate::slab::SOURCE_SLABS.get();
        if !source.is_null()
            && let Some(slab) = (*source).str_slabs.get(cap_idx).and_then(Option::as_ref)
            && slab.owns_addr(body_addr)
        {
            if let Some(slab) = (*source)
                .str_slabs
                .get_mut(cap_idx)
                .and_then(Option::as_mut)
            {
                slab.free(body as *mut u8);
            }
            return;
        }
    }

    if !crate::slab::active_slab_is_global()
        && crate::slab::global_str_free_if_owned(body_addr, cap_idx)
    {
        return;
    }

    STR_SLABS.with(|s| unsafe {
        if let Some(slab) = (&mut *s.get()).classes[cap_idx].as_mut()
            && slab.owns_addr(body_addr)
        {
            slab.free(body as *mut u8);
        }
    });
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
    if crate::slab::slab_membership(body).is_none() {
        return None;
    }
    if !crate::slab::slot_is_live(body) {
        return None;
    }
    let header_val = unsafe { *(body as *const usize).sub(2) };
    let cap_idx = header_val >> 48;
    if cap_idx >= 32 {
        return None;
    }
    let old_cap = 1usize << cap_idx;
    if old_cap < 8 {
        return None;
    }
    let header_len = header_val & LEN_MASK;
    if header_len.checked_add(1).is_none_or(|need| need > old_cap) {
        return None;
    }
    if l_bytes.len() != header_len {
        return None;
    }
    let l_len = l_bytes.len();
    let r_len = r_bytes.len();
    let new_len = checked_concat_len(l_len, r_len);
    let need = new_len
        .checked_add(1)
        .unwrap_or_else(|| crate::panic::abort("string concat overflows length", None));
    let new_cap = class_bytes(need);
    if new_cap <= old_cap {
        unsafe {
            std::ptr::copy_nonoverlapping(r_bytes.as_ptr(), (body as *mut u8).add(l_len), r_len);
            *(body as *mut u8).add(new_len) = 0;
            // Keep the stored capacity class, not the grown one: the slot
            // physically holds `old_cap` bytes, and a later free reads this
            // word to pick which slab releases it.
            let new_header = new_len | (cap_idx << 48);
            *(body as *mut usize).sub(2) = new_header;
        }
        Some(l)
    } else {
        // The new buffer follows the current escape-arena context; the old
        // body frees through whichever arena it actually lives in, which may
        // differ from the current context (`l` can be a local-pool string a
        // task running inside the arena is still holding).
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
    crate::slab::slot_generation(body) as i64
}

/// Whether a heap string borrow captured at generation `gen` is now stale. A
/// literal, null, or sentinel-`gen` borrow is never stale. A captured slab
/// generation becomes stale on reuse, free, or retirement of its arena.
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_gen_stale(ptr: i64, generation: i64) -> i64 {
    let body = str_body(ptr);
    if body == 0 || generation == 0 || !str_is_heap(ptr) {
        return 0;
    }
    let cur = crate::slab::slot_generation(body) as i64;
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
            let s = std::ffi::CStr::from_ptr(
                crate::string_slab::str_body(res) as *const std::ffi::c_char
            )
            .to_bytes();
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
            let s = std::ffi::CStr::from_ptr(
                crate::string_slab::str_body(res) as *const std::ffi::c_char
            )
            .to_bytes();
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
        assert!(crate::slab::chunk_is_global(
            crate::string_slab::str_body(a) as usize
        ));

        let tail = vec![b'y'; 64];
        let grown = str_concat_inplace(a, b"x", &tail).expect("capacity must grow");
        assert_ne!(a, grown);

        let probe = str_alloc(b"z");
        assert_ne!(
            str_body(probe),
            str_body(a),
            "local alloc must not recycle a global-arena slot"
        );

        str_free(grown);
        str_free(probe);
    }

    #[test]
    fn free_of_local_body_inside_escape_arena_stays_local() {
        let warm = str_alloc(b"w");
        str_free(warm);

        let local = str_alloc(b"local");
        let born = olive_str_gen_of(local);

        // Allocations and local frees inside the arena context need no lock;
        // freeing a *global* body here would re-enter `with_escape_arena` and
        // deadlock on GLOBAL_SLABS, so the arena probes leave unfreed.
        let (probe_a, probe_b) = crate::slab::with_escape_arena(|| {
            let a = str_alloc(b"a");
            assert!(crate::slab::chunk_is_global(str_body(a) as usize));
            // The local body must route to this thread's own pool even while
            // the shared arena is the active context.
            str_free(local);
            let b = str_alloc(b"b");
            (a, b)
        });

        assert_ne!(
            str_body(probe_b),
            str_body(local),
            "arena alloc recycled a local-pool slot"
        );
        // The body is genuinely dead in its own pool: generation advanced.
        assert_eq!(olive_str_gen_stale(local, born), 1);

        str_free(probe_a);
        str_free(probe_b);
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

    #[test]
    fn concat_len_opt_rejects_overflow() {
        assert_eq!(concat_len_opt(3, 4), Some(7));
        assert_eq!(concat_len_opt(0, 0), Some(0));
        assert_eq!(concat_len_opt(usize::MAX, 1), None);
        assert_eq!(concat_len_opt(usize::MAX, usize::MAX), None);
    }

    #[test]
    fn class_bytes_opt_rejects_overflow() {
        assert_eq!(class_bytes_opt(1), Some(8));
        assert_eq!(class_bytes_opt(8), Some(8));
        assert_eq!(class_bytes_opt(9), Some(16));
        assert_eq!(class_bytes_opt(usize::MAX), None);
        assert_eq!(class_bytes_opt(1usize << 32), None);
    }

    #[test]
    fn inplace_refuses_dead_slot() {
        let a = str_alloc(b"ab");
        str_free(a);
        assert!(
            str_concat_inplace(a, b"ab", b"cd").is_none(),
            "dead slot must not mutate in place"
        );
    }

    #[test]
    fn inplace_refuses_literal_and_foreign() {
        let c = std::ffi::CString::new("lit").unwrap();
        let raw = c.into_raw();
        let foreign = raw as i64;
        assert!(str_concat_inplace(foreign | 1, b"lit", b"!").is_none());
        let _ = unsafe { std::ffi::CString::from_raw(raw) };
    }

    #[test]
    fn inplace_refuses_forged_heap_outside_any_slab() {
        let forged = 0x1_0000_i64 | STR_TAG | STR_HEAP;
        assert!(str_concat_inplace(forged, b"x", b"y").is_none());
    }

    #[test]
    fn inplace_refuses_corrupt_header_size_class() {
        let p = str_alloc(b"abc");
        let body = str_body(p);
        let orig: usize = unsafe { *(body as *const usize).sub(2) };
        unsafe { *(body as *mut usize).sub(2) = (orig & LEN_MASK) | (33usize << 48) };
        assert!(str_concat_inplace(p, b"abc", b"d").is_none());
        unsafe { *(body as *mut usize).sub(2) = orig };
        str_free(p);
    }

    #[test]
    fn inplace_refuses_corrupt_header_length_exceeds_capacity() {
        let p = str_alloc(b"abc");
        let body = str_body(p);
        let orig: usize = unsafe { *(body as *const usize).sub(2) };
        let cap_idx = orig >> 48;
        let old_cap = 1usize << cap_idx;
        unsafe { *(body as *mut usize).sub(2) = (old_cap | (cap_idx << 48)) as usize };
        assert!(str_concat_inplace(p, b"abc", b"d").is_none());
        unsafe { *(body as *mut usize).sub(2) = orig };
        str_free(p);
    }

    #[test]
    fn inplace_refuses_stale_slice_length_mismatch() {
        let p = str_alloc(b"abc");
        assert!(str_concat_inplace(p, b"ab", b"d").is_none());
        assert_eq!(crate::string::olive_str_from_ptr(p), "abc");
        str_free(p);
    }
}
