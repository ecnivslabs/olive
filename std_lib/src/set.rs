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
    let inner = Box::into_raw(Box::new(FxHashSet::<i64>::default()));
    SET_SLAB.with(|sl| {
        let sl = unsafe { &mut *sl.get() };
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
    })
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
    SET_SLAB.with(|sl| {
        unsafe { &mut *sl.get() }.free(ptr as *mut u8);
    });
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
        if hs.insert(val) {
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
    hs.contains(&val) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_remove(set_ptr: i64, val: i64) -> i64 {
    if set_ptr == 0 {
        return 0;
    }
    unsafe {
        let s = &mut *(set_ptr as *mut OliveHashSet);
        let hs = &mut *s.inner;
        if hs.remove(&val) {
            let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
            if let Some(pos) = v.iter().position(|&x| x == val) {
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
        assert!(unsafe { (*s.inner).contains(&42) });
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
            assert!(unsafe { (*s.inner).contains(&i) });
        }
    }

    #[test]
    fn set_add_null_no_panic() {
        olive_set_add(0, 42);
    }
}
