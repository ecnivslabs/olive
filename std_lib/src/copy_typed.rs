//! Descriptor-driven deep copy, the mirror of `free_typed`. The compiler passes
//! the static type of an escaping value as the same byte descriptor
//! `olive_format_typed` and `olive_free_typed` consume, so a borrowed element
//! stored into a longer-lived container becomes an independent value the
//! container owns outright.
//!
//! Copy is bounded by the descriptor, not the data: a recursive field is cut to
//! `D_OBJ` at emit time, so `copy_val` returns the shared word there and never
//! recurses forever on a data cycle. The untyped `D_ANY` walk has no descriptor
//! to bound it, so it carries a depth cap instead.
//!
//! A stored string is cloned through `olive_copy`, which lifts a literal or a
//! heap string into a fresh heap string; an interned attribute symbol (untagged)
//! is immortal and shared as-is so dict and struct keys keep their identity.

use crate::boxed::TAG_MASK;
use crate::format::{
    D_ANY, D_BACKREF, D_DICT, D_ENUM, D_LIST, D_SET, D_STR, D_STRUCT, D_TUPLE, byte, skip,
};
use crate::slab::slot_is_live;
use crate::{
    KIND_ANY_LIST, KIND_BYTES, KIND_ENUM, KIND_FLOAT, KIND_INT, KIND_LIST, KIND_OBJ, KIND_PYOBJECT,
    KIND_SET, OliveEnum, OliveHashSet, OliveObj, OliveStringKey, StableVec,
};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::collections::HashMap;

const ANY_DEPTH_CAP: u32 = 512;

thread_local! {
    // Tracks already-copied heap pointers to prevent infinite cycles.
    static COPY_VISITED: RefCell<HashMap<i64, i64>> = RefCell::new(HashMap::new());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_copy_typed(val: i64, desc: i64) -> i64 {
    COPY_VISITED.with(|v| v.borrow_mut().clear());
    let mut pos = 0usize;
    let res = copy_val(val, desc as *const u8, &mut pos);
    COPY_VISITED.with(|v| v.borrow_mut().clear());
    res
}

/// Skips a length-prefixed name; length byte is biased by 13.
fn skip_lp(desc: *const u8, pos: &mut usize) {
    let len = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1 + len;
}

fn copy_val(val: i64, desc: *const u8, pos: &mut usize) -> i64 {
    let cloned_opt = if val != 0 && crate::is_active_object(val) {
        COPY_VISITED.with(|v| v.borrow().get(&val).copied())
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
        D_ANY => copy_any(val, 0),
        D_LIST => copy_list_like(val, desc, pos),
        D_SET => copy_set(val, desc, pos),
        D_TUPLE => copy_tuple(val, desc, pos),
        D_DICT => copy_dict(val, desc, pos),
        D_STRUCT => copy_struct(val, desc, pos),
        D_ENUM => copy_enum(val, desc, pos),
        D_BACKREF => {
            let hi = unsafe { byte(desc, *pos) } as usize;
            let lo = unsafe { byte(desc, *pos + 1) } as usize;
            *pos += 2;
            let mut target_pos = (hi << 8) | lo;
            copy_val(val, desc, &mut target_pos)
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

fn copy_list_like(val: i64, desc: *const u8, pos: &mut usize) -> i64 {
    let inner_start = *pos;
    skip(desc, pos);
    if val == 0 || !slot_is_live(val) {
        return val;
    }
    let (kind, eptr, elen) = unsafe {
        let s = &*(val as *const StableVec);
        if s.kind == KIND_SET {
            return copy_set_at(val, desc, inner_start);
        }
        (s.kind, s.ptr, s.len)
    };
    let new = crate::list::olive_list_new(elen as i64);
    COPY_VISITED.with(|v| v.borrow_mut().insert(val, new));
    for i in 0..elen {
        let mut p = inner_start;
        let c = copy_val(unsafe { *eptr.add(i) }, desc, &mut p);
        crate::list::olive_list_set(new, i as i64, c);
    }
    unsafe { (*(new as *mut StableVec)).kind = kind };
    new
}

fn copy_set(val: i64, desc: *const u8, pos: &mut usize) -> i64 {
    let inner_start = *pos;
    skip(desc, pos);
    if val == 0 || !slot_is_live(val) {
        return val;
    }
    copy_set_at(val, desc, inner_start)
}

fn copy_set_at(val: i64, desc: *const u8, inner_start: usize) -> i64 {
    let (eptr, elen) = unsafe {
        let s = &*(val as *const OliveHashSet);
        (s.ptr, s.len)
    };
    let new = crate::set::olive_set_new(elen as i64);
    COPY_VISITED.with(|v| v.borrow_mut().insert(val, new));
    for i in 0..elen {
        let mut p = inner_start;
        let c = copy_val(unsafe { *eptr.add(i) }, desc, &mut p);
        crate::set::olive_set_add(new, c);
    }
    new
}

fn copy_tuple(val: i64, desc: *const u8, pos: &mut usize) -> i64 {
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
    COPY_VISITED.with(|v| v.borrow_mut().insert(val, new));
    for i in 0..n {
        let elem = if i < elen { unsafe { *eptr.add(i) } } else { 0 };
        let c = copy_val(elem, desc, pos);
        crate::list::olive_list_set(new, i as i64, c);
    }
    new
}

fn copy_dict(val: i64, desc: *const u8, pos: &mut usize) -> i64 {
    let key_start = *pos;
    skip(desc, pos);
    let val_start = *pos;
    skip(desc, pos);
    if val == 0 || !slot_is_live(val) {
        return val;
    }
    let obj = unsafe { &*(val as *const OliveObj) };
    let new = crate::obj::olive_obj_new();
    COPY_VISITED.with(|v| v.borrow_mut().insert(val, new));
    let mut fields = FxHashMap::default();
    for (k, &v) in obj.fields.iter() {
        let mut kp = key_start;
        let kc = copy_val(k.0, desc, &mut kp);
        let mut vp = val_start;
        let vc = copy_val(v, desc, &mut vp);
        fields.insert(OliveStringKey(kc), vc);
    }
    unsafe { (*(new as *mut OliveObj)).fields = fields };
    new
}

fn copy_struct(val: i64, desc: *const u8, pos: &mut usize) -> i64 {
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
    COPY_VISITED.with(|v| v.borrow_mut().insert(val, new));
    for i in 0..n {
        skip_lp(desc, pos);
        let field = if (i as i64) < n_fields {
            unsafe { *((val + 8 + 8 * i as i64) as *const i64) }
        } else {
            0
        };
        let c = copy_val(field, desc, pos);
        if (i as i64) < n_fields {
            unsafe { *((new + 8 + 8 * i as i64) as *mut i64) = c };
        }
    }
    new
}

fn copy_enum(val: i64, desc: *const u8, pos: &mut usize) -> i64 {
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
        COPY_VISITED.with(|v| v.borrow_mut().insert(val, new));
    }
    for i in 0..n {
        skip_lp(desc, pos);
        let np = unsafe { byte(desc, *pos) } as usize - 13;
        *pos += 1;
        for j in 0..np {
            if i == tag && j < plen {
                let c = copy_val(unsafe { *pptr.add(j) }, desc, pos);
                crate::olive_enum_set(new, j as i64, c);
            } else {
                skip(desc, pos);
            }
        }
    }
    if live { new } else { val }
}

/// Kind-driven deep copy of a statically-`Any` word, the mirror of the
/// kind-driven deep free `olive_free_any` drives. A descriptor cannot bound this
/// walk, so a depth cap breaks a pathological data cycle by sharing the tail.
fn copy_any(val: i64, depth: u32) -> i64 {
    if val == 0 || val & TAG_MASK != 0 {
        return val;
    }
    if val & 1 != 0 {
        return crate::olive_copy(val);
    }
    if depth >= ANY_DEPTH_CAP || !crate::is_active_object(val) {
        return val;
    }
    if let Some(cloned) = COPY_VISITED.with(|v| v.borrow().get(&val).copied()) {
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
            COPY_VISITED.with(|v| v.borrow_mut().insert(val, new));
            for i in 0..elen {
                let c = copy_any(unsafe { *eptr.add(i) }, depth + 1);
                crate::list::olive_list_set(new, i as i64, c);
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
            COPY_VISITED.with(|v| v.borrow_mut().insert(val, new));
            for i in 0..elen {
                crate::set::olive_set_add(new, copy_any(unsafe { *eptr.add(i) }, depth + 1));
            }
            new
        }
        KIND_OBJ => {
            let obj = unsafe { &*(val as *const OliveObj) };
            let new = crate::obj::olive_obj_new();
            COPY_VISITED.with(|v| v.borrow_mut().insert(val, new));
            let mut fields = FxHashMap::default();
            for (k, &v) in obj.fields.iter() {
                fields.insert(OliveStringKey(copy_str(k.0)), copy_any(v, depth + 1));
            }
            unsafe { (*(new as *mut OliveObj)).fields = fields };
            new
        }
        KIND_ENUM => {
            let e = unsafe { &*(val as *const OliveEnum) };
            let new = crate::olive_enum_new(e.type_id, e.tag, e.payload_len as i64);
            COPY_VISITED.with(|v| v.borrow_mut().insert(val, new));
            for j in 0..e.payload_len {
                crate::olive_enum_set(
                    new,
                    j as i64,
                    copy_any(unsafe { *e.payload_ptr.add(j) }, depth + 1),
                );
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
        KIND_BYTES => {
            let bytes = unsafe { &*(val as *const crate::bytes::OliveBytes) };
            crate::bytes::new_buf(bytes.as_slice().to_vec())
        }
        // A Python handle is refcounted; an owning copy is a fresh reference.
        KIND_PYOBJECT => {
            unsafe { crate::python::PY_INC_REF(val as *mut std::ffi::c_void) };
            val
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
        assert_ne!(a & !1, b & !1, "distinct heap slots");
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
        assert_ne!(copied & !1, elem & !1, "element is a fresh heap slot");
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
}
