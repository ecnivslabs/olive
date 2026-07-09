//! Generational slab allocator for runtime object headers. Each slot has a
//! u64 generation at `body - 8`, odd when live, even when free, increments
//! on every transition. Stale pointer's generation never matches recycled
//! slot. Chunks never freed.

use std::alloc::Layout;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

const CHUNK_TARGET: usize = 1 << 16;

#[derive(Clone, Copy)]
struct ChunkSpan {
    start: usize,
    end: usize,
    slot_bytes: usize,
    live_count: *const AtomicUsize,
}

unsafe impl Send for ChunkSpan {}
unsafe impl Sync for ChunkSpan {}

static CHUNK_TABLE: AtomicPtr<Vec<ChunkSpan>> = AtomicPtr::new(std::ptr::null_mut());
static CHUNK_WRITER: Mutex<()> = Mutex::new(());

fn register_chunk(start: usize, end: usize, slot_bytes: usize, live_count: *const AtomicUsize) {
    let _guard = CHUNK_WRITER.lock().unwrap();
    let cur = CHUNK_TABLE.load(Ordering::Acquire);
    let mut next = if cur.is_null() {
        Vec::with_capacity(8)
    } else {
        unsafe { (*cur).clone() }
    };
    let at = next.partition_point(|c| c.start < start);
    next.insert(
        at,
        ChunkSpan {
            start,
            end,
            slot_bytes,
            live_count,
        },
    );
    // Old tables leak on purpose: lock-free readers may still hold them.
    CHUNK_TABLE.store(Box::into_raw(Box::new(next)), Ordering::Release);
}

fn find_chunk_for_addr(addr: usize) -> Option<ChunkSpan> {
    let table = CHUNK_TABLE.load(Ordering::Acquire);
    if table.is_null() {
        return None;
    }
    let chunks = unsafe { &*table };
    let i = chunks.partition_point(|c| c.start <= addr);
    if i == 0 {
        return None;
    }
    let c = chunks[i - 1];
    if addr >= c.start && addr < c.end {
        Some(c)
    } else {
        None
    }
}

fn reclaim_pages(addr: usize, len: usize) {
    #[cfg(unix)]
    unsafe {
        let page_size = libc::sysconf(libc::_SC_PAGESIZE) as usize;
        if page_size > 0 {
            let start = (addr + page_size - 1) & !(page_size - 1);
            let end = (addr + len) & !(page_size - 1);
            if end > start {
                libc::madvise(start as *mut libc::c_void, end - start, libc::MADV_DONTNEED);
            }
        }
    }
    #[cfg(windows)]
    unsafe {
        let page_size = 4096;
        let start = (addr + page_size - 1) & !(page_size - 1);
        let end = (addr + len) & !(page_size - 1);
        if end > start {
            extern "system" {
                fn VirtualAlloc(
                    lpAddress: *mut std::ffi::c_void,
                    dwSize: usize,
                    flAllocationType: u32,
                    flProtect: u32,
                ) -> *mut std::ffi::c_void;
                fn VirtualUnlock(lpAddress: *mut std::ffi::c_void, dwSize: usize) -> i32;
            }
            const MEM_RESET: u32 = 0x80000;
            VirtualAlloc(start as *mut std::ffi::c_void, end - start, MEM_RESET, 0);
            VirtualUnlock(start as *mut std::ffi::c_void, end - start);
        }
    }
}

/// Whether `val` is the live body of some slab slot. Sound for arbitrary
/// words: non-chunk addresses and mid-slot pointers classify false, and
/// zeroed chunks make never-allocated slots read as dead.
pub fn ptr_is_slab_body(val: i64) -> bool {
    match slab_header_of(val) {
        Some(header) => unsafe { (*header).load(Ordering::Relaxed) & 1 == 1 },
        None => false,
    }
}

/// Whether `val` addresses a slot inside a live chunk, regardless of the
/// slot's own generation. A freed-but-not-recycled slot still lands here;
/// literals and foreign pointers do not. Callers that must tolerate `.rodata`
/// string literals use this to gate a header read the classifier would reject.
pub fn ptr_in_slab_span(val: i64) -> bool {
    slab_header_of(val).is_some()
}

fn slab_header_of(val: i64) -> Option<*const AtomicU64> {
    if val <= 0 || val & 7 != 0 {
        return None;
    }
    let table = CHUNK_TABLE.load(Ordering::Acquire);
    if table.is_null() {
        return None;
    }
    let addr = val as usize;
    let chunks = unsafe { &*table };
    let i = chunks.partition_point(|c| c.start <= addr);
    if i == 0 {
        return None;
    }
    let c = chunks[i - 1];
    if addr >= c.end || (addr - c.start) % c.slot_bytes != 16 {
        return None;
    }
    Some((addr - 8) as *const AtomicU64)
}

pub struct GenSlab {
    free_head: *mut u64,
    bump: *mut u8,
    bump_end: *mut u8,
    slot_bytes: usize,
    active_live_count: *const AtomicUsize,
}

unsafe impl Send for GenSlab {}

impl GenSlab {
    pub const fn new(body_bytes: usize) -> Self {
        let body = (body_bytes + 7) & !7;
        Self {
            free_head: std::ptr::null_mut(),
            bump: std::ptr::null_mut(),
            bump_end: std::ptr::null_mut(),
            slot_bytes: 16 + body,
            active_live_count: std::ptr::null(),
        }
    }

    /// Returns `(body, fresh)`. A fresh body is uninitialized. A recycled body
    /// keeps its previous words except word 0, which held the free-list link.
    #[inline]
    pub fn alloc(&mut self) -> (*mut u8, bool) {
        let head = self.free_head;
        if !head.is_null() {
            unsafe {
                let body = (head as *mut u8).add(16);
                self.free_head = *(body as *const *mut u64);
                let gen_ptr = head.add(1) as *mut AtomicU64;
                let g = (*gen_ptr).load(Ordering::Relaxed) + 1;
                (*gen_ptr).store(g, Ordering::Release);
                if let Some(chunk) = find_chunk_for_addr(body as usize) {
                    (*chunk.live_count).fetch_add(1, Ordering::Relaxed);
                }
                return (body, false);
            }
        }
        if self.bump.is_null() || unsafe { self.bump.add(self.slot_bytes) } > self.bump_end {
            self.grow();
        }
        unsafe {
            let gen_ptr = self.bump.add(8) as *mut AtomicU64;
            self.bump = self.bump.add(self.slot_bytes);
            (*gen_ptr).store(1, Ordering::Release);
            let body = (gen_ptr as *mut u8).add(8);
            if !self.active_live_count.is_null() {
                (*self.active_live_count).fetch_add(1, Ordering::Relaxed);
            }
            (body, true)
        }
    }

    fn grow(&mut self) {
        let slots = (CHUNK_TARGET / self.slot_bytes).max(1);
        let bytes = slots * self.slot_bytes;
        let layout = Layout::from_size_align(bytes, 8).unwrap();
        // Zeroed so un-bumped slots read generation 0, even, dead.
        let chunk = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!chunk.is_null(), "olive: slab chunk allocation failed");
        let live_count = Box::into_raw(Box::new(AtomicUsize::new(0)));
        register_chunk(
            chunk as usize,
            chunk as usize + bytes,
            self.slot_bytes,
            live_count,
        );
        self.bump = chunk;
        self.bump_end = unsafe { chunk.add(bytes) };
        self.active_live_count = live_count;
    }

    fn count_chunk_slots(&self, start: usize, end: usize) -> usize {
        unsafe {
            let mut count = 0;
            let mut curr = self.free_head;
            while !curr.is_null() {
                let addr = curr as usize;
                if addr >= start && addr < end {
                    count += 1;
                }
                let body = (curr as *mut u8).add(16);
                curr = *(body as *const *mut u64);
            }
            count
        }
    }

    fn unlink_chunk_slots(&mut self, start: usize, end: usize) {
        unsafe {
            let mut prev_next_ptr: *mut *mut u64 = &mut self.free_head;
            let mut curr = self.free_head;
            while !curr.is_null() {
                let addr = curr as usize;
                let body = (curr as *mut u8).add(16);
                let next = *(body as *const *mut u64);
                if addr >= start && addr < end {
                    *prev_next_ptr = next;
                } else {
                    prev_next_ptr = body as *mut *mut u64;
                }
                curr = next;
            }
        }
    }

    /// Frees a slot. Returns `false` if the slot was already free, so a
    /// double free through a stale generation read degrades to a no-op.
    #[inline]
    pub fn free(&mut self, body: *mut u8) -> bool {
        unsafe {
            let gen_ptr = (body as *mut AtomicU64).sub(1);
            let generation = (*gen_ptr).load(Ordering::Relaxed);
            if generation & 1 == 0 {
                return false;
            }
            (*gen_ptr).store(generation + 1, Ordering::Release);
            *(body as *mut *mut u64) = self.free_head;
            self.free_head = (body as *mut u64).sub(2);
            #[cfg(debug_assertions)]
            {
                let body_size = self.slot_bytes - 16;
                if body_size > 8 {
                    std::ptr::write_bytes(body.add(8), 0x5a, body_size - 8);
                }
            }
            if let Some(chunk) = find_chunk_for_addr(body as usize) {
                let prev = (*chunk.live_count).fetch_sub(1, Ordering::Release);
                if prev == 1 {
                    let is_active =
                        self.bump as usize >= chunk.start && (self.bump as usize) < chunk.end;
                    if !is_active {
                        let total_slots = (chunk.end - chunk.start) / chunk.slot_bytes;
                        let count = self.count_chunk_slots(chunk.start, chunk.end);
                        if count == total_slots {
                            self.unlink_chunk_slots(chunk.start, chunk.end);
                            reclaim_pages(chunk.start, chunk.end - chunk.start);
                        }
                    }
                }
            }
            true
        }
    }
}

/// Whether a slab slot is currently live. Only valid for pointers returned by
/// a `GenSlab`; the caller guarantees provenance.
#[inline]
pub fn slot_is_live(body: i64) -> bool {
    unsafe { (*((body as *const AtomicU64).sub(1))).load(Ordering::Relaxed) & 1 == 1 }
}

/// Current generation word of a slab slot.
#[inline]
pub fn slot_generation(body: i64) -> u64 {
    unsafe { (*((body as *const AtomicU64).sub(1))).load(Ordering::Relaxed) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_is_live_free_is_not() {
        let mut s = GenSlab::new(32);
        let (p, fresh) = s.alloc();
        assert!(fresh);
        assert!(slot_is_live(p as i64));
        assert!(s.free(p));
        assert!(!slot_is_live(p as i64));
    }

    #[test]
    fn double_free_absorbed() {
        let mut s = GenSlab::new(32);
        let (p, _) = s.alloc();
        assert!(s.free(p));
        assert!(!s.free(p));
    }

    #[test]
    fn recycle_bumps_generation() {
        let mut s = GenSlab::new(16);
        let (p, _) = s.alloc();
        let g0 = slot_generation(p as i64);
        s.free(p);
        let (p2, fresh) = s.alloc();
        assert_eq!(p, p2);
        assert!(!fresh);
        assert_eq!(slot_generation(p2 as i64), g0 + 2);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn recycled_body_keeps_tail_words() {
        let mut s = GenSlab::new(32);
        let (p, _) = s.alloc();
        unsafe {
            *(p as *mut i64) = 1;
            *(p as *mut i64).add(1) = 42;
            *(p as *mut i64).add(2) = 43;
        }
        s.free(p);
        let (p2, _) = s.alloc();
        assert_eq!(p, p2);
        unsafe {
            assert_eq!(*(p2 as *const i64).add(1), 42);
            assert_eq!(*(p2 as *const i64).add(2), 43);
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    fn recycled_body_is_poisoned_in_debug() {
        let mut s = GenSlab::new(32);
        let (p, _) = s.alloc();
        unsafe {
            *(p as *mut i64) = 1;
            *(p as *mut i64).add(1) = 42;
            *(p as *mut i64).add(2) = 43;
        }
        s.free(p);
        let (p2, _) = s.alloc();
        assert_eq!(p, p2);
        unsafe {
            assert_eq!(*(p2 as *const i64).add(1), 0x5a5a5a5a5a5a5a5a);
            assert_eq!(*(p2 as *const i64).add(2), 0x5a5a5a5a5a5a5a5a);
        }
    }

    #[test]
    fn distinct_slots_until_freed() {
        let mut s = GenSlab::new(24);
        let (a, _) = s.alloc();
        let (b, _) = s.alloc();
        assert_ne!(a, b);
        s.free(a);
        let (c, _) = s.alloc();
        assert_eq!(a, c);
    }

    #[test]
    fn crosses_chunk_boundary() {
        let mut s = GenSlab::new(1024);
        let mut ptrs = Vec::new();
        for _ in 0..200 {
            let (p, fresh) = s.alloc();
            assert!(fresh);
            assert!(slot_is_live(p as i64));
            ptrs.push(p);
        }
        ptrs.sort();
        ptrs.dedup();
        assert_eq!(ptrs.len(), 200);
    }

    #[test]
    fn classifier_tracks_liveness() {
        let mut s = GenSlab::new(32);
        let (p, _) = s.alloc();
        assert!(ptr_is_slab_body(p as i64));
        s.free(p);
        assert!(!ptr_is_slab_body(p as i64));
        let (p2, _) = s.alloc();
        assert_eq!(p, p2);
        assert!(ptr_is_slab_body(p2 as i64));
    }

    #[test]
    fn classifier_rejects_foreign_words() {
        assert!(!ptr_is_slab_body(0));
        assert!(!ptr_is_slab_body(-8));
        assert!(!ptr_is_slab_body(42));
        let heap = Box::into_raw(Box::new(0u64));
        assert!(!ptr_is_slab_body(heap as i64));
        drop(unsafe { Box::from_raw(heap) });
    }

    #[test]
    fn classifier_rejects_mid_slot_and_unbumped() {
        let mut s = GenSlab::new(32);
        let (p, _) = s.alloc();
        let slot = 8 + 32;
        assert!(!ptr_is_slab_body(p as i64 + 8), "interior pointer");
        assert!(!ptr_is_slab_body(p as i64 - 8), "header address");
        let next_body = p as i64 + slot as i64;
        if (next_body as usize) < s.bump_end as usize {
            assert!(!ptr_is_slab_body(next_body), "un-bumped slot is dead");
        }
    }

    #[test]
    fn oversized_body_gets_single_slot_chunks() {
        let mut s = GenSlab::new(CHUNK_TARGET * 2);
        let (p, fresh) = s.alloc();
        assert!(fresh);
        unsafe { *p.add(CHUNK_TARGET * 2 - 1) = 7 };
        assert!(s.free(p));
        // Reclaimed since single-slot chunk hits zero live count.
        let (p2, fresh2) = s.alloc();
        assert_ne!(p, p2);
        assert!(fresh2);
    }

    #[test]
    fn threaded_alloc_free_stress() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let slab = Arc::new(std::sync::Mutex::new(GenSlab::new(32)));
        let alloc_count = Arc::new(AtomicUsize::new(0));
        let free_count = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let s = slab.clone();
            let ac = alloc_count.clone();
            let fc = free_count.clone();
            handles.push(std::thread::spawn(move || {
                let mut local_ptrs = Vec::new();
                for _ in 0..500 {
                    let (p, fresh) = s.lock().unwrap().alloc();
                    assert!(slot_is_live(p as i64), "slot live after alloc");
                    if !fresh {
                        assert!(slot_generation(p as i64) & 1 == 1, "recycled gen odd");
                    }
                    ac.fetch_add(1, Ordering::Relaxed);
                    local_ptrs.push(p);
                    if local_ptrs.len() > 10 {
                        let victim = local_ptrs.pop().unwrap();
                        {
                            let mut guard = s.lock().unwrap();
                            assert!(guard.free(victim), "free of live slot");
                            assert!(!slot_is_live(victim as i64), "slot dead after free");
                        }
                        fc.fetch_add(1, Ordering::Relaxed);
                    }
                }
                for p in local_ptrs {
                    {
                        let mut guard = s.lock().unwrap();
                        assert!(guard.free(p), "free remaining slot");
                        assert!(!slot_is_live(p as i64), "slot dead after free");
                    }
                    fc.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            alloc_count.load(Ordering::Relaxed),
            free_count.load(Ordering::Relaxed)
        );
    }

    fn get_vm_rss() -> Option<usize> {
        #[cfg(target_os = "linux")]
        {
            if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
                for line in status.lines() {
                    if line.starts_with("VmRSS:") {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 2 {
                            let kb = parts[1].parse::<usize>().ok()?;
                            return Some(kb * 1024);
                        }
                    }
                }
            }
        }
        None
    }

    #[test]
    #[ignore]
    fn test_memory_reclamation_rss() {
        let mut s = GenSlab::new(1024);
        let mut ptrs = Vec::new();
        for _ in 0..100 {
            ptrs.push(s.alloc().0);
        }
        for p in ptrs.drain(..) {
            s.free(p);
        }
        let baseline = get_vm_rss().unwrap_or(0);
        for _ in 0..40000 {
            ptrs.push(s.alloc().0);
        }
        let spiked = get_vm_rss().unwrap_or(0);
        if spiked > 0 && baseline > 0 {
            assert!(spiked > baseline);
        }
        for p in ptrs.drain(..) {
            s.free(p);
        }
        let post_free = get_vm_rss().unwrap_or(0);
        if post_free > 0 && spiked > 0 {
            assert!(post_free < spiked);
        }
    }
}
