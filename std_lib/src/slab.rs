//! Generational slab allocator for runtime object headers. Each slot has a
//! u64 generation at `body - 8`, odd when live, even when free, increments
//! on every transition. Stale pointer's generation never matches recycled
//! slot. Chunks never freed.

use std::alloc::Layout;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

const CHUNK_TARGET: usize = 1 << 16;

#[derive(Clone, Copy)]
struct ChunkSpan {
    start: usize,
    end: usize,
    slot_bytes: usize,
}

static CHUNK_TABLE: AtomicPtr<Vec<ChunkSpan>> = AtomicPtr::new(std::ptr::null_mut());
static CHUNK_WRITER: Mutex<()> = Mutex::new(());

fn register_chunk(start: usize, end: usize, slot_bytes: usize) {
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
        },
    );
    // Old tables leak on purpose: lock-free readers may still hold them.
    CHUNK_TABLE.store(Box::into_raw(Box::new(next)), Ordering::Release);
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
            ((gen_ptr as *mut u8).add(8), true)
        }
    }

    fn grow(&mut self) {
        let slots = (CHUNK_TARGET / self.slot_bytes).max(1);
        let bytes = slots * self.slot_bytes;
        let layout = Layout::from_size_align(bytes, 8).unwrap();
        // Zeroed so un-bumped slots read generation 0, even, dead.
        let chunk = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!chunk.is_null(), "olive: slab chunk allocation failed");
        register_chunk(chunk as usize, chunk as usize + bytes, self.slot_bytes);
        self.bump = chunk;
        self.bump_end = unsafe { chunk.add(bytes) };
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
        let (p2, fresh2) = s.alloc();
        assert_eq!(p, p2);
        assert!(!fresh2);
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
}
