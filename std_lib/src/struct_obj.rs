use crate::slab::GenSlab;
use std::cell::UnsafeCell;

// Body words = field count word + n_fields; sizes above this use pow2 classes.
const FIXED_MAX_WORDS: usize = 17;

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
    if ptr == 0 || !crate::slab::ptr_in_slab_span(ptr) {
        return;
    }
    let n_fields = unsafe { *(ptr as *const i64) };
    free_struct_slot_raw(ptr, n_fields);
}

pub(crate) fn free_struct_slot_raw(ptr: i64, n_fields: i64) {
    if crate::slab::chunk_is_global(ptr as usize) {
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
