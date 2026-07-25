use super::*;

#[test]
fn teardown_cleans_initialized_backing_storage() {
    use std::sync::Arc;

    unsafe fn cleanup(body: *mut u8) {
        unsafe { std::ptr::drop_in_place(body.add(8) as *mut Arc<()>) };
    }

    let owner = Arc::new(());
    let weak = Arc::downgrade(&owner);
    let mut slab = GenSlab::with_cleanup(8 + std::mem::size_of::<Arc<()>>(), cleanup);
    for freed in [false, true] {
        let body = slab.alloc().0;
        unsafe { std::ptr::write(body.add(8) as *mut Arc<()>, owner.clone()) };
        if freed {
            if cfg!(debug_assertions) {
                unsafe { cleanup(body) };
            }
            slab.free(body);
        }
    }
    drop(owner);
    assert!(weak.upgrade().is_some());
    drop(slab);
    assert!(weak.upgrade().is_none());
}

#[test]
fn empty_bodies_have_space_for_the_free_list() {
    let mut slab = GenSlab::new(0);
    let slots: Vec<_> = (0..5000).map(|_| slab.alloc().0).collect();
    for &slot in &slots {
        assert!(slot_is_live(slot as i64));
        assert!(slab.free(slot));
    }
    for &slot in slots.iter().rev() {
        assert_eq!(slab.alloc(), (slot, false));
    }
}

#[test]
fn overflowing_body_sizes_are_rejected() {
    for bytes in [usize::MAX, usize::MAX - 7, usize::MAX - 16] {
        assert!(std::panic::catch_unwind(|| GenSlab::new(bytes)).is_err());
    }
}

#[test]
fn concurrent_classification_and_teardown() {
    use std::sync::atomic::{AtomicBool, AtomicI64};
    let address = AtomicI64::new(0);
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                while !done.load(Ordering::Acquire) {
                    let value = address.load(Ordering::Acquire);
                    slot_generation(value);
                    ptr_is_slab_body(value);
                }
            });
        }
        for _ in 0..1000 {
            let mut slab = GenSlab::new(32);
            address.store(slab.alloc().0 as i64, Ordering::Release);
            std::thread::yield_now();
        }
        done.store(true, Ordering::Release);
    });
}

#[test]
fn repeated_allocation_waves_reuse_chunks() {
    let mut slab = GenSlab::new(1024);
    let mut peak_chunks = 0;
    for wave in 0..8 {
        let slots: Vec<_> = (0..200).map(|_| slab.alloc().0).collect();
        if wave == 0 {
            peak_chunks = slab.chunks.len();
        }
        assert_eq!(slab.chunks.len(), peak_chunks, "allocation wave {wave}");
        for slot in slots {
            assert!(slab.free(slot));
        }
    }
}

#[test]
fn full_chunk_reuse_preserves_generations() {
    let mut slab = GenSlab::new(CHUNK_TARGET * 2);
    let (slot, _) = slab.alloc();
    let generation = slot_generation(slot as i64);
    assert!(slab.free(slot));
    let (reused, fresh) = slab.alloc();
    assert_eq!(reused, slot);
    assert!(!fresh);
    assert_eq!(slot_generation(reused as i64), generation + 2);
}

#[test]
fn cache_invalidation_does_not_restore_a_retired_span() {
    let mut first = GenSlab::new(32);
    let mut second = GenSlab::new(32);
    let a = first.alloc().0 as i64;
    let b = second.alloc().0 as i64;
    registry::LAST_CHUNKS.with(|cache| cache.set(([None, None], false, 0)));
    assert!(ptr_in_slab_span(a));
    assert!(ptr_in_slab_span(b));
    drop(second);
    assert!(ptr_in_slab_span(a));
    assert!(!ptr_in_slab_span(b));
}

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
fn fresh_chunks_do_not_reuse_retired_generations() {
    let mut generations = std::collections::HashSet::new();
    for _ in 0..64 {
        let mut slab = GenSlab::new(CHUNK_TARGET * 2);
        let value = slab.alloc().0 as i64;
        assert!(generations.insert(slot_generation(value)));
    }
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
    let slot = s.slot_bytes;
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
