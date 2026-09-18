use crate::slab::GenSlab;
use crate::*;
use std::cell::UnsafeCell;

use rustc_hash::FxHashSet;

thread_local! {
    static SET_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::with_cleanup(std::mem::size_of::<OliveHashSet>(), release_set_storage)) };
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_new(capacity: i64) -> i64 {
    // A negative capacity wraps to a huge usize; clamp to the empty vector
    // instead of tripping a capacity overflow.
    let cap = capacity.max(0) as usize;
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

#[inline]
fn free_set_elem(val: i64) {
    if crate::is_tagged_str_key(val) {
        crate::olive_free_str(val);
    } else if crate::is_active_object(val) {
        crate::olive_free_any(val);
    }
}

/// Adds an owned word to a result set under construction, releasing it when
/// an equal element is already present. Internal set combinators pass owned
/// copies, so a rejected copy must not leak.
fn set_add_owned(set_ptr: i64, val: i64) {
    let present = unsafe {
        let s = &*(set_ptr as *const OliveHashSet);
        (*s.inner).contains(&OliveStringKey(val))
    };
    if present {
        free_set_elem(val);
    } else {
        olive_set_add(set_ptr, val);
    }
}

pub(crate) fn olive_free_set(ptr: i64) {
    if ptr == 0 {
        return;
    }
    let Some(is_global) = crate::slab::slab_membership(ptr) else {
        return;
    };
    if crate::slab::slot_is_live(ptr) {
        unsafe {
            let s = &mut *(ptr as *mut OliveHashSet);
            for i in 0..s.len {
                free_set_elem(*s.ptr.add(i));
            }
            release_set_storage(ptr as *mut u8)
        };
    }
    free_set_slot_raw_with(ptr, Some(is_global));
}

pub(crate) unsafe fn release_set_storage(body: *mut u8) {
    unsafe {
        let s = &mut *(body as *mut OliveHashSet);
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
    free_set_slot_raw_with(ptr, None);
}

/// `known_global` skips the chunk lookup when the caller already classified
/// `ptr` a moment ago (e.g. `olive_free_set`'s own span check).
pub(crate) fn free_set_slot_raw_with(ptr: i64, known_global: Option<bool>) {
    if !crate::slab::slot_is_live(ptr) {
        return;
    }
    unsafe {
        let s = &mut *(ptr as *mut OliveHashSet);
        s.ptr = std::ptr::null_mut();
        s.inner = std::ptr::null_mut();
        s.len = 0;
        s.cap = 0;
    }
    let is_global = known_global.unwrap_or_else(|| crate::slab::chunk_is_global(ptr as usize));
    if is_global {
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
            let g = crate::slab::advance_generation(
                (*gen_ptr).load(std::sync::atomic::Ordering::Relaxed),
                2,
            );
            (*gen_ptr).store(g, std::sync::atomic::Ordering::Release);
        }
    }
    let s = unsafe { &mut *(old_ptr as *mut OliveHashSet) };
    let cap = capacity.max(0) as usize;
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
/// Elements are deep copied so the snapshot owns its words independently of
/// the set. Sharing the raw words would double free once both sides release
/// owned strings through the generic element path.
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_items(set_ptr: i64) -> i64 {
    if set_ptr == 0 {
        return crate::list::olive_list_new(0);
    }
    let s = unsafe { &*(set_ptr as *const OliveHashSet) };
    let list = crate::list::olive_list_new(s.len as i64);
    let mut visited = rustc_hash::FxHashMap::default();
    for i in 0..s.len {
        let val = unsafe { *s.ptr.add(i) };
        let copied = crate::copy_typed::copy_any(val, &mut visited);
        crate::list::olive_list_set(list, i as i64, copied);
    }
    list
}

/// Inserts without taking ownership on duplicate. Returns true when stored.
/// The hash insert runs before the vector push so a structurally equal key
/// never leaves both the old and the new word in the snapshot vector.
pub(crate) fn set_try_add(set_ptr: i64, val: i64) -> bool {
    if set_ptr == 0 {
        return false;
    }
    unsafe {
        let s = &mut *(set_ptr as *mut OliveHashSet);
        if (*s.inner).insert(OliveStringKey(val)) {
            let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
            v.push(val);
            s.ptr = v.as_mut_ptr();
            s.cap = v.capacity();
            s.len = v.len();
            std::mem::forget(v);
            true
        } else {
            false
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_add(set_ptr: i64, val: i64) {
    if set_ptr == 0 {
        // `add` takes ownership; a null set has nowhere to store it.
        free_set_elem(val);
        return;
    }
    if !set_try_add(set_ptr, val) {
        free_set_elem(val);
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
                let stored = v.remove(pos);
                // The set owned `stored`; the caller keeps `val`. They are
                // usually distinct copies of the same value, so release the
                // stored one. When they are the same pointer the ownership
                // returns to the caller untouched.
                if stored != val {
                    free_set_elem(stored);
                }
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
        for i in 0..s.len {
            free_set_elem(*s.ptr.add(i));
        }
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
    let mut visited = rustc_hash::FxHashMap::default();
    for i in 0..sa.len {
        let val = unsafe { *sa.ptr.add(i) };
        let copied = crate::copy_typed::copy_any(val, &mut visited);
        set_add_owned(result, copied);
    }
    for i in 0..sb.len {
        let val = unsafe { *sb.ptr.add(i) };
        let copied = crate::copy_typed::copy_any(val, &mut visited);
        set_add_owned(result, copied);
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
    let mut visited = rustc_hash::FxHashMap::default();
    for i in 0..sa.len {
        let val = unsafe { *sa.ptr.add(i) };
        if unsafe { (*sb.inner).contains(&OliveStringKey(val)) } {
            let copied = crate::copy_typed::copy_any(val, &mut visited);
            set_add_owned(result, copied);
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
    let mut visited = rustc_hash::FxHashMap::default();
    for i in 0..sa.len {
        let val = unsafe { *sa.ptr.add(i) };
        if !unsafe { (*sb.inner).contains(&OliveStringKey(val)) } {
            let copied = crate::copy_typed::copy_any(val, &mut visited);
            set_add_owned(result, copied);
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
    let mut visited = rustc_hash::FxHashMap::default();
    for i in 0..sa.len {
        let val = unsafe { *sa.ptr.add(i) };
        if !unsafe { (*sb.inner).contains(&OliveStringKey(val)) } {
            let copied = crate::copy_typed::copy_any(val, &mut visited);
            set_add_owned(result, copied);
        }
    }
    for i in 0..sb.len {
        let val = unsafe { *sb.ptr.add(i) };
        if !unsafe { (*sa.inner).contains(&OliveStringKey(val)) } {
            let copied = crate::copy_typed::copy_any(val, &mut visited);
            set_add_owned(result, copied);
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

    #[test]
    fn clear_releases_tagged_strings() {
        let set = olive_set_new(4);
        let a = crate::olive_str_internal("alpha");
        let b = crate::olive_str_internal("beta");
        let ga = crate::string_slab::olive_str_gen_of(a);
        let gb = crate::string_slab::olive_str_gen_of(b);
        olive_set_add(set, a);
        olive_set_add(set, b);
        olive_set_clear(set);
        assert_eq!(unsafe { (*(set as *const OliveHashSet)).len }, 0);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, ga), 1);
        assert_eq!(crate::string_slab::olive_str_gen_stale(b, gb), 1);
        olive_free_set(set);
    }

    #[test]
    fn free_releases_tagged_strings() {
        let set = olive_set_new(4);
        let a = crate::olive_str_internal("gamma");
        let ga = crate::string_slab::olive_str_gen_of(a);
        olive_set_add(set, a);
        olive_free_set(set);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, ga), 1);
    }

    #[test]
    fn items_snapshot_owns_copies() {
        let set = olive_set_new(4);
        let a = crate::olive_str_internal("shared");
        let ga = crate::string_slab::olive_str_gen_of(a);
        olive_set_add(set, a);
        let snapshot = olive_set_items(set);
        let copied = crate::list::olive_list_get(snapshot, 0);
        assert_ne!(copied, a);
        assert_eq!(crate::olive_str_from_ptr(copied), "shared");
        let gc = crate::string_slab::olive_str_gen_of(copied);
        olive_free_set(set);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, ga), 1);
        assert_eq!(crate::olive_str_from_ptr(copied), "shared");
        crate::list::olive_free_list(snapshot);
        assert_eq!(crate::string_slab::olive_str_gen_stale(copied, gc), 1);
    }

    #[test]
    fn union_result_owns_copies() {
        let a = olive_set_new(4);
        let b = olive_set_new(4);
        let s1 = crate::olive_str_internal("one");
        let s2 = crate::olive_str_internal("two");
        let g1 = crate::string_slab::olive_str_gen_of(s1);
        let g2 = crate::string_slab::olive_str_gen_of(s2);
        olive_set_add(a, s1);
        olive_set_add(b, s2);
        let u = olive_set_union(a, b);
        assert_eq!(olive_set_contains(u, s1), 1);
        assert_eq!(olive_set_contains(u, s2), 1);
        olive_free_set(a);
        olive_free_set(b);
        assert_eq!(crate::string_slab::olive_str_gen_stale(s1, g1), 1);
        assert_eq!(crate::string_slab::olive_str_gen_stale(s2, g2), 1);
        assert_eq!(unsafe { (*(u as *const OliveHashSet)).len }, 2);
        olive_free_set(u);
    }

    #[test]
    fn duplicate_insert_releases_rejected_string() {
        let set = olive_set_new(4);
        let first = crate::olive_str_internal("dup");
        let second = crate::olive_str_internal("dup");
        let g1 = crate::string_slab::olive_str_gen_of(first);
        let g2 = crate::string_slab::olive_str_gen_of(second);
        olive_set_add(set, first);
        olive_set_add(set, second);
        assert_eq!(unsafe { (*(set as *const OliveHashSet)).len }, 1);
        assert_eq!(crate::string_slab::olive_str_gen_stale(second, g2), 1);
        assert_eq!(crate::olive_str_from_ptr(first), "dup");
        olive_free_set(set);
        assert_eq!(crate::string_slab::olive_str_gen_stale(first, g1), 1);
    }

    #[test]
    fn duplicate_insert_typed_releases_rejected() {
        use crate::format::D_STR;
        let desc = [D_STR];
        let desc_ptr = desc.as_ptr() as i64;
        let set = olive_set_new(4);
        let first = crate::olive_str_internal("dup-typed");
        let second = crate::olive_str_internal("dup-typed");
        let g1 = crate::string_slab::olive_str_gen_of(first);
        let g2 = crate::string_slab::olive_str_gen_of(second);
        crate::hash_typed::olive_set_add_typed(set, first, desc_ptr);
        crate::hash_typed::olive_set_add_typed(set, second, desc_ptr);
        assert_eq!(unsafe { (*(set as *const OliveHashSet)).len }, 1);
        assert_eq!(crate::string_slab::olive_str_gen_stale(second, g2), 1);
        olive_free_set(set);
        assert_eq!(crate::string_slab::olive_str_gen_stale(first, g1), 1);
    }

    #[test]
    fn remove_releases_stored_string() {
        let set = olive_set_new(4);
        let stored = crate::olive_str_internal("gone");
        let gs = crate::string_slab::olive_str_gen_of(stored);
        olive_set_add(set, stored);
        let query = crate::olive_str_internal("gone");
        let gq = crate::string_slab::olive_str_gen_of(query);
        assert_eq!(olive_set_contains(set, query), 1);
        let ret = olive_set_remove(set, query);
        assert_eq!(ret, query);
        assert_eq!(unsafe { (*(set as *const OliveHashSet)).len }, 0);
        assert_eq!(crate::string_slab::olive_str_gen_stale(stored, gs), 1);
        assert_eq!(crate::olive_str_from_ptr(query), "gone");
        crate::olive_free_str(query);
        assert_eq!(crate::string_slab::olive_str_gen_stale(query, gq), 1);
        olive_free_set(set);
    }

    #[test]
    fn remove_with_same_pointer_keeps_caller_word() {
        let set = olive_set_new(4);
        let a = crate::olive_str_internal("same");
        let g = crate::string_slab::olive_str_gen_of(a);
        olive_set_add(set, a);
        let ret = olive_set_remove(set, a);
        assert_eq!(ret, a);
        assert_eq!(unsafe { (*(set as *const OliveHashSet)).len }, 0);
        assert_eq!(crate::olive_str_from_ptr(a), "same");
        crate::olive_free_str(a);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, g), 1);
        olive_free_set(set);
    }

    #[test]
    fn remove_releases_stored_boxed_int() {
        let set = olive_set_new(4);
        let big = 1i64 << 61;
        let stored = crate::boxed::olive_box_int(big);
        assert!(crate::slab::slot_is_live(stored));
        olive_set_add(set, stored);
        let query = crate::boxed::olive_box_int(big);
        assert!(crate::slab::slot_is_live(query));
        let ret = olive_set_remove(set, query);
        assert_eq!(ret, query);
        assert_eq!(unsafe { (*(set as *const OliveHashSet)).len }, 0);
        assert!(!crate::slab::slot_is_live(stored));
        assert!(crate::slab::slot_is_live(query));
        crate::boxed::olive_free_boxed(query);
        assert!(!crate::slab::slot_is_live(query));
        olive_free_set(set);
    }

    #[test]
    fn remove_absent_keeps_set_intact() {
        let set = olive_set_new(4);
        let a = crate::olive_str_internal("kept");
        let g = crate::string_slab::olive_str_gen_of(a);
        olive_set_add(set, a);
        let query = crate::olive_str_internal("missing");
        let gq = crate::string_slab::olive_str_gen_of(query);
        let ret = olive_set_remove(set, query);
        assert_eq!(ret, query);
        assert_eq!(unsafe { (*(set as *const OliveHashSet)).len }, 1);
        assert_eq!(crate::olive_str_from_ptr(a), "kept");
        crate::olive_free_str(query);
        assert_eq!(crate::string_slab::olive_str_gen_stale(query, gq), 1);
        olive_free_set(set);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, g), 1);
    }

    #[test]
    fn null_add_releases_owned_string() {
        let s = crate::olive_str_internal("orphan");
        let g = crate::string_slab::olive_str_gen_of(s);
        olive_set_add(0, s);
        assert_eq!(crate::string_slab::olive_str_gen_stale(s, g), 1);
    }

    #[test]
    fn null_add_typed_releases_owned_string() {
        use crate::format::D_STR;
        let desc = [D_STR];
        let desc_ptr = desc.as_ptr() as i64;
        let s = crate::olive_str_internal("orphan-typed");
        let g = crate::string_slab::olive_str_gen_of(s);
        crate::hash_typed::olive_set_add_typed(0, s, desc_ptr);
        assert_eq!(crate::string_slab::olive_str_gen_stale(s, g), 1);
    }
}
