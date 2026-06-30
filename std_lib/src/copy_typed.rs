//! Descriptor-driven deep copy, the mirror of `free_typed`.
//! D_ANY has no descriptor to bound it, so it copies via a worklist, cycles resolved by `COPY_VISITED`.

use crate::boxed::TAG_MASK;
use crate::format::{
    D_ANY, D_BACKREF, D_BYTES, D_DICT, D_ENUM, D_LIST, D_SET, D_STR, D_STRUCT, D_TUPLE, byte, skip,
};
use crate::slab::slot_is_live;
use crate::{
    KIND_ANY_LIST, KIND_BYTES, KIND_ENUM, KIND_FLOAT, KIND_INT, KIND_LIST, KIND_OBJ, KIND_PYOBJECT,
    KIND_SET, OliveEnum, OliveHashSet, OliveObj, OliveStringKey, StableVec,
};
use rustc_hash::FxHashMap;
use std::cell::RefCell;

thread_local! {
    // Tracks already-copied heap pointers to prevent infinite cycles.
    static COPY_VISITED: RefCell<FxHashMap<i64, i64>> = RefCell::new(FxHashMap::default());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_copy_typed(val: i64, desc: i64) -> i64 {
    COPY_VISITED.with(|v| {
        let mut visited = v.borrow_mut();
        visited.clear();
        let mut pos = 0usize;
        let res = copy_val(val, desc as *const u8, &mut pos, &mut visited);
        visited.clear();
        res
    })
}

/// A value crossing a task boundary (`chan_send`/`mutex_new`/`mutex_unlock`,
/// `lib/aio.liv`'s generic wrappers, driven by the compiler the same way
/// `__olive_copy_typed` is): the descriptor-driven deep copy above, run
/// inside `with_escape_arena` so the copy lands in the shared arena instead
/// of whatever task-local slab set happens to be active on the sending
/// side. A plain compile-time-typed copy is not enough on its own -- the
/// sending task's own slab set is torn down when that task completes
/// (`executor_drive`), which would use-after-free a copy left behind in it.
/// `D_ANY`'s own arm inside `copy_val` already falls back to runtime
/// kind-dispatch for a genuinely type-erased value, so this one function
/// covers both the typed and the `Any` case correctly -- no separate
/// kind-guessing path needed.
#[unsafe(no_mangle)]
pub extern "C" fn olive_relocate_typed(val: i64, desc: i64) -> i64 {
    crate::slab::with_escape_arena(|| olive_copy_typed(val, desc))
}

/// Concat for heap-element lists; deep-copies elements via `desc` (`[D_LIST, <elem>...]`) so operand drops can't double-free.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_concat_typed(l: i64, r: i64, desc: i64) -> i64 {
    let read = |v: i64| -> Option<(i64, *const i64, usize)> {
        if v == 0 || !slot_is_live(v) {
            return None;
        }
        let s = unsafe { &*(v as *const StableVec) };
        Some((s.kind, s.ptr, s.len))
    };
    let ls = read(l);
    let rs = read(r);
    if ls.is_none() && rs.is_none() {
        return 0;
    }
    let total = ls.map_or(0, |s| s.2) + rs.map_or(0, |s| s.2);
    let kind = ls.or(rs).map_or(KIND_LIST, |s| s.0);
    let new = crate::list::olive_list_new(total as i64);
    COPY_VISITED.with(|v| {
        let mut visited = v.borrow_mut();
        visited.clear();
        let mut out = 0i64;
        for (_, eptr, elen) in [ls, rs].into_iter().flatten() {
            for i in 0..elen {
                let mut pos = 1usize;
                let c = copy_val(
                    unsafe { *eptr.add(i) },
                    desc as *const u8,
                    &mut pos,
                    &mut visited,
                );
                crate::list::olive_list_set(new, out, c);
                out += 1;
            }
        }
        visited.clear();
    });
    unsafe { (*(new as *mut StableVec)).kind = kind };
    new
}

/// Slice for heap-element lists (see `olive_list_concat_typed`); preserves kind so `[Any]` stays self-describing.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_getslice_typed(
    ptr: i64,
    start: i64,
    stop: i64,
    step: i64,
    flags: i64,
    desc: i64,
) -> i64 {
    if ptr == 0 || !slot_is_live(ptr) {
        return crate::list::olive_list_new(0);
    }
    let (kind, eptr, elen) = unsafe {
        let s = &*(ptr as *const StableVec);
        (s.kind, s.ptr, s.len)
    };
    let idxs = crate::list::slice_indices(elen as i64, start, stop, step, flags);
    let new = crate::list::olive_list_new(idxs.len() as i64);
    COPY_VISITED.with(|v| {
        let mut visited = v.borrow_mut();
        visited.clear();
        for (j, &i) in idxs.iter().enumerate() {
            let mut pos = 1usize;
            let c = copy_val(
                unsafe { *eptr.add(i) },
                desc as *const u8,
                &mut pos,
                &mut visited,
            );
            crate::list::olive_list_set(new, j as i64, c);
        }
        visited.clear();
    });
    unsafe { (*(new as *mut StableVec)).kind = kind };
    new
}

/// `xs * n` for heap-element lists; every repetition gets its own deep copy
/// of each element, so `[[1]] * 3` yields three independent rows.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_repeat_typed(ptr: i64, n: i64, desc: i64) -> i64 {
    if n <= 0 || ptr == 0 || !slot_is_live(ptr) {
        return crate::list::olive_list_new(0);
    }
    let (kind, eptr, elen) = unsafe {
        let s = &*(ptr as *const StableVec);
        (s.kind, s.ptr, s.len)
    };
    let new = crate::list::olive_list_new(elen as i64 * n);
    COPY_VISITED.with(|v| {
        let mut visited = v.borrow_mut();
        let mut out = 0i64;
        for _ in 0..n {
            // Cleared per repetition, not per element: aliasing within one
            // tile stays intact (matches concat/getslice), but tile N+1 must
            // not reuse tile N's copies, or all tiles alias one row.
            visited.clear();
            for i in 0..elen {
                let mut pos = 1usize;
                let c = copy_val(
                    unsafe { *eptr.add(i) },
                    desc as *const u8,
                    &mut pos,
                    &mut visited,
                );
                crate::list::olive_list_set(new, out, c);
                out += 1;
            }
        }
        visited.clear();
    });
    unsafe { (*(new as *mut StableVec)).kind = kind };
    new
}

/// `d.update(other)` for heap-owning values: `other` keeps its own entries,
/// `d` gets independent copies. `desc` is `other`'s own `Dict(K, V)`
/// descriptor (`[D_DICT, <key-desc>, <value-desc>]`); the key descriptor is
/// skipped to find the value descriptor's start, mirroring `copy_dict`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_update_typed(obj_ptr: i64, other_ptr: i64, desc: i64) -> i64 {
    if obj_ptr == 0 || other_ptr == 0 || !slot_is_live(other_ptr) {
        return obj_ptr;
    }
    let entries: Vec<(i64, i64)> = {
        let om = unsafe { &*(other_ptr as *const OliveObj) };
        om.fields.iter().map(|(k, &v)| (k.0, v)).collect()
    };
    let mut key_pos = 1usize;
    crate::format::skip(desc as *const u8, &mut key_pos);
    let val_start = key_pos;
    COPY_VISITED.with(|v| {
        let mut visited = v.borrow_mut();
        visited.clear();
        for (k, v) in entries {
            let mut vp = val_start;
            let vc = copy_val(v, desc as *const u8, &mut vp, &mut visited);
            crate::obj::olive_obj_set(obj_ptr, k, vc);
        }
        visited.clear();
    });
    obj_ptr
}

/// Extend for heap-element lists; source keeps its elements, target appends copies.
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_extend_typed(target: i64, source: i64, desc: i64) {
    if target == 0 || source == 0 || !slot_is_live(source) {
        return;
    }
    let (eptr, elen) = unsafe {
        let s = &*(source as *const StableVec);
        (s.ptr, s.len)
    };
    COPY_VISITED.with(|v| {
        let mut visited = v.borrow_mut();
        visited.clear();
        for i in 0..elen {
            let mut pos = 1usize;
            let c = copy_val(
                unsafe { *eptr.add(i) },
                desc as *const u8,
                &mut pos,
                &mut visited,
            );
            crate::olive_list_append(target, c);
        }
        visited.clear();
    });
}

/// Skips a length-prefixed name; length byte is biased by 13.
fn skip_lp(desc: *const u8, pos: &mut usize) {
    let len = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1 + len;
}

fn copy_val(val: i64, desc: *const u8, pos: &mut usize, visited: &mut FxHashMap<i64, i64>) -> i64 {
    let cloned_opt = if val != 0 && crate::is_active_object(val) {
        visited.get(&val).copied()
    } else {
        None
    };
    if let Some(cloned) = cloned_opt {
        skip(desc, pos);
        return cloned;
    }
    let tag = unsafe { byte(desc, *pos) };
    *pos += 1;
    match tag {
        D_STR => copy_str(val),
        D_ANY | D_BYTES => copy_any(val, visited),
        D_LIST => copy_list_like(val, desc, pos, visited),
        D_SET => copy_set(val, desc, pos, visited),
        D_TUPLE => copy_tuple(val, desc, pos, visited),
        D_DICT => copy_dict(val, desc, pos, visited),
        D_STRUCT => copy_struct(val, desc, pos, visited),
        D_ENUM => copy_enum(val, desc, pos, visited),
        D_BACKREF => {
            let hi = unsafe { byte(desc, *pos) } as usize;
            let lo = unsafe { byte(desc, *pos + 1) } as usize;
            *pos += 2;
            let mut target_pos = (hi << 8) | lo;
            copy_val(val, desc, &mut target_pos, visited)
        }
        // Scalars and the recursion-cut `D_OBJ` share the word by value.
        _ => val,
    }
}

/// A tagged pointer is a literal or heap string and clones to a fresh heap
/// string; an untagged word is an interned symbol or a scalar and is shared.
fn copy_str(val: i64) -> i64 {
    if val == 0 || val & 1 == 0 {
        return val;
    }
    crate::olive_copy(val)
}

fn copy_list_like(
    val: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashMap<i64, i64>,
) -> i64 {
    let inner_start = *pos;
    skip(desc, pos);
    if val == 0 || !slot_is_live(val) {
        return val;
    }
    let (kind, eptr, elen) = unsafe {
        let s = &*(val as *const StableVec);
        if s.kind == KIND_SET {
            return copy_set_at(val, desc, inner_start, visited);
        }
        (s.kind, s.ptr, s.len)
    };
    let new = crate::list::olive_list_new(elen as i64);
    visited.insert(val, new);
    for i in 0..elen {
        let mut p = inner_start;
        let c = copy_val(unsafe { *eptr.add(i) }, desc, &mut p, visited);
        crate::list::olive_list_set(new, i as i64, c);
    }
    unsafe { (*(new as *mut StableVec)).kind = kind };
    new
}

fn copy_set(val: i64, desc: *const u8, pos: &mut usize, visited: &mut FxHashMap<i64, i64>) -> i64 {
    let inner_start = *pos;
    skip(desc, pos);
    if val == 0 || !slot_is_live(val) {
        return val;
    }
    copy_set_at(val, desc, inner_start, visited)
}

fn copy_set_at(
    val: i64,
    desc: *const u8,
    inner_start: usize,
    visited: &mut FxHashMap<i64, i64>,
) -> i64 {
    let (eptr, elen) = unsafe {
        let s = &*(val as *const OliveHashSet);
        (s.ptr, s.len)
    };
    let new = crate::set::olive_set_new(elen as i64);
    visited.insert(val, new);
    for i in 0..elen {
        let mut p = inner_start;
        let c = copy_val(unsafe { *eptr.add(i) }, desc, &mut p, visited);
        crate::set::olive_set_add(new, c);
    }
    new
}

fn copy_tuple(
    val: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashMap<i64, i64>,
) -> i64 {
    let n = unsafe { byte(desc, *pos) } as usize - 1;
    *pos += 1;
    if val == 0 || !slot_is_live(val) {
        for _ in 0..n {
            skip(desc, pos);
        }
        return val;
    }
    let (eptr, elen) = unsafe {
        let s = &*(val as *const StableVec);
        (s.ptr, s.len)
    };
    let new = crate::list::olive_list_new(n as i64);
    visited.insert(val, new);
    for i in 0..n {
        let elem = if i < elen { unsafe { *eptr.add(i) } } else { 0 };
        let c = copy_val(elem, desc, pos, visited);
        crate::list::olive_list_set(new, i as i64, c);
    }
    new
}

fn copy_dict(val: i64, desc: *const u8, pos: &mut usize, visited: &mut FxHashMap<i64, i64>) -> i64 {
    let key_start = *pos;
    skip(desc, pos);
    let val_start = *pos;
    skip(desc, pos);
    if val == 0 || !slot_is_live(val) {
        return val;
    }
    let obj = unsafe { &*(val as *const OliveObj) };
    let new = crate::obj::olive_obj_new();
    visited.insert(val, new);
    let mut fields = FxHashMap::default();
    for (k, &v) in obj.fields.iter() {
        let mut kp = key_start;
        let kc = copy_val(k.0, desc, &mut kp, visited);
        let mut vp = val_start;
        let vc = copy_val(v, desc, &mut vp, visited);
        fields.insert(OliveStringKey(kc), vc);
    }
    unsafe { (*(new as *mut OliveObj)).fields = fields };
    new
}

fn copy_struct(
    val: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashMap<i64, i64>,
) -> i64 {
    skip_lp(desc, pos);
    let n = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1;
    if val == 0 || !slot_is_live(val) {
        for _ in 0..n {
            skip_lp(desc, pos);
            skip(desc, pos);
        }
        return val;
    }
    let n_fields = unsafe { *(val as *const i64) };
    let new = crate::olive_struct_alloc(n_fields);
    visited.insert(val, new);
    for i in 0..n {
        skip_lp(desc, pos);
        let field = if (i as i64) < n_fields {
            unsafe { *((val + 8 + 8 * i as i64) as *const i64) }
        } else {
            0
        };
        let c = copy_val(field, desc, pos, visited);
        if (i as i64) < n_fields {
            unsafe { *((new + 8 + 8 * i as i64) as *mut i64) = c };
        }
    }
    new
}

fn copy_enum(val: i64, desc: *const u8, pos: &mut usize, visited: &mut FxHashMap<i64, i64>) -> i64 {
    skip_lp(desc, pos);
    let n = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1;
    let live = val != 0 && slot_is_live(val);
    let (type_id, tag, pptr, plen) = if live {
        let e = unsafe { &*(val as *const OliveEnum) };
        (e.type_id, e.tag as usize, e.payload_ptr, e.payload_len)
    } else {
        (0, usize::MAX, std::ptr::null_mut(), 0)
    };
    let new = if live {
        crate::olive_enum_new(type_id, tag as i64, plen as i64)
    } else {
        0
    };
    if live {
        visited.insert(val, new);
    }
    for i in 0..n {
        skip_lp(desc, pos);
        let np = unsafe { byte(desc, *pos) } as usize - 13;
        *pos += 1;
        for j in 0..np {
            if i == tag && j < plen {
                let c = copy_val(unsafe { *pptr.add(j) }, desc, pos, visited);
                crate::olive_enum_set(new, j as i64, c);
            } else {
                skip(desc, pos);
            }
        }
    }
    if live { new } else { val }
}

/// Where a pending `D_ANY` child's copy gets written once computed.
enum AnyDest {
    Root,
    List(i64, i64),
    Set(i64),
    ObjField(i64, i64),
    EnumSlot(i64, i64),
}

/// Kind-driven deep copy of a statically-`Any` word, the mirror of the
/// kind-driven deep free `olive_free_any` drives. Iterative: each container
/// allocates its (empty) copy and records it in `COPY_VISITED` before its
/// children are queued, so a cycle resolves to the already-allocated shell
/// exactly as the recursive form did, but the walk is bounded by heap, not
/// by call-stack depth.
pub(crate) fn copy_any(val: i64, visited: &mut FxHashMap<i64, i64>) -> i64 {
    copy_any_impl(val, visited, false)
}

/// Native ints can be odd too; gate string check on slab membership.
fn copy_boundary(val: i64, visited: &mut FxHashMap<i64, i64>) -> i64 {
    copy_any_impl(val, visited, true)
}

fn copy_any_impl(val: i64, visited: &mut FxHashMap<i64, i64>, boundary: bool) -> i64 {
    let mut root = 0i64;
    let mut stack: Vec<(i64, AnyDest)> = vec![(val, AnyDest::Root)];
    while let Some((src, dest)) = stack.pop() {
        let copied = copy_any_node(src, &mut stack, visited, boundary);
        match dest {
            AnyDest::Root => root = copied,
            AnyDest::List(list, i) => crate::list::olive_list_set(list, i, copied),
            AnyDest::Set(set) => crate::set::olive_set_add(set, copied),
            AnyDest::ObjField(obj, key) => unsafe {
                (*(obj as *mut OliveObj))
                    .fields
                    .insert(OliveStringKey(key), copied);
            },
            AnyDest::EnumSlot(en, i) => crate::olive_enum_set(en, i, copied),
        }
    }
    root
}

/// Copies val into GLOBAL_SLABS before it crosses; copy semantics, original untouched.
pub(crate) fn relocate_across_boundary(val: i64) -> i64 {
    crate::slab::with_escape_arena(|| copy_boundary(val, &mut FxHashMap::default()))
}

/// Copies one `D_ANY` node. Leaves resolve immediately; a container
/// allocates its shell, marks it visited, and pushes `(child, dest)` work
/// for each element so the caller's stack keeps walking instead of recursing.
fn copy_any_node(
    val: i64,
    stack: &mut Vec<(i64, AnyDest)>,
    visited: &mut FxHashMap<i64, i64>,
    boundary: bool,
) -> i64 {
    if val == 0 {
        return val;
    }
    if val & 1 != 0 {
        if boundary && !crate::string_slab::str_is_heap(val) {
            return val;
        }
        return crate::olive_copy(val);
    }
    if val & TAG_MASK != 0 {
        return val;
    }
    if !crate::is_active_object(val) {
        return val;
    }
    if let Some(cloned) = visited.get(&val).copied() {
        return cloned;
    }
    let kind = unsafe { *(val as *const i64) };
    match kind {
        KIND_LIST | KIND_ANY_LIST => {
            let (eptr, elen) = unsafe {
                let s = &*(val as *const StableVec);
                (s.ptr, s.len)
            };
            let new = crate::list::olive_list_new(elen as i64);
            visited.insert(val, new);
            for i in (0..elen).rev() {
                stack.push((unsafe { *eptr.add(i) }, AnyDest::List(new, i as i64)));
            }
            unsafe { (*(new as *mut StableVec)).kind = kind };
            new
        }
        KIND_SET => {
            let (eptr, elen) = unsafe {
                let s = &*(val as *const OliveHashSet);
                (s.ptr, s.len)
            };
            let new = crate::set::olive_set_new(elen as i64);
            visited.insert(val, new);
            for i in (0..elen).rev() {
                stack.push((unsafe { *eptr.add(i) }, AnyDest::Set(new)));
            }
            new
        }
        KIND_OBJ => {
            let obj = unsafe { &*(val as *const OliveObj) };
            let new = crate::obj::olive_obj_new();
            visited.insert(val, new);
            for (k, &v) in obj.fields.iter() {
                stack.push((v, AnyDest::ObjField(new, copy_str(k.0))));
            }
            new
        }
        KIND_ENUM => {
            let e = unsafe { &*(val as *const OliveEnum) };
            let new = crate::olive_enum_new(e.type_id, e.tag, e.payload_len as i64);
            visited.insert(val, new);
            for j in (0..e.payload_len).rev() {
                stack.push((
                    unsafe { *e.payload_ptr.add(j) },
                    AnyDest::EnumSlot(new, j as i64),
                ));
            }
            new
        }
        KIND_FLOAT => {
            let bits = unsafe { (*(val as *const crate::boxed::OliveBoxed)).bits };
            crate::boxed::olive_box_float(f64::from_bits(bits as u64))
        }
        KIND_INT => {
            let bits = unsafe { (*(val as *const crate::boxed::OliveBoxed)).bits };
            crate::boxed::olive_box_int(bits)
        }
        KIND_BYTES => crate::bytes::clone_buf(val),
        // Wrap a fresh handle rather than incref in place; that would clobber the kind field (CPython's ob_refcnt slot).
        KIND_PYOBJECT => {
            let py_ptr = unsafe { (*(val as *const crate::python::OlivePyObject)).py_ptr };
            unsafe { crate::python::olive_py_wrap_borrowed(py_ptr) as i64 }
        }
        crate::struct_box::KIND_STRUCT_BOX => {
            let (desc, inner) = {
                let b = unsafe { &*(val as *const crate::struct_box::OliveStructBox) };
                (b.desc, b.ptr)
            };
            // Shell first so a cycle back through this box resolves to it.
            let new = crate::struct_box::alloc_shell(desc);
            visited.insert(val, new);
            let mut pos = 0usize;
            let copied = copy_val(inner, desc as *const u8, &mut pos, visited);
            crate::struct_box::set_inner(new, copied);
            new
        }
        _ => val,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::D_INT;
    use crate::free_typed::olive_free_typed;
    use crate::list::{list_from_vec, olive_list_get};
    use crate::obj::{olive_obj_get, olive_obj_new, olive_obj_set};

    fn desc(bytes: &[u8]) -> i64 {
        Box::leak(bytes.to_vec().into_boxed_slice()).as_ptr() as i64
    }

    fn s(text: &str) -> i64 {
        crate::olive_str_internal(text)
    }

    fn read(ptr: i64) -> String {
        crate::olive_str_from_ptr(ptr)
    }

    #[test]
    fn heap_str_copy_is_independent() {
        let a = s("hello");
        let b = olive_copy_typed(a, desc(&[D_STR]));
        assert_ne!(crate::string_slab::str_body(a), crate::string_slab::str_body(b), "distinct heap slots");
        crate::olive_free_str(a);
        assert_eq!(read(b), "hello", "copy survives source free");
        crate::olive_free_str(b);
    }

    #[test]
    fn list_of_str_copy_survives_source_free() {
        let src = list_from_vec(vec![s("val0"), s("val1")]);
        let d = desc(&[D_LIST, D_STR]);
        let cp = olive_copy_typed(src, d);
        olive_free_typed(src, d);
        assert_eq!(read(olive_list_get(cp, 0)), "val0");
        assert_eq!(read(olive_list_get(cp, 1)), "val1");
        olive_free_typed(cp, d);
    }

    #[test]
    fn nested_list_deep_copied() {
        let inner = list_from_vec(vec![s("x")]);
        let outer = list_from_vec(vec![inner]);
        let d = desc(&[D_LIST, D_LIST, D_STR]);
        let cp = olive_copy_typed(outer, d);
        let cp_inner = olive_list_get(cp, 0);
        assert_ne!(cp_inner, inner, "inner list is a fresh slot");
        olive_free_typed(outer, d);
        assert_eq!(read(olive_list_get(cp_inner, 0)), "x");
        olive_free_typed(cp, d);
    }

    #[test]
    fn scalar_list_copies_by_value() {
        let src = list_from_vec(vec![10, 20, 30]);
        let d = desc(&[D_LIST, D_INT]);
        let cp = olive_copy_typed(src, d);
        assert_ne!(cp, src);
        assert_eq!(olive_list_get(cp, 1), 20);
        olive_free_typed(src, d);
        assert_eq!(olive_list_get(cp, 2), 30, "values independent of source");
        olive_free_typed(cp, d);
    }

    #[test]
    fn dict_str_values_deep_copied() {
        let d = olive_obj_new();
        olive_obj_set(d, s("k"), s("v"));
        let dd = desc(&[D_DICT, D_STR, D_STR]);
        let cp = olive_copy_typed(d, dd);
        olive_free_typed(d, dd);
        assert_eq!(read(olive_obj_get(cp, s("k"))), "v");
        olive_free_typed(cp, dd);
    }

    #[test]
    fn set_str_deep_copied() {
        let src = crate::set::olive_set_new(2);
        let elem = s("a");
        crate::set::olive_set_add(src, elem);
        let d = desc(&[D_SET, D_STR]);
        let cp = olive_copy_typed(src, d);
        let items = crate::set::olive_set_items(cp);
        let copied = olive_list_get(items, 0);
        assert_ne!(crate::string_slab::str_body(copied), crate::string_slab::str_body(elem), "element is a fresh heap slot");
        olive_free_typed(src, d);
        assert_eq!(read(copied), "a", "copied element survives source free");
        olive_free_typed(cp, d);
    }

    #[test]
    fn struct_str_field_deep_copied() {
        let st = crate::olive_struct_alloc(2);
        unsafe {
            *((st + 8) as *mut i64) = 7;
            *((st + 16) as *mut i64) = s("field");
        }
        let dp = desc(&[
            D_STRUCT,
            13 + 1,
            b'P',
            13 + 2,
            13 + 1,
            b'a',
            D_INT,
            13 + 1,
            b'b',
            D_STR,
        ]);
        let cp = olive_copy_typed(st, dp);
        olive_free_typed(st, dp);
        assert_eq!(unsafe { *((cp + 8) as *const i64) }, 7);
        assert_eq!(read(unsafe { *((cp + 16) as *const i64) }), "field");
        olive_free_typed(cp, dp);
    }

    #[test]
    fn any_scalar_copies_by_value() {
        let boxed = crate::boxed::olive_box_int(42);
        let cp = olive_copy_typed(boxed, desc(&[D_ANY]));
        assert_eq!(cp, boxed, "inline immediate shares its word");
    }

    #[test]
    fn any_object_deep_copied() {
        let inner = list_from_vec(vec![s("deep")]);
        let src = list_from_vec(vec![inner]);
        let d = desc(&[D_ANY]);
        let cp = olive_copy_typed(src, d);
        assert_ne!(cp, src);
        let cp_inner = olive_list_get(cp, 0);
        assert_ne!(cp_inner, inner);
        crate::olive_free_any(src);
        assert_eq!(read(olive_list_get(cp_inner, 0)), "deep");
        crate::olive_free_any(cp);
    }

    #[test]
    fn any_deep_nest_has_no_depth_cap() {
        // 1000 levels comfortably clears the old 512 cap; every level must be
        // its own independent list, not the source shared past the cutoff.
        let mut cur = list_from_vec(vec![s("leaf")]);
        for _ in 0..1000 {
            cur = list_from_vec(vec![cur]);
        }
        let cp = olive_copy_typed(cur, desc(&[D_ANY]));

        let mut o = cur;
        let mut c = cp;
        for _ in 0..1000 {
            assert_ne!(o, c, "every nesting level must be a fresh list");
            o = olive_list_get(o, 0);
            c = olive_list_get(c, 0);
        }
        crate::list::olive_list_set(o, 0, s("mutated"));
        assert_eq!(
            read(olive_list_get(c, 0)),
            "leaf",
            "copy independent of a mutation at the innermost level"
        );

        crate::olive_free_any(cur);
        crate::olive_free_any(cp);
    }

    #[test]
    fn any_list_cycle_round_trips() {
        let src = list_from_vec(vec![0i64]);
        unsafe { (*(src as *mut StableVec)).kind = KIND_ANY_LIST };
        crate::list::olive_list_set(src, 0, src);

        let cp = olive_copy_typed(src, desc(&[D_ANY]));
        assert_ne!(cp, src, "copy is a fresh list");
        let cp_inner = olive_list_get(cp, 0);
        assert_eq!(cp, cp_inner, "cycle must preserve identity in the copy");

        // Break both cycles before freeing so free_any's own walk terminates.
        crate::list::olive_list_set(src, 0, 0);
        crate::list::olive_list_set(cp, 0, 0);
        crate::olive_free_any(src);
        crate::olive_free_any(cp);
    }

    #[test]
    fn recursive_struct_deep_copied() {
        let val1 = s("hello");
        let val2 = s("world");
        let node1 = crate::olive_struct_alloc(2);
        let node2 = crate::olive_struct_alloc(2);
        unsafe {
            *((node1 + 8) as *mut i64) = val1;
            *((node1 + 16) as *mut i64) = node2;
            *((node2 + 8) as *mut i64) = val2;
            *((node2 + 16) as *mut i64) = 0;
        }
        let d = desc(&[
            D_STRUCT,
            13 + 4,
            b'N',
            b'o',
            b'd',
            b'e',
            13 + 2,
            13 + 3,
            b'v',
            b'a',
            b'l',
            D_STR,
            13 + 4,
            b'n',
            b'e',
            b'x',
            b't',
            D_BACKREF,
            0,
            0,
        ]);
        let cp = olive_copy_typed(node1, d);
        assert_ne!(cp, node1);
        let cp_node2 = unsafe { *((cp + 16) as *const i64) };
        assert_ne!(cp_node2, node2);
        let cp_val1 = unsafe { *((cp + 8) as *const i64) };
        let cp_val2 = unsafe { *((cp_node2 + 8) as *const i64) };
        assert_ne!(cp_val1, val1);
        assert_ne!(cp_val2, val2);

        olive_free_typed(node1, d);
        assert_eq!(read(cp_val1), "hello");
        assert_eq!(read(cp_val2), "world");
        olive_free_typed(cp, d);
    }

    #[test]
    fn cyclic_struct_copy_handles_cycles() {
        let val1 = s("cyclic");
        let node1 = crate::olive_struct_alloc(2);
        unsafe {
            *((node1 + 8) as *mut i64) = val1;
            *((node1 + 16) as *mut i64) = node1;
        }
        let d = desc(&[
            D_STRUCT,
            13 + 4,
            b'N',
            b'o',
            b'd',
            b'e',
            13 + 2,
            13 + 3,
            b'v',
            b'a',
            b'l',
            D_STR,
            13 + 4,
            b'n',
            b'e',
            b'x',
            b't',
            D_BACKREF,
            0,
            0,
        ]);
        let cp = olive_copy_typed(node1, d);
        assert_ne!(cp, node1);
        let cp_next = unsafe { *((cp + 16) as *const i64) };
        assert_eq!(cp, cp_next, "cycle must preserve identity in copy");

        olive_free_typed(node1, d);
        assert_eq!(read(unsafe { *((cp + 8) as *const i64) }), "cyclic");
        olive_free_typed(cp, d);
    }

    #[test]
    fn concat_typed_survives_operand_free() {
        let d = desc(&[D_LIST, D_STR]);
        let a = list_from_vec(vec![s("left0")]);
        let b = list_from_vec(vec![s("right0"), s("right1")]);
        let cat = olive_list_concat_typed(a, b, d);
        olive_free_typed(a, d);
        olive_free_typed(b, d);
        assert_eq!(read(olive_list_get(cat, 0)), "left0");
        assert_eq!(read(olive_list_get(cat, 1)), "right0");
        assert_eq!(read(olive_list_get(cat, 2)), "right1");
        olive_free_typed(cat, d);
    }

    #[test]
    fn concat_typed_null_side() {
        let d = desc(&[D_LIST, D_STR]);
        let b = list_from_vec(vec![s("only")]);
        let cat = olive_list_concat_typed(0, b, d);
        olive_free_typed(b, d);
        assert_eq!(read(olive_list_get(cat, 0)), "only");
        olive_free_typed(cat, d);
        assert_eq!(olive_list_concat_typed(0, 0, d), 0);
    }

    #[test]
    fn getslice_typed_survives_source_free() {
        let d = desc(&[D_LIST, D_STR]);
        let src = list_from_vec(vec![s("a"), s("b"), s("c")]);
        // [1:] with start flag only
        let sub = olive_list_getslice_typed(src, 1, 0, 0, 1, d);
        olive_free_typed(src, d);
        assert_eq!(read(olive_list_get(sub, 0)), "b");
        assert_eq!(read(olive_list_get(sub, 1)), "c");
        olive_free_typed(sub, d);
    }

    #[test]
    fn getslice_typed_preserves_kind() {
        let d = desc(&[D_LIST, D_ANY]);
        let src = list_from_vec(vec![crate::boxed::olive_box_int(1)]);
        unsafe { (*(src as *mut StableVec)).kind = KIND_ANY_LIST };
        let sub = olive_list_getslice_typed(src, 0, 0, 0, 0, d);
        assert_eq!(unsafe { (*(sub as *const StableVec)).kind }, KIND_ANY_LIST);
        olive_free_typed(src, d);
        olive_free_typed(sub, d);
    }

    #[test]
    fn extend_typed_survives_source_free() {
        let d = desc(&[D_LIST, D_STR]);
        let target = list_from_vec(vec![s("t0")]);
        let source = list_from_vec(vec![s("s0"), s("s1")]);
        olive_list_extend_typed(target, source, d);
        olive_free_typed(source, d);
        assert_eq!(read(olive_list_get(target, 0)), "t0");
        assert_eq!(read(olive_list_get(target, 1)), "s0");
        assert_eq!(read(olive_list_get(target, 2)), "s1");
        olive_free_typed(target, d);
    }

    #[test]
    fn bytes_desc_deep_copies_buffer() {
        let src = crate::bytes::new_buf(vec![1, 2, 3]);
        let d = desc(&[D_BYTES]);
        let cp = olive_copy_typed(src, d);
        assert_ne!(cp, src, "buffer is a fresh slot");
        olive_free_typed(src, d);
        assert_eq!(crate::bytes::olive_buf_len(cp), 3);
        assert_eq!(crate::bytes::olive_buf_get(cp, 1), 2);
        olive_free_typed(cp, d);
    }

    #[test]
    fn py_handle_copy_keeps_handle_kind_intact() {
        // Old path ran Py_IncRef on the handle itself, silently rewriting KIND_PYOBJECT.
        let fake_py = Box::leak(Box::new([0i64; 4])) as *mut _ as *mut std::ffi::c_void;
        let h = unsafe { crate::python::olive_py_wrap_owned(fake_py) } as i64;
        let cp = olive_copy_typed(h, desc(&[D_ANY]));
        assert_eq!(unsafe { *(h as *const i64) }, KIND_PYOBJECT);
        assert_ne!(cp, h, "copy is a fresh handle");
        assert_eq!(unsafe { *(cp as *const i64) }, KIND_PYOBJECT);
    }
}
