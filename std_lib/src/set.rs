use crate::slab::GenSlab;
use crate::*;
use std::cell::UnsafeCell;

use rustc_hash::FxHashSet;

thread_local! {
    static SET_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::with_cleanup(std::mem::size_of::<OliveHashSet>(), release_set_storage)) };
}

/// Whether `v` lives in a set slab. Gates set key reads so raw structs
/// (whose headers collide with the set kind) never read past their slots.
pub(crate) fn owns_set(v: i64) -> bool {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            if (*active).set.owns_addr(v as usize) {
                return true;
            }
            if crate::slab::active_slab_is_global() {
                return SET_SLAB.with(|sl| (*sl.get()).owns_addr(v as usize));
            }
            return crate::slab::global_set_owns_addr(v as usize);
        }
        SET_SLAB.with(|sl| (*sl.get()).owns_addr(v as usize))
            || crate::slab::global_set_owns_addr(v as usize)
    }
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

/// Adds an owned word to a result set under construction, releasing it when
/// an equal element is already present. Internal set combinators pass owned
/// copies, so a rejected copy must not leak.
fn set_add_owned(set_ptr: i64, val: i64) {
    let present = unsafe {
        let s = &*(set_ptr as *const OliveHashSet);
        (*s.inner).contains(&OliveStringKey(val))
    };
    if present {
        crate::free_any_word(val);
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
                crate::free_any_word(*s.ptr.add(i));
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

/// Descriptor-driven set snapshot: copies each element through the set's
/// static element type instead of kind dispatch. A raw struct word misreads
/// by kind (a 1-field header is `KIND_LIST`, and struct bodies are not even
/// 8-aligned words the kind reader expects), so struct-element sets must
/// take this entry; the compiler selects it exactly when the element type
/// owns heap data. `set_desc` is the full `Set(E)` descriptor; the element
/// descriptor starts at offset 1.
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_items_typed(set_ptr: i64, set_desc: i64) -> i64 {
    if set_ptr == 0 {
        return crate::list::olive_list_new(0);
    }
    // SAFETY: same contract as the untyped snapshot above — the compiler
    // passes a live set of the statically described element type. Element
    // reads stay inside the header length; copies go through the
    // descriptor, never raw kind dispatch.
    let s = unsafe { &*(set_ptr as *const OliveHashSet) };
    let list = crate::list::olive_list_new(s.len as i64);
    let mut visited = rustc_hash::FxHashMap::default();
    let desc = set_desc as *const u8;
    let elem_start = 1usize;
    for i in 0..s.len {
        let val = unsafe { *s.ptr.add(i) };
        let mut pos = elem_start;
        let copied = crate::copy_typed::copy_val(val, desc, &mut pos, &mut visited);
        crate::list::olive_list_set(list, i as i64, copied);
    }
    list
}

/// A `__drop__` hook as a callable word, for per-element cleanup below.
type ElementDropHook = extern "C" fn(i64) -> i64;

/// Runs a struct element's `__drop__` for every live element of a set whose
/// element type statically carries one, zeroing each slot as it goes so the
/// set's own drop (which follows) only releases the buffer. The membership
/// index is untouched and never consulted again before the drop.
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_drop_each_struct(ptr: i64, hook: i64) {
    if ptr == 0 || hook == 0 {
        return;
    }
    let hook: ElementDropHook = unsafe { std::mem::transmute(hook as usize) };
    let s = unsafe { &mut *(ptr as *mut OliveHashSet) };
    for i in 0..s.len {
        let slot = unsafe { s.ptr.add(i) };
        let elem = unsafe { *slot };
        if elem != 0 {
            unsafe { *slot = 0 };
            hook(elem);
        }
    }
}

/// Union elements: only struct-boxed members decode into the hook (the shell
/// is released by the unbox); scalars pass through to the ordinary drop
/// untouched, so only hooked arms are zeroed.
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_drop_each_union(ptr: i64, hook: i64) {
    if ptr == 0 || hook == 0 {
        return;
    }
    let hook: ElementDropHook = unsafe { std::mem::transmute(hook as usize) };
    let s = unsafe { &mut *(ptr as *mut OliveHashSet) };
    for i in 0..s.len {
        let slot = unsafe { s.ptr.add(i) };
        let elem = unsafe { *slot };
        if elem != 0 && crate::boxed::olive_any_is_struct_box(elem) != 0 {
            let inner = crate::struct_box::olive_struct_unbox_take(elem);
            unsafe { *slot = 0 };
            hook(inner);
        }
    }
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
        crate::free_any_word(val);
        return;
    }
    if !set_try_add(set_ptr, val) {
        crate::free_any_word(val);
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
    olive_set_remove_inner(set_ptr, val, None)
}

/// Shared body for `remove`/`discard`: unlinks `val` and releases the
/// stored copy. `elem_desc` (a `*const u8` cast to `i64`) frees the stored
/// word through the set's static element type; without it the word goes
/// through kind dispatch, which misreads raw struct payloads (a 1-field
/// header is `KIND_LIST`). The typed entry points pass their key
/// descriptor, which for a set *is* the element descriptor.
pub(crate) fn olive_set_remove_inner(set_ptr: i64, val: i64, elem_desc: Option<i64>) -> i64 {
    if set_ptr == 0 {
        return 0;
    }
    // SAFETY: body moved verbatim from the extern below — same contract
    // (live set; buffer/len/cap coherent), plus an optional element
    // descriptor for the stored-word release.
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
                    match elem_desc {
                        Some(desc) => {
                            let mut p = 0usize;
                            crate::free_typed::free_val(stored, desc as *const u8, &mut p);
                        }
                        None => crate::free_any_word(stored),
                    }
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
/// keeps `olive_set_remove`'s existing silent-on-absence behavior. Both
/// release through the untyped path, exact for scalar and `Any`-boxed
/// elements; struct elements take the typed entries below.
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_remove_checked(set_ptr: i64, val: i64, loc: i64) -> i64 {
    olive_set_remove_checked_inner(set_ptr, val, loc, None)
}

pub(crate) fn olive_set_remove_checked_inner(
    set_ptr: i64,
    val: i64,
    loc: i64,
    elem_desc: Option<i64>,
) -> i64 {
    // SAFETY: body moved verbatim from the extern below — same contract
    // (live set on the checked path; reads stay inside the header).
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
    olive_set_remove_inner(set_ptr, val, elem_desc)
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
            crate::free_any_word(*s.ptr.add(i));
        }
        (*s.inner).clear();
        s.len = 0;
    }
    set_ptr
}

/// Descriptor-driven `s.clear()`: elements release through the set's
/// static element type instead of kind dispatch, which misreads raw
/// struct payloads. `set_desc` is the full `Set(E)` descriptor with the
/// element encoding at offset 1. Element hooks run through the usual
/// typed-free registry path when the last reference goes away.
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_clear_typed(set_ptr: i64, set_desc: i64) -> i64 {
    if set_ptr == 0 {
        return set_ptr;
    }
    // SAFETY: same contract as the untyped clear — the compiler passes a
    // live set of the statically described element type. Element release
    // goes through the descriptor; the membership table clears after every
    // element is released, so no path reads a freed slot.
    unsafe {
        let s = &mut *(set_ptr as *mut OliveHashSet);
        let desc = set_desc as *const u8;
        for i in 0..s.len {
            let elem = *s.ptr.add(i);
            if elem != 0 {
                let mut pos = 1usize;
                crate::free_typed::free_val(elem, desc, &mut pos);
            }
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

/// Deep-copies one element word through the element descriptor.
#[inline]
fn copy_typed_elem(
    val: i64,
    desc: *const u8,
    visited: &mut rustc_hash::FxHashMap<i64, i64>,
) -> i64 {
    let mut pos = 0usize;
    crate::copy_typed::copy_val(val, desc, &mut pos, visited)
}

/// Inserts an owned copy, releasing it through `desc` when an equal element
/// is already present. Typed counterpart to `set_add_owned`.
fn typed_add_owned(set_ptr: i64, val: i64, desc: *const u8) {
    if !set_try_add(set_ptr, val) {
        crate::free_typed::olive_free_typed(val, desc as i64);
    }
}

/// Typed set algebra (`s | t`, `s & t`, `s - t`, `s ^ t` on concrete element
/// types). `key_desc` is the element descriptor, the same contract as
/// `olive_set_add_typed`. The untyped bodies hash through the string-pointer
/// magnitude heuristic, which misreads a raw odd int above the tag floor (or
/// an odd float bit pattern) as a string pointer and faults dereferencing
/// the raw bits; the descriptor drives exact hashing, copying, and
/// duplicate release instead. Null inputs yield empty sets (never a list).
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_union_typed(a: i64, b: i64, key_desc: i64) -> i64 {
    crate::hash_typed::with_key_descriptor(key_desc, || {
        let desc = key_desc as *const u8;
        let total = [a, b]
            .iter()
            .filter(|&&s| s != 0)
            .map(|&s| unsafe { (*(s as *const OliveHashSet)).len })
            .sum::<usize>();
        let result = olive_set_new(total as i64);
        let mut visited = rustc_hash::FxHashMap::default();
        for src in [a, b] {
            if src == 0 {
                continue;
            }
            let s = unsafe { &*(src as *const OliveHashSet) };
            for i in 0..s.len {
                let val = unsafe { *s.ptr.add(i) };
                typed_add_owned(result, copy_typed_elem(val, desc, &mut visited), desc);
            }
        }
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_intersection_typed(a: i64, b: i64, key_desc: i64) -> i64 {
    crate::hash_typed::with_key_descriptor(key_desc, || {
        if a == 0 || b == 0 {
            return olive_set_new(0);
        }
        let desc = key_desc as *const u8;
        let (sa, sb) = unsafe { (&*(a as *const OliveHashSet), &*(b as *const OliveHashSet)) };
        let result = olive_set_new(sa.len.min(sb.len) as i64);
        let mut visited = rustc_hash::FxHashMap::default();
        for i in 0..sa.len {
            let val = unsafe { *sa.ptr.add(i) };
            if unsafe { (*sb.inner).contains(&OliveStringKey(val)) } {
                typed_add_owned(result, copy_typed_elem(val, desc, &mut visited), desc);
            }
        }
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_diff_typed(a: i64, b: i64, key_desc: i64) -> i64 {
    crate::hash_typed::with_key_descriptor(key_desc, || {
        if a == 0 {
            return olive_set_new(0);
        }
        let desc = key_desc as *const u8;
        let sa = unsafe { &*(a as *const OliveHashSet) };
        let result = olive_set_new(sa.len as i64);
        let mut visited = rustc_hash::FxHashMap::default();
        for i in 0..sa.len {
            let val = unsafe { *sa.ptr.add(i) };
            let excluded = b != 0
                && unsafe { (*(b as *const OliveHashSet)).inner.as_ref() }
                    .is_some_and(|inner| inner.contains(&OliveStringKey(val)));
            if !excluded {
                typed_add_owned(result, copy_typed_elem(val, desc, &mut visited), desc);
            }
        }
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_sym_diff_typed(a: i64, b: i64, key_desc: i64) -> i64 {
    crate::hash_typed::with_key_descriptor(key_desc, || {
        let desc = key_desc as *const u8;
        let total = [a, b]
            .iter()
            .filter(|&&s| s != 0)
            .map(|&s| unsafe { (*(s as *const OliveHashSet)).len })
            .sum::<usize>();
        let result = olive_set_new(total as i64);
        let mut visited = rustc_hash::FxHashMap::default();
        for (src, other) in [(a, b), (b, a)] {
            if src == 0 {
                continue;
            }
            let s = unsafe { &*(src as *const OliveHashSet) };
            for i in 0..s.len {
                let val = unsafe { *s.ptr.add(i) };
                let excluded = other != 0
                    && unsafe { (*(other as *const OliveHashSet)).inner.as_ref() }
                        .is_some_and(|inner| inner.contains(&OliveStringKey(val)));
                if !excluded {
                    typed_add_owned(result, copy_typed_elem(val, desc, &mut visited), desc);
                }
            }
        }
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4-aligned descriptor buffer for `_typed` key ops. The key path strips
    /// the low 2 tag bits (`str_body`, a no-op for 4-aligned codegen data);
    /// a plain `[u8; N]` local is only 1-aligned and would be corrupted.
    #[repr(align(4))]
    struct AlignedDesc<const N: usize>([u8; N]);

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
    fn typed_combinators_handle_big_odd_ints() {
        use crate::format::D_INT;
        let desc = AlignedDesc([D_INT]);
        let desc_ptr = desc.0.as_ptr() as i64;
        let add = |s: i64, v: i64| crate::hash_typed::olive_set_add_typed(s, v, desc_ptr);
        let contains = |s: i64, v: i64| crate::hash_typed::olive_set_contains_typed(s, v, desc_ptr);
        let a = olive_set_new(4);
        let b = olive_set_new(4);
        add(a, 99999);
        add(a, 2);
        add(b, 99999);
        add(b, 3);
        let u = olive_set_union_typed(a, b, desc_ptr);
        assert_eq!(unsafe { (*(u as *const OliveHashSet)).len }, 3);
        assert_eq!(contains(u, 99999), 1);
        assert_eq!(contains(u, 2), 1);
        assert_eq!(contains(u, 3), 1);
        let i = olive_set_intersection_typed(a, b, desc_ptr);
        assert_eq!(unsafe { (*(i as *const OliveHashSet)).len }, 1);
        assert_eq!(contains(i, 99999), 1);
        let d = olive_set_diff_typed(a, b, desc_ptr);
        assert_eq!(unsafe { (*(d as *const OliveHashSet)).len }, 1);
        assert_eq!(contains(d, 2), 1);
        let s = olive_set_sym_diff_typed(a, b, desc_ptr);
        assert_eq!(unsafe { (*(s as *const OliveHashSet)).len }, 2);
        assert_eq!(contains(s, 2), 1);
        assert_eq!(contains(s, 3), 1);
        olive_free_set(a);
        olive_free_set(b);
        olive_free_set(u);
        olive_free_set(i);
        olive_free_set(d);
        olive_free_set(s);
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
        let desc = AlignedDesc([D_STR]);
        let desc_ptr = desc.0.as_ptr() as i64;
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
        let desc = AlignedDesc([D_STR]);
        let desc_ptr = desc.0.as_ptr() as i64;
        let s = crate::olive_str_internal("orphan-typed");
        let g = crate::string_slab::olive_str_gen_of(s);
        crate::hash_typed::olive_set_add_typed(0, s, desc_ptr);
        assert_eq!(crate::string_slab::olive_str_gen_stale(s, g), 1);
    }

    #[test]
    fn items_typed_shares_struct_element_and_frees_once() {
        use crate::format::{D_SET, D_STR, D_STRUCT_SHARED};
        use crate::slab::slot_is_live;
        let desc = [D_SET, D_STRUCT_SHARED, 14, b'R', 14, 14, b's', D_STR];
        let desc_ptr = desc.as_ptr() as i64;
        let text = crate::olive_str_internal("typed snapshot resource field value");
        let value = crate::olive_struct_alloc(1);
        unsafe { *((value as *mut i64).add(1)) = text };
        let set = olive_set_new(4);
        assert!(set_try_add(set, value));
        let snapshot = olive_set_items_typed(set, desc_ptr);
        assert_eq!(crate::list::olive_list_len(snapshot), 1);
        assert_eq!(crate::list::olive_list_get(snapshot, 0), value);
        assert!(slot_is_live(value));
        unsafe { crate::list::free_snapshot_typed(snapshot, desc_ptr) };
        assert!(slot_is_live(value));
        crate::free_typed::olive_free_typed(set, desc_ptr);
        assert!(!slot_is_live(value));
    }

    #[test]
    fn remove_typed_releases_stored_struct() {
        use crate::format::{D_SET, D_STR, D_STRUCT_SHARED};
        use crate::slab::slot_is_live;
        let desc = AlignedDesc([D_SET, D_STRUCT_SHARED, 14, b'R', 14, 14, b's', D_STR]);
        let mk = || {
            let text = crate::olive_str_internal("typed remove resource field value");
            let value = crate::olive_struct_alloc(1);
            unsafe { *((value as *mut i64).add(1)) = text };
            value
        };
        let stored = mk();
        let set = olive_set_new(4);
        // The typed entries carry the *element* descriptor (from the value
        // argument), not the set descriptor: skip the D_SET tag. It also
        // drives structural hashing, so insertion goes through the typed
        // add like the production path.
        let elem_desc = unsafe { desc.0.as_ptr().add(1) } as i64;
        crate::hash_typed::olive_set_add_typed(set, stored, elem_desc);
        let arg = mk();
        let out = crate::hash_typed::with_key_descriptor(elem_desc, || {
            olive_set_remove_inner(set, arg, Some(elem_desc))
        });
        assert_eq!(out, arg);
        assert!(!slot_is_live(stored));
        assert!(slot_is_live(arg));
        assert_eq!(unsafe { (*(set as *const OliveHashSet)).len }, 0);
        crate::free_typed::olive_free_typed(arg, elem_desc);
        assert!(!slot_is_live(arg));
        olive_free_set(set);
    }

    #[test]
    fn clear_typed_releases_struct_elements() {
        use crate::format::{D_SET, D_STR, D_STRUCT_SHARED};
        use crate::slab::slot_is_live;
        let desc = [D_SET, D_STRUCT_SHARED, 14, b'R', 14, 14, b's', D_STR];
        let desc_ptr = desc.as_ptr() as i64;
        let text = crate::olive_str_internal("typed clear resource field value");
        let value = crate::olive_struct_alloc(1);
        unsafe { *((value as *mut i64).add(1)) = text };
        let set = olive_set_new(4);
        assert!(set_try_add(set, value));
        olive_set_clear_typed(set, desc_ptr);
        assert!(!slot_is_live(value));
        assert_eq!(unsafe { (*(set as *const OliveHashSet)).len }, 0);
        assert!(slot_is_live(set));
        olive_free_set(set);
    }
}
