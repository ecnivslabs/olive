//! Generational slab allocator for runtime object headers. Each slot has a
//! u64 generation at `body - 8`, odd when live, even when free, increments
//! on every transition. Stale pointer's generation never matches recycled
//! slot. Freed slots remain reusable until the owning slab is dropped.

use std::alloc::Layout;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

mod registry;
use registry::{ChunkGuard, find_chunk_for_addr, register_chunk, unregister_chunk};
mod generation;
pub(crate) use generation::advance_generation;

const CHUNK_TARGET: usize = 1 << 16;

/// Whether `val` is the live body of some slab slot. Sound for arbitrary
/// words: non-chunk addresses and mid-slot pointers classify false, and
/// zeroed chunks make never-allocated slots read as dead.
pub fn ptr_is_slab_body(val: i64) -> bool {
    slot_generation(val) & 1 == 1
}

/// Whether `val` addresses a slot inside a live chunk, regardless of the
/// slot's own generation. A freed-but-not-recycled slot still lands here;
/// literals and foreign pointers do not. Callers that must tolerate `.rodata`
/// string literals use this to gate a header read the classifier would reject.
pub fn ptr_in_slab_span(val: i64) -> bool {
    chunk_for_valid_slot(val).is_some()
}

fn chunk_for_valid_slot(val: i64) -> Option<ChunkGuard> {
    if val <= 0 || val & 7 != 0 {
        return None;
    }
    let addr = val as usize;
    let c = find_chunk_for_addr(addr)?;
    if (addr - c.start) % c.slot_bytes != 16 {
        return None;
    }
    Some(c)
}

/// Combines `ptr_in_slab_span` and `chunk_is_global` into the one chunk
/// lookup both need, for a free path that would otherwise classify the same
/// address twice back to back: `None` for a literal or foreign pointer,
/// `Some(is_global)` for a live slot.
pub fn slab_membership(val: i64) -> Option<bool> {
    chunk_for_valid_slot(val).map(|c| c.is_global)
}

pub struct GenSlab {
    free_head: *mut u64,
    bump: *mut u8,
    bump_end: *mut u8,
    bump_generation: u64,
    slot_bytes: usize,
    chunks: Vec<(*mut u8, Layout)>,
    cleanup: Option<unsafe fn(*mut u8)>,
    is_global: bool,
}

unsafe impl Send for GenSlab {}

impl Drop for GenSlab {
    fn drop(&mut self) {
        for &(chunk, layout) in &self.chunks {
            unregister_chunk(chunk as usize);
            unsafe {
                if let Some(cleanup) = self.cleanup {
                    for offset in (0..layout.size()).step_by(self.slot_bytes) {
                        let slot = chunk.add(offset);
                        let generation =
                            (*(slot.add(8) as *const AtomicU64)).load(Ordering::Relaxed);
                        if generation != 0 && (!cfg!(debug_assertions) || generation & 1 != 0) {
                            cleanup(slot.add(16));
                        }
                    }
                }
                std::alloc::dealloc(chunk, layout);
            }
        }
    }
}

impl GenSlab {
    pub const fn new(body_bytes: usize) -> Self {
        let body = body_bytes
            .checked_add(7)
            .expect("olive: slab body size overflow")
            & !7;
        // Every free slot stores a pointer in its body, including empty objects.
        let body = if body < 8 { 8 } else { body };
        let slot_bytes = body
            .checked_add(16)
            .expect("olive: slab slot size overflow");
        Self {
            free_head: std::ptr::null_mut(),
            bump: std::ptr::null_mut(),
            bump_end: std::ptr::null_mut(),
            bump_generation: 0,
            slot_bytes,
            chunks: Vec::new(),
            cleanup: None,
            is_global: false,
        }
    }

    /// The callback releases backing storage for initialized slots at teardown.
    /// In release builds it must also accept freed slots whose first word holds
    /// the free-list link. Debug frees poison those slots and skip the callback.
    pub(crate) const fn with_cleanup(body_bytes: usize, cleanup: unsafe fn(*mut u8)) -> Self {
        let mut slab = Self::new(body_bytes);
        slab.cleanup = Some(cleanup);
        slab
    }

    pub(crate) const fn with_global(mut self, is_global: bool) -> Self {
        self.is_global = is_global;
        self
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
                let g = advance_generation((*gen_ptr).load(Ordering::Relaxed), 1);
                (*gen_ptr).store(g, Ordering::Release);
                return (body, false);
            }
        }
        if self.bump == self.bump_end {
            self.grow();
        }
        unsafe {
            let gen_ptr = self.bump.add(8) as *mut AtomicU64;
            self.bump = self.bump.add(self.slot_bytes);
            (*gen_ptr).store(self.bump_generation, Ordering::Release);
            let body = (gen_ptr as *mut u8).add(8);
            (body, true)
        }
    }

    fn grow(&mut self) {
        self.bump_generation = generation::fresh_generation();
        let slots = (CHUNK_TARGET / self.slot_bytes).max(1);
        let bytes = slots * self.slot_bytes;
        let layout = Layout::from_size_align(bytes, 8).unwrap();
        // Allocate without zeroing, manually zeroing only the generation words.
        let chunk = unsafe { std::alloc::alloc(layout) };
        assert!(!chunk.is_null(), "olive: slab chunk allocation failed");

        // Zero only the generation word (body - 8) of each slot so un-bumped slots read as dead.
        let mut ptr = chunk;
        for _ in 0..slots {
            unsafe {
                let gen_ptr = ptr.add(8) as *mut AtomicU64;
                (*gen_ptr).store(0, Ordering::Relaxed);
                ptr = ptr.add(self.slot_bytes);
            }
        }

        register_chunk(
            chunk as usize,
            chunk as usize + bytes,
            self.slot_bytes,
            self.is_global,
        );
        self.bump = chunk;
        self.bump_end = unsafe { chunk.add(bytes) };
        self.chunks.push((chunk, layout));
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
            (*gen_ptr).store(advance_generation(generation, 1), Ordering::Release);
            *(body as *mut *mut u64) = self.free_head;
            self.free_head = (body as *mut u64).sub(2);
            #[cfg(debug_assertions)]
            {
                let body_size = self.slot_bytes - 16;
                if body_size > 8 {
                    std::ptr::write_bytes(body.add(8), 0x5a, body_size - 8);
                }
            }
            true
        }
    }

    /// Whether this specific GenSlab instance allocated the chunk containing `addr`.
    #[inline]
    pub fn owns_addr(&self, addr: usize) -> bool {
        self.chunks.iter().any(|(chunk, layout)| {
            let start = *chunk as usize;
            let end = start + layout.size();
            addr >= start && addr < end
        })
    }
}

/// Whether a slab slot is currently live.
#[inline]
pub fn slot_is_live(body: i64) -> bool {
    ptr_is_slab_body(body)
}

/// Current generation word of a slab slot.
#[inline]
pub fn slot_generation(body: i64) -> u64 {
    let Some(_chunk) = chunk_for_valid_slot(body) else {
        return 0;
    };
    // Keep the registry read guard alive until the header read completes.
    unsafe { (*(body as *const AtomicU64).sub(1)).load(Ordering::Relaxed) }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod lifecycle_tests;

thread_local! {
    pub(crate) static ACTIVE_SLABS: std::cell::Cell<*mut SlabSet> = const { std::cell::Cell::new(std::ptr::null_mut()) };
    pub(crate) static SOURCE_SLABS: std::cell::Cell<*mut SlabSet> = const { std::cell::Cell::new(std::ptr::null_mut()) };
    static ACTIVE_GLOBAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[inline]
pub(crate) fn active_slab_is_global() -> bool {
    ACTIVE_GLOBAL.get()
}

pub struct SlabSet {
    pub(crate) is_global: bool,
    pub list: GenSlab,
    pub obj: GenSlab,
    pub set: GenSlab,
    pub enum_slab: GenSlab,
    pub boxed: GenSlab,
    pub bytes: GenSlab,
    pub result: GenSlab,
    pub iter: GenSlab,
    pub str_slabs: [Option<GenSlab>; 32],
    pub struct_slabs: crate::struct_obj::StructSlabs,
    pub struct_box: GenSlab,
}

impl Default for SlabSet {
    fn default() -> Self {
        Self::new()
    }
}

impl SlabSet {
    pub fn new() -> Self {
        Self::with_global(false)
    }

    fn with_global(is_global: bool) -> Self {
        Self {
            is_global,
            list: GenSlab::with_cleanup(
                std::mem::size_of::<crate::StableVec>(),
                crate::list::release_list_storage,
            )
            .with_global(is_global),
            obj: GenSlab::with_cleanup(
                std::mem::size_of::<crate::OliveObj>(),
                crate::obj::release_obj_storage,
            )
            .with_global(is_global),
            set: GenSlab::with_cleanup(
                std::mem::size_of::<crate::OliveHashSet>(),
                crate::set::release_set_storage,
            )
            .with_global(is_global),
            enum_slab: GenSlab::with_cleanup(
                std::mem::size_of::<crate::OliveEnum>(),
                crate::enum_obj::release_enum_storage,
            )
            .with_global(is_global),
            boxed: GenSlab::new(std::mem::size_of::<crate::boxed::OliveBoxed>())
                .with_global(is_global),
            bytes: GenSlab::with_cleanup(
                std::mem::size_of::<crate::bytes::OliveBytes>(),
                crate::bytes::release_bytes_storage,
            )
            .with_global(is_global),
            result: GenSlab::new(std::mem::size_of::<crate::result::OliveResult>())
                .with_global(is_global),
            iter: GenSlab::new(std::mem::size_of::<crate::list::OliveIter>())
                .with_global(is_global),
            str_slabs: std::array::from_fn(|_| None),
            struct_slabs: crate::struct_obj::StructSlabs::with_global(is_global),
            struct_box: GenSlab::new(std::mem::size_of::<crate::struct_box::OliveStructBox>())
                .with_global(is_global),
        }
    }
}

/// Process-lifetime arena for values crossing a task/thread boundary; never torn down.
static GLOBAL_SLABS: std::sync::LazyLock<Mutex<SlabSet>> =
    std::sync::LazyLock::new(|| Mutex::new(SlabSet::with_global(true)));

/// Whether addr's chunk is GLOBAL_SLABS; its frees must route through that lock.
pub fn chunk_is_global(addr: usize) -> bool {
    find_chunk_for_addr(addr)
        .map(|c| c.is_global)
        .unwrap_or(false)
}

pub(crate) fn global_struct_box_owns_addr(addr: usize) -> bool {
    GLOBAL_SLABS.lock().unwrap().struct_box.owns_addr(addr)
}

pub(crate) fn global_obj_owns_addr(addr: usize) -> bool {
    GLOBAL_SLABS.lock().unwrap().obj.owns_addr(addr)
}

pub(crate) fn global_list_owns_addr(addr: usize) -> bool {
    GLOBAL_SLABS.lock().unwrap().list.owns_addr(addr)
}

pub(crate) fn global_bytes_owns_addr(addr: usize) -> bool {
    GLOBAL_SLABS.lock().unwrap().bytes.owns_addr(addr)
}

pub(crate) fn global_enum_owns_addr(addr: usize) -> bool {
    GLOBAL_SLABS.lock().unwrap().enum_slab.owns_addr(addr)
}

pub(crate) fn global_set_owns_addr(addr: usize) -> bool {
    GLOBAL_SLABS.lock().unwrap().set.owns_addr(addr)
}

pub(crate) fn global_boxed_owns_addr(addr: usize) -> bool {
    GLOBAL_SLABS.lock().unwrap().boxed.owns_addr(addr)
}

pub(crate) fn global_result_owns_addr(addr: usize) -> bool {
    GLOBAL_SLABS.lock().unwrap().result.owns_addr(addr)
}

pub(crate) fn global_iter_owns_addr(addr: usize) -> bool {
    GLOBAL_SLABS.lock().unwrap().iter.owns_addr(addr)
}

pub(crate) fn global_struct_raw_owns_addr(addr: usize) -> bool {
    GLOBAL_SLABS.lock().unwrap().struct_slabs.owns_addr(addr)
}

/// Redirects ACTIVE_SLABS to the locked global arena for the duration of `f`.
/// The guard is a MutexGuard held across the call, so an unwinding `f` still
/// unlocks; the restore closure keeps the redirect itself from leaking on
/// unwind (release builds abort, but tests and debug builds unwind).
pub fn with_escape_arena<T>(f: impl FnOnce() -> T) -> T {
    let mut guard = GLOBAL_SLABS.lock().unwrap();
    let slabs_ptr = &mut *guard as *mut SlabSet;
    let old = ACTIVE_SLABS.get();
    let old_source = SOURCE_SLABS.get();
    let source = if old.is_null() { old_source } else { old };
    let old_global = ACTIVE_GLOBAL.replace(true);
    ACTIVE_SLABS.set(slabs_ptr);
    SOURCE_SLABS.set(source);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    ACTIVE_SLABS.set(old);
    SOURCE_SLABS.set(old_source);
    ACTIVE_GLOBAL.set(old_global);
    drop(guard);
    match result {
        Ok(v) => v,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
