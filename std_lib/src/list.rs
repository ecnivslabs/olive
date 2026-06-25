use crate::slab::GenSlab;
use crate::*;
use std::cell::UnsafeCell;

/// Element buffers up to this capacity stay attached to a freed slot for reuse.
pub(crate) const RETAIN_CAP: usize = 4;

thread_local! {
    static LIST_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::new(std::mem::size_of::<StableVec>())) };
    static ITER_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::new(std::mem::size_of::<OliveIter>())) };
}

/// Allocates a list header from the slab and fills it. A recycled slot may
/// carry a retained element buffer, which is released before overwriting.
pub(crate) fn alloc_list_header(kind: i64, ptr: *mut i64, cap: usize, len: usize) -> i64 {
    let slab_alloc = |sl: &mut GenSlab| {
        let (body, fresh) = sl.alloc();
        let s = body as *mut StableVec;
        unsafe {
            if !fresh && !cfg!(debug_assertions) {
                let old = &*s;
                if !old.ptr.is_null() && old.cap > 0 {
                    let _ = Vec::from_raw_parts(old.ptr, 0, old.cap);
                }
            }
            (*s).kind = kind;
            (*s).ptr = ptr;
            (*s).cap = cap;
            (*s).len = len;
        }
        body as i64
    };
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            slab_alloc(&mut (*active).list)
        } else {
            LIST_SLAB.with(|sl| slab_alloc(&mut *sl.get()))
        }
    }
}

pub(crate) fn list_from_vec(mut v: Vec<i64>) -> i64 {
    let ptr = v.as_mut_ptr();
    let cap = v.capacity();
    let len = v.len();
    std::mem::forget(v);
    alloc_list_header(KIND_LIST, ptr, cap, len)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_new(len: i64) -> i64 {
    let n = len as usize;
    let slab_alloc = |sl: &mut GenSlab| {
        let (body, fresh) = sl.alloc();
        let s = unsafe { &mut *(body as *mut StableVec) };
        if fresh || cfg!(debug_assertions) {
            let mut v = vec![0i64; n];
            s.ptr = v.as_mut_ptr();
            s.cap = v.capacity();
            std::mem::forget(v);
        } else {
            if s.ptr.is_null() || s.cap < n {
                let mut v = if s.ptr.is_null() {
                    Vec::with_capacity(n)
                } else {
                    unsafe { Vec::from_raw_parts(s.ptr, 0, s.cap) }
                };
                v.reserve(n);
                s.ptr = v.as_mut_ptr();
                s.cap = v.capacity();
                std::mem::forget(v);
            }
            unsafe {
                std::ptr::write_bytes(s.ptr, 0, n);
            }
        }
        s.kind = KIND_LIST;
        s.len = n;
        body as i64
    };
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            slab_alloc(&mut (*active).list)
        } else {
            LIST_SLAB.with(|sl| slab_alloc(&mut *sl.get()))
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_range_list(start: i64, end: i64, inclusive: i64, step: i64) -> i64 {
    // Matches Python's `range(start, stop, step)` element count: derive an
    // exclusive `last` from `inclusive` (direction-aware, since `..=` walks
    // toward `end` from either side), then floor-divide. Numerator and `step`
    // always share a sign for a non-empty range, so Rust's truncating `/`
    // coincides with floor division; `max(0)` covers the empty case either way.
    let count = if step > 0 {
        let last = if inclusive != 0 { end + 1 } else { end };
        (last - start + step - 1) / step
    } else {
        let last = if inclusive != 0 { end - 1 } else { end };
        (last - start + step + 1) / step
    }
    .max(0);
    let list = olive_list_new(count);
    for i in 0..count {
        olive_list_set(list, i, start + i * step);
    }
    list
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_min_int(ptr: i64) -> i64 {
    let v = checked_nonempty(ptr, "min");
    let mut m = unsafe { *v.ptr };
    for i in 1..v.len {
        let e = unsafe { *v.ptr.add(i) };
        if e < m {
            m = e;
        }
    }
    m
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_max_int(ptr: i64) -> i64 {
    let v = checked_nonempty(ptr, "max");
    let mut m = unsafe { *v.ptr };
    for i in 1..v.len {
        let e = unsafe { *v.ptr.add(i) };
        if e > m {
            m = e;
        }
    }
    m
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_min_float(ptr: i64) -> f64 {
    let v = checked_nonempty(ptr, "min");
    let mut m = f64::from_bits(unsafe { *v.ptr } as u64);
    for i in 1..v.len {
        let e = f64::from_bits(unsafe { *v.ptr.add(i) } as u64);
        if e < m {
            m = e;
        }
    }
    m
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_max_float(ptr: i64) -> f64 {
    let v = checked_nonempty(ptr, "max");
    let mut m = f64::from_bits(unsafe { *v.ptr } as u64);
    for i in 1..v.len {
        let e = f64::from_bits(unsafe { *v.ptr.add(i) } as u64);
        if e > m {
            m = e;
        }
    }
    m
}

fn checked_nonempty<'a>(ptr: i64, who: &str) -> &'a StableVec {
    if ptr == 0 {
        crate::panic::abort(&format!("{who}() of empty list"), None);
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    if v.len == 0 {
        crate::panic::abort(&format!("{who}() of empty list"), None);
    }
    v
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_sum_int(ptr: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    let mut acc: i64 = 0;
    for i in 0..v.len {
        acc = acc.wrapping_add(unsafe { *v.ptr.add(i) });
    }
    acc
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_sum_float(ptr: i64) -> f64 {
    if ptr == 0 {
        return 0.0;
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    let mut acc = 0.0f64;
    for i in 0..v.len {
        acc += f64::from_bits(unsafe { *v.ptr.add(i) } as u64);
    }
    acc
}

/// `any(xs)` over a `[bool]` list: elements are raw 0/1 words.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_any_bool(ptr: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    let slice = unsafe { std::slice::from_raw_parts(v.ptr, v.len) };
    slice.iter().any(|&e| e != 0) as i64
}

/// `all(xs)` over a `[bool]` list: elements are raw 0/1 words.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_all_bool(ptr: i64) -> i64 {
    if ptr == 0 {
        return 1;
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    let slice = unsafe { std::slice::from_raw_parts(v.ptr, v.len) };
    slice.iter().all(|&e| e != 0) as i64
}

/// `any(xs)` over an `[Any]` list: each element is truthy-checked by its own tag.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_any_any(ptr: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    let slice = unsafe { std::slice::from_raw_parts(v.ptr, v.len) };
    slice
        .iter()
        .any(|&e| crate::boxed::olive_any_truthy(e) != 0) as i64
}

/// `all(xs)` over an `[Any]` list: each element is truthy-checked by its own tag.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_all_any(ptr: i64) -> i64 {
    if ptr == 0 {
        return 1;
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    let slice = unsafe { std::slice::from_raw_parts(v.ptr, v.len) };
    slice
        .iter()
        .all(|&e| crate::boxed::olive_any_truthy(e) != 0) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_set(list_ptr: i64, idx: i64, val: i64) {
    if list_ptr == 0 {
        return;
    }
    let s = unsafe { &mut *(list_ptr as *mut StableVec) };
    if (idx as usize) < s.len {
        unsafe {
            *s.ptr.add(idx as usize) = val;
        }
    }
}

/// Reverses a list in place. Element representation is irrelevant.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_reverse(list_ptr: i64) {
    if list_ptr == 0 {
        return;
    }
    let s = unsafe { &mut *(list_ptr as *mut StableVec) };
    let slice = unsafe { std::slice::from_raw_parts_mut(s.ptr, s.len) };
    slice.reverse();
}

fn list_slice_mut<'a>(list_ptr: i64) -> Option<&'a mut [i64]> {
    if list_ptr == 0 {
        return None;
    }
    let s = unsafe { &mut *(list_ptr as *mut StableVec) };
    Some(unsafe { std::slice::from_raw_parts_mut(s.ptr, s.len) })
}

/// Sorts a list of integers ascending, in place.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_sort_int(list_ptr: i64) {
    if let Some(slice) = list_slice_mut(list_ptr) {
        slice.sort_unstable();
    }
}

/// Sorts a list of floats ascending, in place. Elements are stored as bit
/// patterns, so they are read back as `f64` for the comparison.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_sort_float(list_ptr: i64) {
    if let Some(slice) = list_slice_mut(list_ptr) {
        slice.sort_by(|a, b| {
            let fa = f64::from_bits(*a as u64);
            let fb = f64::from_bits(*b as u64);
            fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal)
        });
    }
}

/// Sorts a list of strings lexicographically, in place.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_sort_str(list_ptr: i64) {
    if let Some(slice) = list_slice_mut(list_ptr) {
        slice.sort_by_key(|&p| crate::olive_str_from_ptr(p));
    }
}

/// `key_kind` for `olive_list_sort_by_keys` below: which ordering the key's
/// own raw word needs (the key's type, not the element's). `0` (int/bool)
/// is the fallback for any other tag, so it has no named constant.
const KEY_FLOAT: i64 = 1;
const KEY_STR: i64 = 2;

fn cmp_key(a: i64, b: i64, key_kind: i64) -> std::cmp::Ordering {
    match key_kind {
        KEY_FLOAT => f64::from_bits(a as u64)
            .partial_cmp(&f64::from_bits(b as u64))
            .unwrap_or(std::cmp::Ordering::Equal),
        KEY_STR => crate::olive_str_from_ptr(a).cmp(&crate::olive_str_from_ptr(b)),
        _ => a.cmp(&b),
    }
}

/// `sort(key=f)`'s decorate-sort-undecorate variant (E5.5): the caller
/// evaluates `f(elem)` once per element into `keys_ptr` (same length and
/// order as `list_ptr`), this reorders `list_ptr` to match `keys_ptr`'s
/// sort order. `keys_ptr` is caller-owned; freeing it is the caller's job.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_sort_by_keys(list_ptr: i64, keys_ptr: i64, key_kind: i64) {
    let Some(elems) = list_slice_mut(list_ptr) else {
        return;
    };
    let Some(keys) = list_slice_mut(keys_ptr) else {
        return;
    };
    let n = elems.len().min(keys.len());
    let mut order: Vec<u32> = (0..n as u32).collect();
    if key_kind == KEY_STR {
        let decoded: Vec<String> = keys[..n]
            .iter()
            .map(|&k| crate::olive_str_from_ptr(k))
            .collect();
        order.sort_by(|&i, &j| decoded[i as usize].cmp(&decoded[j as usize]));
    } else {
        order.sort_by(|&i, &j| cmp_key(keys[i as usize], keys[j as usize], key_kind));
    }
    apply_order(elems, &order);
}

fn apply_order(elems: &mut [i64], order: &[u32]) {
    let n = elems.len().min(order.len());
    let orig: Vec<i64> = elems[..n].to_vec();
    for (out_i, &src_i) in order[..n].iter().enumerate() {
        elems[out_i] = orig[src_i as usize];
    }
}

/// Permutes `list_ptr` in place to match a caller-computed `order` (an
/// Olive `[int]` list of source indices): `list_ptr[i]` becomes the element
/// that was originally at `order[i]`. Pure word reordering -- no allocation,
/// no calls back into user code -- the safe half of a comparator sort whose
/// comparisons ran in MIR against borrowed (`&Self`) reads (E6.3).
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_apply_order(list_ptr: i64, order_ptr: i64) {
    let Some(elems) = list_slice_mut(list_ptr) else {
        return;
    };
    let Some(order) = list_slice_mut(order_ptr) else {
        return;
    };
    let order: Vec<u32> = order.iter().map(|&v| v as u32).collect();
    apply_order(elems, &order);
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_get(list_ptr: i64, idx: i64) -> i64 {
    if list_ptr == 0 {
        return 0;
    }
    let s = unsafe { &*(list_ptr as *const StableVec) };
    let i = if idx < 0 { idx + s.len as i64 } else { idx };
    if (i as usize) < s.len {
        unsafe { *s.ptr.add(i as usize) }
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_len(ptr: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    unsafe {
        let raw_ptr = ptr as *const libc::c_void;
        if python::is_readable_ptr(raw_ptr) {
            let kind = *(ptr as *const i64);
            if kind == KIND_PYOBJECT {
                return python::olive_py_len(ptr as *mut libc::c_void);
            }
        }
        (*(ptr as *const StableVec)).len as i64
    }
}

/// Checks a starred destructure's source has at least `min_len` elements
/// (the plain names on the left); returns the list's actual length so the
/// caller can compute the starred slice's bound without a second call.
#[unsafe(no_mangle)]
pub extern "C" fn olive_check_list_min_len(ptr: i64, min_len: i64, loc: i64) -> i64 {
    let len = olive_list_len(ptr);
    if len < min_len {
        crate::panic::olive_starred_unpack_fail(len, min_len, loc);
    }
    len
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_insert(list_ptr: i64, idx: i64, val: i64) {
    if list_ptr == 0 {
        return;
    }
    unsafe {
        let s = &mut *(list_ptr as *mut StableVec);
        let idx = idx as usize;
        let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
        if idx <= v.len() {
            v.insert(idx, val);
        }
        s.ptr = v.as_mut_ptr();
        s.cap = v.capacity();
        s.len = v.len();
        std::mem::forget(v);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_remove(list_ptr: i64, idx: i64) -> i64 {
    if list_ptr == 0 {
        return 0;
    }
    unsafe {
        let s = &mut *(list_ptr as *mut StableVec);
        let idx = idx as usize;
        if idx >= s.len {
            return 0;
        }
        let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
        let val = v.remove(idx);
        s.ptr = v.as_mut_ptr();
        s.cap = v.capacity();
        s.len = v.len();
        std::mem::forget(v);
        val
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_pop(list_ptr: i64) -> i64 {
    if list_ptr == 0 {
        return 0;
    }
    unsafe {
        let s = &mut *(list_ptr as *mut StableVec);
        if s.len == 0 {
            return 0;
        }
        let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
        let val = v.pop().unwrap_or(0);
        s.ptr = v.as_mut_ptr();
        s.cap = v.capacity();
        s.len = v.len();
        std::mem::forget(v);
        val
    }
}

/// Concat that reuses the left list's storage and steals the right list's
/// elements. Only reachable when ownership proved both operands die at this
/// statement, so no element is shared afterward. The right list is left as
/// an empty shell for its pending drop.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_concat_move(l: i64, r: i64) -> i64 {
    if l == 0 {
        if r == 0 {
            return 0;
        }
        return olive_list_concat_move(olive_list_new(0), r);
    }
    if r == 0 {
        return l;
    }
    unsafe {
        let sl = &mut *(l as *mut StableVec);
        let sr = &mut *(r as *mut StableVec);
        let mut v = Vec::from_raw_parts(sl.ptr, sl.len, sl.cap);
        v.extend_from_slice(std::slice::from_raw_parts(sr.ptr, sr.len));
        sl.ptr = v.as_mut_ptr();
        sl.cap = v.capacity();
        sl.len = v.len();
        std::mem::forget(v);
        sr.len = 0;
    }
    l
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_concat(l: i64, r: i64) -> i64 {
    if l == 0 {
        return r;
    }
    if r == 0 {
        return l;
    }
    let sl = unsafe { &*(l as *const StableVec) };
    let sr = unsafe { &*(r as *const StableVec) };
    let mut v = Vec::with_capacity(sl.len + sr.len);
    unsafe {
        v.extend_from_slice(std::slice::from_raw_parts(sl.ptr, sl.len));
        v.extend_from_slice(std::slice::from_raw_parts(sr.ptr, sr.len));
    }
    list_from_vec(v)
}

const SLICE_HAS_START: i64 = 1;
const SLICE_HAS_STOP: i64 = 2;
const SLICE_HAS_STEP: i64 = 4;

/// Normalizes Python slice bounds against a sequence length. Handles
/// negative bounds, clamping, omitted endpoints, and negative steps exactly
/// as CPython's `PySlice_AdjustIndices`.
pub(crate) fn slice_bounds(
    len: i64,
    start: i64,
    stop: i64,
    step: i64,
    flags: i64,
) -> (i64, i64, i64) {
    let step = if flags & SLICE_HAS_STEP != 0 { step } else { 1 };
    if step == 0 {
        crate::panic::abort("slice step cannot be zero", None);
    }
    let (lower, upper) = if step < 0 { (-1, len - 1) } else { (0, len) };
    let clamp = |mut v: i64| -> i64 {
        if v < 0 {
            v += len;
            if v < lower {
                v = lower;
            }
        } else if v > upper {
            v = upper;
        }
        v
    };
    let start = if flags & SLICE_HAS_START == 0 {
        if step < 0 { upper } else { lower }
    } else {
        clamp(start)
    };
    let stop = if flags & SLICE_HAS_STOP == 0 {
        if step < 0 { lower } else { upper }
    } else {
        clamp(stop)
    };
    (start, stop, step)
}

/// Resolves a Python slice to the selected indices via `slice_bounds`.
pub(crate) fn slice_indices(len: i64, start: i64, stop: i64, step: i64, flags: i64) -> Vec<usize> {
    let (start, stop, step) = slice_bounds(len, start, stop, step, flags);
    let mut out = Vec::new();
    let mut i = start;
    if step > 0 {
        while i < stop {
            out.push(i as usize);
            i += step;
        }
    } else {
        while i > stop {
            out.push(i as usize);
            i += step;
        }
    }
    out
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_getslice(
    ptr: i64,
    start: i64,
    stop: i64,
    step: i64,
    flags: i64,
) -> i64 {
    if ptr == 0 {
        return olive_list_new(0);
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    let idxs = slice_indices(v.len as i64, start, stop, step, flags);
    let out = olive_list_new(idxs.len() as i64);
    let ov = unsafe { &mut *(out as *mut StableVec) };
    for (j, &i) in idxs.iter().enumerate() {
        unsafe { *ov.ptr.add(j) = *v.ptr.add(i) };
    }
    out
}

/// `xs * n` for scalar-element lists: `n <= 0` yields an empty list.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_repeat(ptr: i64, n: i64) -> i64 {
    if n <= 0 || ptr == 0 {
        return olive_list_new(0);
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    let out = olive_list_new(v.len as i64 * n);
    let ov = unsafe { &mut *(out as *mut StableVec) };
    for rep in 0..n as usize {
        for i in 0..v.len {
            unsafe {
                *ov.ptr.add(rep * v.len + i) = *v.ptr.add(i);
            }
        }
    }
    out
}

/// `xs.count(x)`: number of elements structurally equal to `x`. Always goes
/// through `olive_eq_typed`, whose raw `a == b` fast path already covers
/// scalars, so one path serves every element type.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_count_typed(list_ptr: i64, val: i64, desc: i64) -> i64 {
    if list_ptr == 0 {
        return 0;
    }
    let s = unsafe { &*(list_ptr as *const StableVec) };
    let mut n = 0i64;
    for i in 0..s.len {
        let elem = unsafe { *s.ptr.add(i) };
        if crate::eq_typed::olive_eq_typed(elem, val, desc) != 0 {
            n += 1;
        }
    }
    n
}

/// `xs.index(x)`: first index structurally equal to `x`; faults if absent.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_index_typed(list_ptr: i64, val: i64, loc: i64, desc: i64) -> i64 {
    if list_ptr != 0 {
        let s = unsafe { &*(list_ptr as *const StableVec) };
        for i in 0..s.len {
            let elem = unsafe { *s.ptr.add(i) };
            if crate::eq_typed::olive_eq_typed(elem, val, desc) != 0 {
                return i as i64;
            }
        }
    }
    let len = if list_ptr == 0 {
        0
    } else {
        unsafe { (*(list_ptr as *const StableVec)).len as i64 }
    };
    crate::panic::olive_bounds_fail(-1, len, loc);
    0
}

/// `xs.clear()`: empties the list in place (freeing owned elements), returns it.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_clear(ptr: i64) -> i64 {
    if ptr == 0 {
        return ptr;
    }
    let s = unsafe { &mut *(ptr as *mut StableVec) };
    for i in 0..s.len {
        let elem = unsafe { *s.ptr.add(i) };
        if is_active_object(elem) {
            olive_free_any(elem);
        }
    }
    s.len = 0;
    ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_extend(target: i64, source: i64) {
    if target == 0 || source == 0 {
        return;
    }
    let src_len = olive_list_len(source);
    for i in 0..src_len {
        let val = olive_list_get(source, i);
        olive_list_append(target, val);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_list(ptr: i64) {
    if ptr == 0 || !crate::slab::ptr_in_slab_span(ptr) {
        return;
    }
    unsafe {
        let s = &mut *(ptr as *mut StableVec);
        if s.kind == KIND_SET {
            return crate::set::olive_free_set(ptr);
        }
        if crate::slab::slot_is_live(ptr) {
            for i in 0..s.len {
                let elem = *s.ptr.add(i);
                if is_active_object(elem) {
                    olive_free_any(elem);
                }
            }
            settle_list_buffer(ptr);
        }
        free_list_slot_raw(ptr);
    }
}

/// Releases or retains the element buffer of a (possibly already freed) slot.
/// The slot body persists after a slab free, so this is safe in either order.
pub(crate) unsafe fn settle_list_buffer(ptr: i64) {
    let s = unsafe { &mut *(ptr as *mut StableVec) };
    if s.cap > RETAIN_CAP {
        if !s.ptr.is_null() {
            let _ = unsafe { Vec::from_raw_parts(s.ptr, 0, s.cap) };
        }
        s.ptr = std::ptr::null_mut();
        s.cap = 0;
    }
}

pub(crate) fn free_list_slot_raw(ptr: i64) {
    if crate::slab::chunk_is_global(ptr as usize) {
        crate::slab::with_escape_arena(|| free_list_slot_raw_local(ptr));
    } else {
        free_list_slot_raw_local(ptr);
    }
}

fn free_list_slot_raw_local(ptr: i64) {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            (*active).list.free(ptr as *mut u8);
        } else {
            LIST_SLAB.with(|sl| {
                (&mut *sl.get()).free(ptr as *mut u8);
            });
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_new_reuse(old_ptr: i64, n_len: i64, bump: i64) -> i64 {
    if old_ptr == 0 {
        return olive_list_new(n_len);
    }
    let n = n_len as usize;
    if bump != 0 {
        unsafe {
            let gen_ptr = (old_ptr as *mut std::sync::atomic::AtomicU64).sub(1);
            let g = (*gen_ptr).load(std::sync::atomic::Ordering::Relaxed) + 2;
            (*gen_ptr).store(g, std::sync::atomic::Ordering::Release);
        }
    }
    unsafe {
        let s = &mut *(old_ptr as *mut StableVec);
        if s.ptr.is_null() || s.cap < n {
            let mut v = if s.ptr.is_null() {
                Vec::with_capacity(n)
            } else {
                Vec::from_raw_parts(s.ptr, 0, s.cap)
            };
            v.reserve(n);
            s.ptr = v.as_mut_ptr();
            s.cap = v.capacity();
            std::mem::forget(v);
        }
        std::ptr::write_bytes(s.ptr, 0, n);
        s.len = n;
        s.kind = KIND_LIST;
    }
    old_ptr
}

#[repr(C)]
pub struct OliveIter {
    pub kind: i64,
    pub list_ptr: i64,
    pub index: usize,
    pub is_py: bool,
    pub py_peeked: i64,
    pub has_peeked: bool,
    // list_ptr was allocated for this iterator (dict keys, set items, str chars)
    // rather than borrowed from the iterated value, so freeing the iterator frees it.
    pub derived: bool,
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_iter(list_ptr: i64) -> i64 {
    let mut is_py = false;
    let mut derived = false;
    let mut actual_list_ptr = list_ptr;

    // A tagged pointer with a high address is a string: iterate its characters.
    if list_ptr != 0 && (list_ptr & 1) == 1 && (list_ptr & !1) > 0x10000 {
        actual_list_ptr = crate::string::olive_str_chars(list_ptr);
        derived = true;
    } else if list_ptr != 0 {
        unsafe {
            let raw_ptr = list_ptr as *const libc::c_void;
            if python::is_readable_ptr(raw_ptr) {
                let kind = *(list_ptr as *const i64);
                if kind == KIND_PYOBJECT {
                    is_py = true;
                    actual_list_ptr =
                        crate::python::python_iter::olive_py_iter(list_ptr as *mut libc::c_void)
                            as i64;
                } else if kind == KIND_OBJ {
                    // A dict iterates over its keys.
                    actual_list_ptr = crate::obj::olive_obj_keys(list_ptr);
                    derived = true;
                } else if kind == KIND_SET {
                    actual_list_ptr = crate::set::olive_set_items(list_ptr);
                    derived = true;
                }
            }
        }
    }

    ITER_SLAB.with(|sl| {
        let sl = unsafe { &mut *sl.get() };
        let (body, _) = sl.alloc();
        unsafe {
            std::ptr::write(
                body as *mut OliveIter,
                OliveIter {
                    kind: KIND_ITER,
                    list_ptr: actual_list_ptr,
                    index: 0,
                    is_py,
                    py_peeked: 0,
                    has_peeked: false,
                    derived,
                },
            );
        }
        body as i64
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_iter(ptr: i64) {
    if ptr == 0 || !crate::slab::ptr_in_slab_span(ptr) {
        return;
    }
    if crate::slab::slot_is_live(ptr) {
        unsafe {
            let it = &*(ptr as *const OliveIter);
            if it.is_py && it.list_ptr != 0 {
                crate::python::olive_py_decref(it.list_ptr as *mut libc::c_void);
            } else if it.derived && it.list_ptr != 0 {
                olive_free_list(it.list_ptr);
            }
        }
    }
    ITER_SLAB.with(|sl| {
        unsafe { &mut *sl.get() }.free(ptr as *mut u8);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_has_next(iter_ptr: i64) -> i64 {
    if iter_ptr == 0 {
        return 0;
    }
    let it = unsafe { &mut *(iter_ptr as *mut OliveIter) };
    if it.list_ptr == 0 {
        return 0;
    }
    if it.is_py {
        if it.has_peeked {
            return if it.py_peeked != 0 { 1 } else { 0 };
        }
        it.py_peeked =
            crate::python::python_iter::olive_py_iter_next(it.list_ptr as *mut libc::c_void);
        it.has_peeked = true;
        return if it.py_peeked != 0 { 1 } else { 0 };
    }
    let s = unsafe { &*(it.list_ptr as *const StableVec) };
    if it.index < s.len { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_next(iter_ptr: i64) -> i64 {
    if iter_ptr == 0 {
        return 0;
    }
    let it = unsafe { &mut *(iter_ptr as *mut OliveIter) };
    if it.list_ptr == 0 {
        return 0;
    }
    if it.is_py {
        if it.has_peeked {
            it.has_peeked = false;
            let val = it.py_peeked;
            it.py_peeked = 0;
            return val;
        }
        return crate::python::python_iter::olive_py_iter_next(it.list_ptr as *mut libc::c_void);
    }
    let s = unsafe { &*(it.list_ptr as *const StableVec) };
    if it.index < s.len {
        let val = unsafe { *s.ptr.add(it.index) };
        it.index += 1;
        val
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_is_list(val: i64) -> i64 {
    if val == 0 || (val & 1) != 0 {
        return 0;
    }
    let kind = unsafe { *(val as *const i64) };
    if kind == KIND_LIST || kind == KIND_ANY_LIST {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_list(elems: &[i64]) -> i64 {
        let ptr = olive_list_new(elems.len() as i64);
        for (i, &v) in elems.iter().enumerate() {
            olive_list_set(ptr, i as i64, v);
        }
        ptr
    }

    #[test]
    fn new_empty() {
        let ptr = olive_list_new(0);
        assert_ne!(ptr, 0);
        let s = unsafe { &*(ptr as *const StableVec) };
        assert_eq!(s.len, 0);
    }

    #[test]
    fn new_with_size() {
        let ptr = olive_list_new(5);
        assert_ne!(ptr, 0);
        let s = unsafe { &*(ptr as *const StableVec) };
        assert_eq!(s.len, 5);
        for i in 0..5 {
            assert_eq!(unsafe { *s.ptr.add(i) }, 0);
        }
    }

    #[test]
    fn get_and_set() {
        let ptr = olive_list_new(3);
        olive_list_set(ptr, 0, 42);
        olive_list_set(ptr, 1, 99);
        olive_list_set(ptr, 2, -7);
        assert_eq!(olive_list_get(ptr, 0), 42);
        assert_eq!(olive_list_get(ptr, 1), 99);
        assert_eq!(olive_list_get(ptr, 2), -7);
    }

    #[test]
    fn get_out_of_bounds() {
        let ptr = olive_list_new(1);
        assert_eq!(olive_list_get(ptr, 10), 0);
        assert_eq!(olive_list_get(ptr, !0), 0);
    }

    #[test]
    fn set_out_of_bounds_no_panic() {
        let ptr = olive_list_new(1);
        olive_list_set(ptr, 100, 42);
        assert_eq!(olive_list_get(ptr, 0), 0);
    }

    #[test]
    fn get_null_returns_zero() {
        assert_eq!(olive_list_get(0, 0), 0);
    }

    #[test]
    fn len_basic() {
        let ptr = olive_list_new(10);
        assert_eq!(olive_list_len(ptr), 10);
    }

    #[test]
    fn len_null() {
        assert_eq!(olive_list_len(0), 0);
    }

    #[test]
    fn insert_middle() {
        let ptr = make_list(&[1, 3, 4]);
        olive_list_insert(ptr, 1, 2);
        assert_eq!(olive_list_len(ptr), 4);
        assert_eq!(olive_list_get(ptr, 0), 1);
        assert_eq!(olive_list_get(ptr, 1), 2);
        assert_eq!(olive_list_get(ptr, 2), 3);
        assert_eq!(olive_list_get(ptr, 3), 4);
    }

    #[test]
    fn insert_beginning() {
        let ptr = make_list(&[2, 3]);
        olive_list_insert(ptr, 0, 1);
        assert_eq!(olive_list_len(ptr), 3);
        assert_eq!(olive_list_get(ptr, 0), 1);
    }

    #[test]
    fn insert_end() {
        let ptr = make_list(&[1, 2]);
        olive_list_insert(ptr, 2, 3);
        assert_eq!(olive_list_len(ptr), 3);
        assert_eq!(olive_list_get(ptr, 2), 3);
    }

    #[test]
    fn remove_middle() {
        let ptr = make_list(&[1, 99, 3]);
        let removed = olive_list_remove(ptr, 1);
        assert_eq!(removed, 99);
        assert_eq!(olive_list_len(ptr), 2);
        assert_eq!(olive_list_get(ptr, 0), 1);
        assert_eq!(olive_list_get(ptr, 1), 3);
    }

    #[test]
    fn remove_beginning() {
        let ptr = make_list(&[1, 2, 3]);
        let removed = olive_list_remove(ptr, 0);
        assert_eq!(removed, 1);
        assert_eq!(olive_list_len(ptr), 2);
    }

    #[test]
    fn remove_out_of_bounds() {
        let ptr = make_list(&[1]);
        let removed = olive_list_remove(ptr, 5);
        assert_eq!(removed, 0);
        assert_eq!(olive_list_len(ptr), 1);
    }

    #[test]
    fn concat_two_lists() {
        let a = make_list(&[1, 2]);
        let b = make_list(&[3, 4]);
        let c = olive_list_concat(a, b);
        assert_eq!(olive_list_len(c), 4);
        assert_eq!(olive_list_get(c, 0), 1);
        assert_eq!(olive_list_get(c, 3), 4);
    }

    #[test]
    fn concat_with_null() {
        let a = make_list(&[1, 2]);
        let c = olive_list_concat(a, 0);
        assert_eq!(c, a);
    }

    #[test]
    fn concat_move_reuses_left_and_empties_right() {
        let a = make_list(&[1, 2]);
        let b = make_list(&[3, 4]);
        let c = olive_list_concat_move(a, b);
        assert_eq!(c, a);
        assert_eq!(olive_list_len(c), 4);
        assert_eq!(olive_list_get(c, 2), 3);
        assert_eq!(olive_list_len(b), 0);
    }

    #[test]
    fn concat_move_null_left_still_owns_result() {
        let b = make_list(&[7]);
        let c = olive_list_concat_move(0, b);
        assert_ne!(c, b);
        assert_eq!(olive_list_len(c), 1);
        assert_eq!(olive_list_get(c, 0), 7);
        assert_eq!(olive_list_len(b), 0);
    }

    #[test]
    fn concat_move_null_right_is_identity() {
        let a = make_list(&[5]);
        assert_eq!(olive_list_concat_move(a, 0), a);
        assert_eq!(olive_list_len(a), 1);
    }

    #[test]
    fn extend_list() {
        let target = make_list(&[1, 2]);
        let source = make_list(&[3, 4]);
        olive_list_extend(target, source);
        assert_eq!(olive_list_len(target), 4);
        assert_eq!(olive_list_get(target, 2), 3);
    }

    #[test]
    fn is_list_true() {
        let ptr = make_list(&[]);
        assert_eq!(olive_is_list(ptr), 1);
    }

    #[test]
    fn is_list_false() {
        assert_eq!(olive_is_list(0), 0);
        assert_eq!(olive_is_list(1), 0);
        assert_eq!(olive_is_list(1 | 1), 0);
    }

    #[test]
    fn free_list_no_panic() {
        let ptr = make_list(&[1, 2, 3]);
        olive_free_list(ptr);

        let ptr2 = make_list(&[4, 5]);
        assert_ne!(ptr2, 0);
        assert_eq!(olive_list_len(ptr2), 2);
    }

    #[test]
    fn iter_basic() {
        let ptr = make_list(&[10, 20, 30]);
        let it = olive_iter(ptr);
        assert_ne!(it, 0);
        assert_eq!(olive_has_next(it), 1);
        assert_eq!(olive_next(it), 10);
        assert_eq!(olive_has_next(it), 1);
        assert_eq!(olive_next(it), 20);
        assert_eq!(olive_has_next(it), 1);
        assert_eq!(olive_next(it), 30);
        assert_eq!(olive_has_next(it), 0);
        assert_eq!(olive_next(it), 0);
        olive_free_iter(it);
    }

    #[test]
    fn iter_empty_list() {
        let ptr = make_list(&[]);
        let it = olive_iter(ptr);
        assert_eq!(olive_has_next(it), 0);
        assert_eq!(olive_next(it), 0);
        olive_free_iter(it);
    }
}
