use crate::slab::GenSlab;
use crate::*;
use std::cell::UnsafeCell;

use rustc_hash::FxHashSet;

thread_local! {
    static SET_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::new(std::mem::size_of::<OliveHashSet>())) };
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_new(capacity: i64) -> i64 {
    let cap = capacity as usize;
    let mut v: Vec<i64> = Vec::with_capacity(cap);
    let ptr = v.as_mut_ptr();
    let v_cap = v.capacity();
    std::mem::forget(v);
    let inner = Box::into_raw(Box::new(FxHashSet::<OliveStringKey>::default()));
    let slab_alloc = |sl: &mut GenSlab| {
        let (body, _) = sl.alloc();
        unsafe {
            std::ptr::write(
                body as *mut OliveHashSet,
                OliveHashSet {
                    kind: KIND_SET,
                    ptr,
                    cap: v_cap,
                    len: 0,
                    inner,
                },
            );
        }
        body as i64
    };
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            slab_alloc(&mut (*active).set)
        } else {
            SET_SLAB.with(|sl| slab_alloc(&mut *sl.get()))
        }
    }
}

pub(crate) fn olive_free_set(ptr: i64) {
    if ptr == 0 || !crate::slab::ptr_in_slab_span(ptr) {
        return;
    }
    if crate::slab::slot_is_live(ptr) {
        unsafe { release_set_storage(ptr) };
    }
    free_set_slot_raw(ptr);
}

/// Drops a set's element vector and inner hash set; the slot body persists
/// after a slab free, so this is safe in either order.
pub(crate) unsafe fn release_set_storage(ptr: i64) {
    unsafe {
        let s = &mut *(ptr as *mut OliveHashSet);
        if !s.ptr.is_null() {
            let _ = Vec::from_raw_parts(s.ptr, s.len, s.cap);
            s.ptr = std::ptr::null_mut();
        }
        if !s.inner.is_null() {
            let _ = Box::from_raw(s.inner);
            s.inner = std::ptr::null_mut();
        }
    }
}

pub(crate) fn free_set_slot_raw(ptr: i64) {
    if crate::slab::chunk_is_global(ptr as usize) {
        crate::slab::with_escape_arena(|| free_set_slot_raw_local(ptr));
    } else {
        free_set_slot_raw_local(ptr);
    }
}

fn free_set_slot_raw_local(ptr: i64) {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            (*active).set.free(ptr as *mut u8);
        } else {
            SET_SLAB.with(|sl| {
                (&mut *sl.get()).free(ptr as *mut u8);
            });
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_new_reuse(old_ptr: i64, capacity: i64, bump: i64) -> i64 {
    if old_ptr == 0 {
        return olive_set_new(capacity);
    }
    if bump != 0 {
        unsafe {
            let gen_ptr = (old_ptr as *mut std::sync::atomic::AtomicU64).sub(1);
            let g = (*gen_ptr).load(std::sync::atomic::Ordering::Relaxed) + 2;
            (*gen_ptr).store(g, std::sync::atomic::Ordering::Release);
        }
    }
    let s = unsafe { &mut *(old_ptr as *mut OliveHashSet) };
    let cap = capacity as usize;
    unsafe {
        if s.ptr.is_null() || s.cap < cap {
            let mut v = if s.ptr.is_null() {
                Vec::with_capacity(cap)
            } else {
                Vec::from_raw_parts(s.ptr, 0, s.cap)
            };
            v.reserve(cap);
            s.ptr = v.as_mut_ptr();
            s.cap = v.capacity();
            std::mem::forget(v);
        }
        if s.inner.is_null() {
            s.inner = Box::into_raw(Box::new(FxHashSet::<OliveStringKey>::default()));
        }
        s.len = 0;
    }
    old_ptr
}

/// Snapshots a set's elements into a list, backing `for x in some_set`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_items(set_ptr: i64) -> i64 {
    if set_ptr == 0 {
        return crate::list::olive_list_new(0);
    }
    let s = unsafe { &*(set_ptr as *const OliveHashSet) };
    let list = crate::list::olive_list_new(s.len as i64);
    for i in 0..s.len {
        let val = unsafe { *s.ptr.add(i) };
        crate::list::olive_list_set(list, i as i64, val);
    }
    list
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_add(set_ptr: i64, val: i64) {
    if set_ptr == 0 {
        return;
    }
    unsafe {
        let s = &mut *(set_ptr as *mut OliveHashSet);
        let hs = &mut *s.inner;
        if hs.insert(OliveStringKey(val)) {
            let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
            v.push(val);
            s.ptr = v.as_mut_ptr();
            s.cap = v.capacity();
            s.len = v.len();
            std::mem::forget(v);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_contains(set_ptr: i64, val: i64) -> i64 {
    if set_ptr == 0 {
        return 0;
    }
    let s = unsafe { &*(set_ptr as *const OliveHashSet) };
    let hs = unsafe { &*s.inner };
    hs.contains(&OliveStringKey(val)) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_remove(set_ptr: i64, val: i64) -> i64 {
    if set_ptr == 0 {
        return 0;
    }
    unsafe {
        let s = &mut *(set_ptr as *mut OliveHashSet);
        let hs = &mut *s.inner;
        if hs.remove(&OliveStringKey(val)) {
            let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
            // Structural removal (a distinct-but-equal pointer) must find
            // the same element here that `hs.remove` just found, not its
            // own raw-pointer match -- see `OliveStringKey`'s `PartialEq`.
            if let Some(pos) = v
                .iter()
                .position(|&x| OliveStringKey(x) == OliveStringKey(val))
            {
                v.remove(pos);
            }
            s.ptr = v.as_mut_ptr();
            s.cap = v.capacity();
            s.len = v.len();
            std::mem::forget(v);
        }
    }
    val
}

/// `s.remove(x)`: faults if `x` is absent (Python semantics). `discard`
/// keeps `olive_set_remove`'s existing silent-on-absence behavior.
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_remove_checked(set_ptr: i64, val: i64, loc: i64) -> i64 {
    if set_ptr == 0 {
        crate::panic::olive_bounds_fail(0, 0, loc);
        return 0;
    }
    let present = unsafe {
        let s = &*(set_ptr as *const OliveHashSet);
        (*s.inner).contains(&OliveStringKey(val))
    };
    if !present {
        let len = unsafe { (*(set_ptr as *const OliveHashSet)).len as i64 };
        crate::panic::olive_bounds_fail(0, len, loc);
        return 0;
    }
    olive_set_remove(set_ptr, val)
}

/// `s.clear()`: empties the set in place, returns it.
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_clear(set_ptr: i64) -> i64 {
    if set_ptr == 0 {
        return set_ptr;
    }
    unsafe {
        let s = &mut *(set_ptr as *mut OliveHashSet);
        (*s.inner).clear();
        s.len = 0;
    }
    set_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_union(a: i64, b: i64) -> i64 {
    if a == 0 {
        return olive_set_items(b);
    }
    if b == 0 {
        return olive_set_items(a);
    }
    let sa = unsafe { &*(a as *const OliveHashSet) };
    let sb = unsafe { &*(b as *const OliveHashSet) };
    let result = olive_set_new((sa.len + sb.len) as i64);
    for i in 0..sa.len {
        let val = unsafe { *sa.ptr.add(i) };
        olive_set_add(result, val);
    }
    for i in 0..sb.len {
        let val = unsafe { *sb.ptr.add(i) };
        olive_set_add(result, val);
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_intersection(a: i64, b: i64) -> i64 {
    if a == 0 || b == 0 {
        return olive_set_new(0);
    }
    let sa = unsafe { &*(a as *const OliveHashSet) };
    let sb = unsafe { &*(b as *const OliveHashSet) };
    let result = olive_set_new(sa.len.min(sb.len) as i64);
    for i in 0..sa.len {
        let val = unsafe { *sa.ptr.add(i) };
        if unsafe { (*sb.inner).contains(&OliveStringKey(val)) } {
            olive_set_add(result, val);
        }
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_diff(a: i64, b: i64) -> i64 {
    if a == 0 {
        return olive_set_new(0);
    }
    if b == 0 {
        return olive_set_items(a);
    }
    let sa = unsafe { &*(a as *const OliveHashSet) };
    let sb = unsafe { &*(b as *const OliveHashSet) };
    let result = olive_set_new(sa.len as i64);
    for i in 0..sa.len {
        let val = unsafe { *sa.ptr.add(i) };
        if !unsafe { (*sb.inner).contains(&OliveStringKey(val)) } {
            olive_set_add(result, val);
        }
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_sym_diff(a: i64, b: i64) -> i64 {
    if a == 0 {
        return olive_set_items(b);
    }
    if b == 0 {
        return olive_set_items(a);
    }
    let sa = unsafe { &*(a as *const OliveHashSet) };
    let sb = unsafe { &*(b as *const OliveHashSet) };
    let result = olive_set_new((sa.len + sb.len) as i64);
    for i in 0..sa.len {
        let val = unsafe { *sa.ptr.add(i) };
        if !unsafe { (*sb.inner).contains(&OliveStringKey(val)) } {
            olive_set_add(result, val);
        }
    }
    for i in 0..sb.len {
        let val = unsafe { *sb.ptr.add(i) };
        if !unsafe { (*sa.inner).contains(&OliveStringKey(val)) } {
            olive_set_add(result, val);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_set() -> i64 {
        olive_set_new(8)
    }

    #[test]
    fn new_set_creates_empty() {
        let ptr = new_set();
        assert_ne!(ptr, 0);
        let s = unsafe { &*(ptr as *const OliveHashSet) };
        assert_eq!(s.len, 0);
    }

    #[test]
    fn add_single_element() {
        let ptr = new_set();
        olive_set_add(ptr, 42);
        let s = unsafe { &*(ptr as *const OliveHashSet) };
        assert_eq!(s.len, 1);
        assert!(unsafe { (*s.inner).contains(&OliveStringKey(42)) });
    }

    #[test]
    fn add_duplicate_no_change() {
        let ptr = new_set();
        olive_set_add(ptr, 1);
        olive_set_add(ptr, 1);
        let s = unsafe { &*(ptr as *const OliveHashSet) };
        assert_eq!(s.len, 1);
    }

    #[test]
    fn add_multiple_elements() {
        let ptr = new_set();
        for i in 0..10 {
            olive_set_add(ptr, i);
        }
        let s = unsafe { &*(ptr as *const OliveHashSet) };
        assert_eq!(s.len, 10);
        for i in 0..10 {
            assert!(unsafe { (*s.inner).contains(&OliveStringKey(i)) });
        }
    }

    #[test]
    fn set_add_null_no_panic() {
        olive_set_add(0, 42);
    }
}
