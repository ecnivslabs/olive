use crate::slab::GenSlab;
use std::cell::UnsafeCell;

// Body words = field count word + n_fields; sizes above this use pow2 classes.
const FIXED_MAX_WORDS: usize = 17;

/// Trait-object record: [kind, data ptr, vtable ptr, drop shim ptr, concrete
/// descriptor ptr]. The descriptor is the erased concrete struct's, kept so
/// the copy path can deep-copy the value behind `data` the same way the drop
/// shim deep-frees it; without it a trait object would share one allocation
/// across owners and double-free.
pub(crate) const KIND_FATPTR: i64 = 17;
const FATPTR_WORDS: usize = 5;

pub struct StructSlabs {
    fixed: Vec<GenSlab>,
    large: Vec<(usize, GenSlab)>,
}

impl StructSlabs {
    pub fn new() -> Self {
        Self {
            fixed: (0..=FIXED_MAX_WORDS).map(|w| GenSlab::new(w * 8)).collect(),
            large: Vec::new(),
        }
    }

    fn class_for(&mut self, body_words: usize) -> &mut GenSlab {
        if body_words <= FIXED_MAX_WORDS {
            return &mut self.fixed[body_words];
        }
        let class = body_words.next_power_of_two();
        if let Some(i) = self.large.iter().position(|(w, _)| *w == class) {
            return &mut self.large[i].1;
        }
        self.large.push((class, GenSlab::new(class * 8)));
        &mut self.large.last_mut().unwrap().1
    }
}

impl Default for StructSlabs {
    fn default() -> Self {
        Self::new()
    }
}

thread_local! {
    static STRUCT_SLABS: UnsafeCell<StructSlabs> = UnsafeCell::new(StructSlabs::new());
}

/// A value crossing a task boundary (E5.6) is relocated into the shared
/// escape arena (`with_escape_arena`), which redirects `ACTIVE_SLABS` --
/// struct allocation must consult it the same way `list`/`obj`/`set`/`enum`
/// already do (`alloc_list_header` et al.), or a "relocated" struct or
/// closure record still lands in the sending thread's own thread-local
/// pool and dangles once that thread/task tears its pool down.
#[unsafe(no_mangle)]
pub extern "C" fn olive_struct_alloc(n_fields: i64) -> i64 {
    let words = n_fields as usize + 1;
    let active = crate::slab::ACTIVE_SLABS.get();
    let body = if !active.is_null() {
        unsafe { (*active).struct_slabs.class_for(words).alloc().0 }
    } else {
        STRUCT_SLABS.with(|s| unsafe { (&mut *s.get()).class_for(words).alloc().0 })
    };
    unsafe { *(body as *mut i64) = n_fields };
    body as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_struct(ptr: i64) {
    if ptr == 0 {
        return;
    }
    let Some(is_global) = crate::slab::slab_membership(ptr) else {
        return;
    };
    let n_fields = unsafe { *(ptr as *const i64) };
    free_struct_slot_raw_with(ptr, n_fields, Some(is_global));
}

pub(crate) fn free_struct_slot_raw(ptr: i64, n_fields: i64) {
    free_struct_slot_raw_with(ptr, n_fields, None);
}

/// `known_global` skips the chunk lookup when the caller already classified
/// `ptr` a moment ago (e.g. `olive_free_struct`'s own span check).
pub(crate) fn free_struct_slot_raw_with(ptr: i64, n_fields: i64, known_global: Option<bool>) {
    let is_global = known_global.unwrap_or_else(|| crate::slab::chunk_is_global(ptr as usize));
    if is_global {
        crate::slab::with_escape_arena(|| free_struct_slot_raw_local(ptr, n_fields));
    } else {
        free_struct_slot_raw_local(ptr, n_fields);
    }
}

fn free_struct_slot_raw_local(ptr: i64, n_fields: i64) {
    let words = n_fields as usize + 1;
    let active = crate::slab::ACTIVE_SLABS.get();
    if !active.is_null() {
        unsafe { (*active).struct_slabs.class_for(words).free(ptr as *mut u8) };
    } else {
        STRUCT_SLABS.with(|s| {
            let s = unsafe { &mut *s.get() };
            s.class_for(words).free(ptr as *mut u8);
        });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_fatptr_alloc() -> i64 {
    let active = crate::slab::ACTIVE_SLABS.get();
    let body = if !active.is_null() {
        unsafe { (*active).struct_slabs.class_for(FATPTR_WORDS).alloc().0 }
    } else {
        STRUCT_SLABS.with(|s| unsafe { (&mut *s.get()).class_for(FATPTR_WORDS).alloc().0 })
    };
    unsafe { *(body as *mut i64) = KIND_FATPTR };
    body as i64
}

/// The concrete descriptor a trait-object record was built with (word 4),
/// with the string tag bits stripped so it reads as a raw descriptor pointer.
pub(crate) fn fatptr_desc(ptr: i64) -> i64 {
    unsafe {
        *(ptr as *const i64).add(4) & !(crate::string_slab::STR_TAG | crate::string_slab::STR_HEAP)
    }
}

/// Builds a trait-object record from an already-owned concrete `data` and the
/// erased type's vtable, drop shim and descriptor. Used by the copy path so a
/// duplicated trait object owns its own concrete allocation.
pub(crate) fn fatptr_new(data: i64, vtable: i64, drop_shim: i64, desc_word: i64) -> i64 {
    let ptr = olive_fatptr_alloc();
    unsafe {
        let words = ptr as *mut i64;
        *words.add(1) = data;
        *words.add(2) = vtable;
        *words.add(3) = drop_shim;
        *words.add(4) = desc_word;
    }
    ptr
}

/// Patches the data word after the record is already registered in the
/// copy path's `visited` map, so cyclic trait objects resolve instead of
/// recursing forever.
pub(crate) fn fatptr_set_data(ptr: i64, data: i64) {
    unsafe { *(ptr as *mut i64).add(1) = data };
}

/// The raw words a copy needs to rebuild the record: (data, vtable, drop_shim,
/// desc word as stored). `desc_word` keeps its tag bits so the rebuilt record
/// is byte-identical to the original.
pub(crate) fn fatptr_fields(ptr: i64) -> (i64, i64, i64, i64) {
    unsafe {
        let words = ptr as *const i64;
        (*words.add(1), *words.add(2), *words.add(3), *words.add(4))
    }
}

/// Frees the concrete value through the record's drop shim, then the record.
#[unsafe(no_mangle)]
pub extern "C" fn olive_free_fatptr(ptr: i64) {
    if ptr == 0 {
        return;
    }
    let Some(is_global) = crate::slab::slab_membership(ptr) else {
        return;
    };
    if !crate::slab::slot_is_live(ptr) {
        return;
    }
    unsafe {
        let words = ptr as *const i64;
        if *words != KIND_FATPTR {
            return;
        }
        let data = *words.add(1);
        let shim = *words.add(3);
        if shim != 0 && data != 0 {
            let shim_fn: extern "C" fn(i64) -> i64 = std::mem::transmute(shim as usize);
            shim_fn(data);
        }
    }
    free_struct_slot_raw_with(ptr, FATPTR_WORDS as i64 - 1, Some(is_global));
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_struct_gen_of(ptr: i64) -> i64 {
    if ptr == 0 || !crate::slab::ptr_in_slab_span(ptr) {
        return 0;
    }
    unsafe { *((ptr - 8) as *const i64) }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_struct_gen_stale(ptr: i64, generation: i64) -> i64 {
    if ptr == 0 || generation == 0 || !crate::slab::ptr_in_slab_span(ptr) {
        return 0;
    }
    let cur = unsafe { *((ptr - 8) as *const i64) };
    (((cur ^ generation) << 1) != 0 || cur & 1 == 0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_small_struct() {
        let ptr = olive_struct_alloc(1);
        assert_ne!(ptr, 0);
        let n = unsafe { *(ptr as *const i64) };
        assert_eq!(n, 1);
        olive_free_struct(ptr);
    }

    #[test]
    fn alloc_large_struct() {
        let ptr = olive_struct_alloc(100);
        assert_ne!(ptr, 0);
        let n = unsafe { *(ptr as *const i64) };
        assert_eq!(n, 100);
        olive_free_struct(ptr);
    }

    #[test]
    fn alloc_and_write_fields() {
        let ptr = olive_struct_alloc(3);
        assert_ne!(ptr, 0);
        unsafe {
            let fields = (ptr + 8) as *mut i64;
            *fields = 10;
            *fields.add(1) = 20;
            *fields.add(2) = 30;
            assert_eq!(*fields, 10);
            assert_eq!(*fields.add(1), 20);
            assert_eq!(*fields.add(2), 30);
        }
        olive_free_struct(ptr);
    }

    #[test]
    fn free_null_no_panic() {
        olive_free_struct(0);
    }

    #[test]
    fn repeated_alloc_free() {
        for _ in 0..10 {
            let ptr = olive_struct_alloc(2);
            assert_ne!(ptr, 0);
            olive_free_struct(ptr);
        }
    }

    #[test]
    fn double_free_absorbed() {
        let ptr = olive_struct_alloc(2);
        olive_free_struct(ptr);
        olive_free_struct(ptr);
        let ptr2 = olive_struct_alloc(2);
        assert_eq!(ptr, ptr2);
        olive_free_struct(ptr2);
    }

    #[test]
    fn empty_struct_alloc() {
        let ptr = olive_struct_alloc(0);
        assert_ne!(ptr, 0);
        assert_eq!(unsafe { *(ptr as *const i64) }, 0);
        olive_free_struct(ptr);
    }

    #[test]
    fn fatptr_alloc_writes_kind() {
        let ptr = olive_fatptr_alloc();
        assert_ne!(ptr, 0);
        assert_eq!(unsafe { *(ptr as *const i64) }, KIND_FATPTR);
        unsafe {
            *((ptr + 8) as *mut i64) = 0;
            *((ptr + 24) as *mut i64) = 0;
        }
        olive_free_fatptr(ptr);
    }

    #[test]
    fn fatptr_free_recycles_and_absorbs_double_free() {
        let a = olive_fatptr_alloc();
        unsafe {
            *((a + 8) as *mut i64) = 0;
            *((a + 24) as *mut i64) = 0;
        }
        olive_free_fatptr(a);
        olive_free_fatptr(a);
        let b = olive_fatptr_alloc();
        assert_eq!(a, b, "slot recycles after free");
        unsafe {
            *((b + 8) as *mut i64) = 0;
            *((b + 24) as *mut i64) = 0;
        }
        olive_free_fatptr(b);
    }

    extern "C" fn count_shim(data: i64) -> i64 {
        unsafe { *(data as *mut i64) += 1 };
        0
    }

    #[test]
    fn fatptr_free_runs_drop_shim() {
        let mut hits: i64 = 0;
        let ptr = olive_fatptr_alloc();
        unsafe {
            *((ptr + 8) as *mut i64) = &mut hits as *mut i64 as i64;
            *((ptr + 16) as *mut i64) = 0;
            *((ptr + 24) as *mut i64) = count_shim as extern "C" fn(i64) -> i64 as usize as i64;
        }
        olive_free_fatptr(ptr);
        assert_eq!(hits, 1, "shim runs exactly once");
    }

    #[test]
    fn free_any_classifies_fatptr() {
        let mut hits: i64 = 0;
        let ptr = olive_fatptr_alloc();
        unsafe {
            *((ptr + 8) as *mut i64) = &mut hits as *mut i64 as i64;
            *((ptr + 16) as *mut i64) = 0;
            *((ptr + 24) as *mut i64) = count_shim as extern "C" fn(i64) -> i64 as usize as i64;
        }
        crate::olive_free_any(ptr);
        assert_eq!(hits, 1, "free_any dispatches by kind word");
    }

    #[test]
    fn struct_generation_check() {
        let ptr = olive_struct_alloc(1);
        assert_ne!(ptr, 0);
        let generation = olive_struct_gen_of(ptr);
        assert_ne!(generation, 0);
        assert_eq!(olive_struct_gen_stale(ptr, generation), 0);

        olive_free_struct(ptr);
        assert_eq!(olive_struct_gen_stale(ptr, generation), 1);
    }
}
