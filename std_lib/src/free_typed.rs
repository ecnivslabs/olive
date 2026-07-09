//! Descriptor-driven deep free. The compiler passes the static type of a
//! dropped value as the same byte descriptor `olive_format_typed` consumes,
//! so ownership of elements, fields, and payloads is released without the
//! runtime guessing what a raw word means.
//!
//! Slots are marked freed before their children are walked, so a data cycle
//! terminates at the generation guard instead of recursing forever.
//!
//! A stored string is freed through `olive_free_str`, which classifies the
//! pointer: a read-only literal is not in any slab chunk and frees as a no-op,
//! while a heap string returns to its size class. `D_OBJ` (a recursion-cut or
//! unregistered aggregate) is skipped, since no layout is known for it.

use crate::boxed::TAG_MASK;
use crate::format::{
    D_ANY, D_BACKREF, D_BYTES, D_DICT, D_ENUM, D_LIST, D_SET, D_STR, D_STRUCT, D_TUPLE, byte, skip,
};
use crate::slab::slot_is_live;
use crate::{OliveEnum, OliveHashSet, OliveObj, StableVec};

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_typed(val: i64, desc: i64) {
    let mut pos = 0usize;
    free_val(val, desc as *const u8, &mut pos);
}

/// Skips a length-prefixed name; length byte is biased by 13.
fn skip_lp(desc: *const u8, pos: &mut usize) {
    let len = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1 + len;
}

fn free_val(val: i64, desc: *const u8, pos: &mut usize) {
    let tag = unsafe { byte(desc, *pos) };
    *pos += 1;
    match tag {
        D_ANY => free_any_elem(val),
        D_STR => crate::olive_free_str(val),
        D_BYTES => crate::bytes::olive_buf_free(val),
        D_LIST => free_list_like(val, desc, pos),
        D_SET => free_set(val, desc, pos),
        D_TUPLE => free_tuple(val, desc, pos),
        D_DICT => free_dict(val, desc, pos),
        D_STRUCT => free_struct(val, desc, pos),
        D_ENUM => free_enum(val, desc, pos),
        D_BACKREF => {
            let hi = unsafe { byte(desc, *pos) } as usize;
            let lo = unsafe { byte(desc, *pos + 1) } as usize;
            *pos += 2;
            let mut target_pos = (hi << 8) | lo;
            free_val(val, desc, &mut target_pos);
        }
        _ => {}
    }
}

/// A statically-`Any` slot can still hold a raw scalar (a `Param` that
/// defaulted to `Any` stores unboxed), so a bare word is only kind-dispatched
/// when it classifies as a live object pointer. Tagged immediates own
/// nothing; a tagged string may be interned and is never freed.
fn free_any_elem(val: i64) {
    if val == 0 || val & TAG_MASK != 0 {
        return;
    }
    if crate::is_active_object(val) {
        crate::olive_free_any(val);
    }
}

fn free_list_like(val: i64, desc: *const u8, pos: &mut usize) {
    let inner_start = *pos;
    skip(desc, pos);
    if val == 0 || !slot_is_live(val) {
        return;
    }
    let (eptr, elen, ecap) = unsafe {
        let s = &mut *(val as *mut StableVec);
        if s.kind == crate::KIND_SET {
            // A set-typed value that reached a list descriptor (e.g. through
            // inference); elements are scalars or shared words, free storage.
            crate::set::release_set_storage(val);
            crate::set::free_set_slot_raw(val);
            return;
        }
        let res = (s.ptr, s.len, s.cap);
        if s.cap > crate::list::RETAIN_CAP {
            s.ptr = std::ptr::null_mut();
            s.cap = 0;
        }
        res
    };
    crate::list::free_list_slot_raw(val);
    free_elems(eptr, elen, desc, inner_start);
    if ecap > crate::list::RETAIN_CAP && !eptr.is_null() {
        let _ = unsafe { Vec::from_raw_parts(eptr, 0, ecap) };
    }
}

fn free_elems(eptr: *const i64, elen: usize, desc: *const u8, inner_start: usize) {
    match unsafe { byte(desc, inner_start) } {
        // Direct loop keeps the common Any-element walk out of the recursion.
        D_ANY => {
            for i in 0..elen {
                free_any_elem(unsafe { *eptr.add(i) });
            }
        }
        D_STR => {
            for i in 0..elen {
                crate::olive_free_str(unsafe { *eptr.add(i) });
            }
        }
        D_BYTES => {
            for i in 0..elen {
                crate::bytes::olive_buf_free(unsafe { *eptr.add(i) });
            }
        }
        D_LIST | D_SET | D_TUPLE | D_DICT | D_STRUCT | D_ENUM => {
            for i in 0..elen {
                let mut p = inner_start;
                free_val(unsafe { *eptr.add(i) }, desc, &mut p);
            }
        }
        _ => {}
    }
}

fn free_set(val: i64, desc: *const u8, pos: &mut usize) {
    let inner_start = *pos;
    skip(desc, pos);
    if val == 0 || !slot_is_live(val) {
        return;
    }
    let (eptr, elen, ecap, einner) = unsafe {
        let s = &*(val as *const OliveHashSet);
        (s.ptr, s.len, s.cap, s.inner)
    };
    crate::set::free_set_slot_raw(val);
    free_elems(eptr, elen, desc, inner_start);
    unsafe {
        if !eptr.is_null() {
            let _ = Vec::from_raw_parts(eptr, elen, ecap);
        }
        if !einner.is_null() {
            let _ = Box::from_raw(einner);
        }
    }
}

fn free_tuple(val: i64, desc: *const u8, pos: &mut usize) {
    let n = unsafe { byte(desc, *pos) } as usize - 1;
    *pos += 1;
    if val == 0 || !slot_is_live(val) {
        for _ in 0..n {
            skip(desc, pos);
        }
        return;
    }
    let (eptr, elen, ecap) = unsafe {
        let s = &mut *(val as *mut StableVec);
        let res = (s.ptr, s.len, s.cap);
        if s.cap > crate::list::RETAIN_CAP {
            s.ptr = std::ptr::null_mut();
            s.cap = 0;
        }
        res
    };
    crate::list::free_list_slot_raw(val);
    for i in 0..n {
        let elem = if i < elen { unsafe { *eptr.add(i) } } else { 0 };
        free_val(elem, desc, pos);
    }
    if ecap > crate::list::RETAIN_CAP && !eptr.is_null() {
        let _ = unsafe { Vec::from_raw_parts(eptr, 0, ecap) };
    }
}

fn free_dict(val: i64, desc: *const u8, pos: &mut usize) {
    skip(desc, pos);
    let val_start = *pos;
    skip(desc, pos);
    if val == 0 || !slot_is_live(val) {
        return;
    }
    let fields = unsafe {
        let obj = &mut *(val as *mut OliveObj);
        std::mem::take(&mut obj.fields)
    };
    crate::obj::free_obj_slot_raw(val);
    if elem_owns(desc, val_start) {
        for &v in fields.values() {
            let mut p = val_start;
            free_val(v, desc, &mut p);
        }
    }
    // Tagged keys are dict-owned string copies; free them. Untagged attribute
    // names are read-only interned symbols and classify as no-ops anyway.
    for k in fields.keys() {
        if k.0 & 1 != 0 {
            crate::olive_free_str(k.0);
        }
    }
}

fn free_struct(val: i64, desc: *const u8, pos: &mut usize) {
    skip_lp(desc, pos);
    let n = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1;
    if val == 0 || !slot_is_live(val) {
        for _ in 0..n {
            skip_lp(desc, pos);
            skip(desc, pos);
        }
        return;
    }
    let n_fields = unsafe { *(val as *const i64) };
    let fields = unsafe {
        let mut f = Vec::with_capacity(n_fields as usize);
        for i in 0..n_fields {
            f.push(*((val + 8 + 8 * i) as *const i64));
        }
        f
    };
    crate::struct_obj::free_struct_slot_raw(val, n_fields);
    for i in 0..n {
        skip_lp(desc, pos);
        let field = if i < fields.len() { fields[i] } else { 0 };
        free_val(field, desc, pos);
    }
}

fn free_enum(val: i64, desc: *const u8, pos: &mut usize) {
    skip_lp(desc, pos);
    let n = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1;
    let live = val != 0 && slot_is_live(val);
    let (tag, pptr, plen) = if live {
        let e = unsafe { &*(val as *const OliveEnum) };
        (e.tag as usize, e.payload_ptr, e.payload_len)
    } else {
        (usize::MAX, std::ptr::null_mut(), 0)
    };
    if live {
        crate::enum_obj::free_enum_slot_raw(val);
    }
    for i in 0..n {
        skip_lp(desc, pos);
        let np = unsafe { byte(desc, *pos) } as usize - 13;
        *pos += 1;
        for j in 0..np {
            if i == tag && j < plen {
                free_val(unsafe { *pptr.add(j) }, desc, pos);
            } else {
                skip(desc, pos);
            }
        }
    }
    if live && !pptr.is_null() {
        let _ = unsafe { Vec::from_raw_parts(pptr, plen, plen) };
    }
}

/// Whether elements of this descriptor class can own heap memory; scalar and
/// string elements make the per-element walk pointless.
fn elem_owns(desc: *const u8, pos: usize) -> bool {
    matches!(
        unsafe { byte(desc, pos) },
        D_ANY | D_BACKREF | D_BYTES | D_LIST | D_SET | D_TUPLE | D_DICT | D_STRUCT | D_ENUM
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::{list_from_vec, olive_list_get, olive_list_new};
    use crate::obj::{olive_obj_new, olive_obj_set};
    use crate::slab::slot_is_live;

    // Descriptor bytes mirror imports::type_descriptor encoding.
    fn desc(bytes: &[u8]) -> i64 {
        Box::leak(bytes.to_vec().into_boxed_slice()).as_ptr() as i64
    }

    #[test]
    fn scalar_list_slot_freed_no_elem_walk() {
        let l = list_from_vec(vec![1, 2, 3]);
        olive_free_typed(l, desc(&[D_LIST, 1]));
        assert!(!slot_is_live(l));
    }

    #[test]
    fn nested_list_children_freed() {
        let inner = list_from_vec(vec![7, 8]);
        let outer = list_from_vec(vec![inner]);
        olive_free_typed(outer, desc(&[D_LIST, D_LIST, 1]));
        assert!(!slot_is_live(outer));
        assert!(!slot_is_live(inner));
    }

    #[test]
    fn double_free_is_noop() {
        let l = list_from_vec(vec![1]);
        olive_free_typed(l, desc(&[D_LIST, 1]));
        olive_free_typed(l, desc(&[D_LIST, 1]));
        assert!(!slot_is_live(l));
    }

    #[test]
    fn shared_child_freed_once() {
        let child = list_from_vec(vec![1]);
        let a = list_from_vec(vec![child]);
        let b = list_from_vec(vec![child]);
        olive_free_typed(a, desc(&[D_LIST, D_LIST, 1]));
        assert!(!slot_is_live(child));
        // The second owner's free must not touch the recycled slot twice.
        olive_free_typed(b, desc(&[D_LIST, D_LIST, 1]));
        assert!(!slot_is_live(b));
    }

    #[test]
    fn cycle_terminates() {
        let l = olive_list_new(1);
        crate::olive_list_set(l, 0, l);
        olive_free_typed(l, desc(&[D_LIST, D_LIST, D_LIST, 1]));
        assert!(!slot_is_live(l));
    }

    #[test]
    fn dict_values_freed_keys_kept() {
        let inner = list_from_vec(vec![5]);
        let d = olive_obj_new();
        let key = crate::olive_str_internal("k");
        olive_obj_set(d, key, inner);
        olive_free_typed(d, desc(&[D_DICT, crate::format::D_STR, D_LIST, 1]));
        assert!(!slot_is_live(d));
        assert!(!slot_is_live(inner));
    }

    #[test]
    fn heap_str_elements_freed() {
        let s = crate::olive_str_internal("alive");
        let l = list_from_vec(vec![s]);
        olive_free_typed(l, desc(&[D_LIST, crate::format::D_STR]));
        assert!(!slot_is_live(l));
        assert!(!crate::slab::ptr_is_slab_body(s & !1), "heap string freed");
    }

    #[test]
    fn tuple_mixed_elements() {
        let inner = list_from_vec(vec![9]);
        let t = list_from_vec(vec![42, inner]);
        olive_free_typed(t, desc(&[D_TUPLE, 3, 1, D_LIST, 1]));
        assert!(!slot_is_live(t));
        assert!(!slot_is_live(inner));
    }

    #[test]
    fn struct_heap_fields_freed() {
        let inner = list_from_vec(vec![1, 2]);
        let st = crate::olive_struct_alloc(2);
        unsafe {
            *((st + 8) as *mut i64) = 10;
            *((st + 16) as *mut i64) = inner;
        }
        // struct P { a: int, b: [int] }
        let mut d = vec![D_STRUCT];
        d.push(13 + 1);
        d.push(b'P');
        d.push(13 + 2);
        d.push(13 + 1);
        d.push(b'a');
        d.push(1);
        d.push(13 + 1);
        d.push(b'b');
        d.extend_from_slice(&[D_LIST, 1]);
        olive_free_typed(st, desc(&d));
        assert!(!slot_is_live(st));
        assert!(!slot_is_live(inner));
    }

    #[test]
    fn null_and_dead_values_ignored() {
        olive_free_typed(0, desc(&[D_LIST, 1]));
        let l = list_from_vec(vec![1]);
        olive_free_typed(l, desc(&[D_LIST, 1]));
        let recycled = list_from_vec(vec![2]);
        assert_eq!(olive_list_get(recycled, 0), 2);
    }

    #[test]
    fn recursive_struct_freed_without_leaks() {
        let val1 = crate::olive_str_internal("hello");
        let val2 = crate::olive_str_internal("world");
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
        olive_free_typed(node1, d);
        assert!(!slot_is_live(node1));
        assert!(!slot_is_live(node2));
        assert!(!slot_is_live(val1 & !1));
        assert!(!slot_is_live(val2 & !1));
    }

    #[test]
    fn cyclic_struct_free_terminates() {
        let val1 = crate::olive_str_internal("cyclic");
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
        olive_free_typed(node1, d);
        assert!(!slot_is_live(node1));
        assert!(!slot_is_live(val1 & !1));
    }
}
