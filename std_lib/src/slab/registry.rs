use std::cell::Cell;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{RwLock, RwLockReadGuard};

#[derive(Clone, Copy)]
pub(super) struct ChunkSpan {
    pub start: usize,
    pub end: usize,
    pub slot_bytes: usize,
    pub is_global: bool,
}

struct Registry {
    chunks: Vec<ChunkSpan>,
    epoch: u64,
}

static CHUNKS: RwLock<Registry> = RwLock::new(Registry {
    chunks: Vec::new(),
    epoch: 0,
});

// A conservative bound lets literals and foreign pointers skip locking.
static SPAN_MIN: AtomicUsize = AtomicUsize::new(usize::MAX);
static SPAN_MAX: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    pub(super) static LAST_CHUNKS: Cell<([Option<ChunkSpan>; 2], bool, u64)> =
        const { Cell::new(([None, None], false, 0)) };
}

/// Keeps teardown from freeing chunk memory while a caller reads its header.
pub(super) struct ChunkGuard {
    span: ChunkSpan,
    _guard: RwLockReadGuard<'static, Registry>,
}

impl Deref for ChunkGuard {
    type Target = ChunkSpan;

    fn deref(&self) -> &ChunkSpan {
        &self.span
    }
}

pub(super) fn register_chunk(start: usize, end: usize, slot_bytes: usize, is_global: bool) {
    let mut registry = CHUNKS.write().unwrap();
    let at = registry.chunks.partition_point(|c| c.start < start);
    registry.chunks.insert(
        at,
        ChunkSpan {
            start,
            end,
            slot_bytes,
            is_global,
        },
    );
    registry.epoch = registry.epoch.wrapping_add(1);
    SPAN_MIN.fetch_min(start, Ordering::Relaxed);
    SPAN_MAX.fetch_max(end, Ordering::Relaxed);
}

pub(super) fn unregister_chunk(start: usize) {
    let mut registry = CHUNKS.write().unwrap();
    let at = registry
        .chunks
        .binary_search_by_key(&start, |c| c.start)
        .expect("olive: slab chunk missing from registry");
    registry.chunks.remove(at);
    registry.epoch = registry.epoch.wrapping_add(1);
    if registry.chunks.is_empty() {
        registry.chunks = Vec::new();
    } else if registry.chunks.len() < registry.chunks.capacity() / 4 {
        let capacity = registry.chunks.len() * 2;
        registry.chunks.shrink_to(capacity);
    }
}

pub(super) fn find_chunk_for_addr(addr: usize) -> Option<ChunkGuard> {
    if addr < SPAN_MIN.load(Ordering::Relaxed) || addr >= SPAN_MAX.load(Ordering::Relaxed) {
        return None;
    }
    let registry = CHUNKS.read().unwrap();
    let (mut cached, mut next_slot, cached_epoch) = LAST_CHUNKS.get();
    if cached_epoch != registry.epoch {
        cached = [None, None];
        next_slot = false;
        LAST_CHUNKS.set((cached, next_slot, registry.epoch));
    }
    for span in cached.iter().flatten() {
        if addr >= span.start && addr < span.end {
            return Some(ChunkGuard {
                span: *span,
                _guard: registry,
            });
        }
    }
    let at = registry.chunks.partition_point(|c| c.start <= addr);
    let span = *registry.chunks.get(at.checked_sub(1)?)?;
    if addr >= span.end {
        return None;
    }
    cached[next_slot as usize] = Some(span);
    LAST_CHUNKS.set((cached, !next_slot, registry.epoch));
    Some(ChunkGuard {
        span,
        _guard: registry,
    })
}
