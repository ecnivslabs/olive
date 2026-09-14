#![allow(unsafe_op_in_unsafe_fn)]

use super::*;
use crate::format::{
    D_ANY, D_BACKREF, D_BYTES, D_DICT, D_LIST, D_SET, D_STRUCT, D_STRUCT_SHARED, D_TUPLE, byte,
    skip,
};
use rustc_hash::FxHashMap;

#[derive(Clone, Debug)]
pub(super) enum PathSegment {
    Dict(i64),
    Index(usize),
    SetValue(i64),
}

struct NestedEntry {
    key: i64,
    value: i64,
    visible: bool,
}

pub(super) struct NestedAnySnapshot {
    path: Vec<PathSegment>,
    entries: Vec<NestedEntry>,
}

#[repr(align(8))]
struct AlignedAnyDescriptor([u8; 1]);
static ANY_DESCRIPTOR: AlignedAnyDescriptor = AlignedAnyDescriptor([D_ANY]);

pub(super) unsafe fn collect_for_typed(value: i64, descriptor: &[u8]) -> Vec<NestedAnySnapshot> {
    let mut snapshots = Vec::new();
    let mut active = Vec::new();
    unsafe {
        collect_descriptor(
            value,
            descriptor.as_ptr(),
            0,
            &mut Vec::new(),
            &mut snapshots,
            &mut active,
        );
    }
    snapshots
}

pub(super) unsafe fn collect_dynamic_root(value: i64) -> Vec<NestedAnySnapshot> {
    let mut snapshots = Vec::new();
    let mut active = Vec::new();
    unsafe {
        collect_dynamic(value, &mut Vec::new(), &mut snapshots, &mut active);
    }
    snapshots
}

pub(super) unsafe fn collect_dynamic_nested(value: i64) -> Vec<NestedAnySnapshot> {
    let mut snapshots = collect_dynamic_root(value);
    snapshots.retain(|snapshot| !snapshot.path.is_empty());
    snapshots
}

pub(super) fn release(snapshots: &[NestedAnySnapshot]) {
    for snapshot in snapshots {
        for entry in &snapshot.entries {
            crate::free_any_word(entry.key);
            crate::free_any_word(entry.value);
        }
    }
}

unsafe fn collect_descriptor(
    value: i64,
    descriptor: *const u8,
    start: usize,
    path: &mut Vec<PathSegment>,
    snapshots: &mut Vec<NestedAnySnapshot>,
    active: &mut Vec<i64>,
) {
    if value == 0 || !crate::slab::slot_is_live(value) || active.contains(&value) {
        return;
    }
    let mut pos = start;
    let tag = byte(descriptor, pos);
    pos += 1;
    match tag {
        D_LIST | D_TUPLE => {
            let child = pos;
            skip(descriptor, &mut pos);
            let kind = unsafe { *(value as *const i64) };
            if kind != crate::KIND_LIST && kind != crate::KIND_ANY_LIST {
                return;
            }
            active.push(value);
            let len = crate::olive_list_len(value);
            for index in 0..len {
                path.push(PathSegment::Index(index as usize));
                collect_descriptor(
                    crate::olive_list_get(value, index),
                    descriptor,
                    child,
                    path,
                    snapshots,
                    active,
                );
                path.pop();
            }
            active.pop();
        }
        D_SET => {
            let child = pos;
            skip(descriptor, &mut pos);
            if unsafe { *(value as *const i64) } != crate::KIND_SET {
                return;
            }
            active.push(value);
            let set = unsafe { &*(value as *const crate::OliveHashSet) };
            for index in 0..set.len {
                let element = unsafe { *set.ptr.add(index) };
                path.push(PathSegment::SetValue(element));
                collect_descriptor(element, descriptor, child, path, snapshots, active);
                path.pop();
            }
            active.pop();
        }
        D_DICT => {
            let key_start = pos;
            skip(descriptor, &mut pos);
            let value_start = pos;
            skip(descriptor, &mut pos);
            if unsafe { *(value as *const i64) } != crate::KIND_OBJ {
                return;
            }
            if byte(descriptor, key_start) == D_ANY {
                record_any_dict(value, path, snapshots);
            }
            active.push(value);
            let object = unsafe { &*(value as *const crate::OliveObj) };
            for (key, &child_value) in &object.fields {
                path.push(PathSegment::Dict(key.0));
                collect_descriptor(
                    child_value,
                    descriptor,
                    value_start,
                    path,
                    snapshots,
                    active,
                );
                path.pop();
            }
            active.pop();
        }
        D_STRUCT | D_STRUCT_SHARED => {
            let name_start = pos;
            let _ = name_start;
            skip_lp(descriptor, &mut pos);
            let count = byte(descriptor, pos) as usize - 13;
            pos += 1;
            if unsafe { *(value as *const i64) } == crate::struct_box::KIND_STRUCT_BOX {
                return;
            }
            active.push(value);
            let field_count = unsafe { *(value as *const i64) } as usize;
            for index in 0..count {
                skip_lp(descriptor, &mut pos);
                let field_start = pos;
                let field = if index < field_count {
                    unsafe { *((value + 8 + 8 * index as i64) as *const i64) }
                } else {
                    0
                };
                collect_descriptor(field, descriptor, field_start, path, snapshots, active);
                skip(descriptor, &mut pos);
            }
            active.pop();
        }
        D_BACKREF => {
            let hi = byte(descriptor, pos) as usize;
            let lo = byte(descriptor, pos + 1) as usize;
            collect_descriptor(value, descriptor, (hi << 8) | lo, path, snapshots, active);
        }
        D_ANY => collect_dynamic(value, path, snapshots, active),
        D_BYTES => {}
        _ => {}
    }
}

unsafe fn collect_dynamic(
    value: i64,
    path: &mut Vec<PathSegment>,
    snapshots: &mut Vec<NestedAnySnapshot>,
    active: &mut Vec<i64>,
) {
    if value == 0 || !crate::slab::slot_is_live(value) || active.contains(&value) {
        return;
    }
    let kind = unsafe { *(value as *const i64) };
    match kind {
        crate::KIND_OBJ => {
            record_any_dict(value, path, snapshots);
            active.push(value);
            let object = unsafe { &*(value as *const crate::OliveObj) };
            for (key, &child) in &object.fields {
                path.push(PathSegment::Dict(key.0));
                collect_dynamic(child, path, snapshots, active);
                path.pop();
            }
            active.pop();
        }
        crate::KIND_LIST | crate::KIND_ANY_LIST => {
            active.push(value);
            let len = crate::olive_list_len(value);
            for index in 0..len {
                path.push(PathSegment::Index(index as usize));
                collect_dynamic(crate::olive_list_get(value, index), path, snapshots, active);
                path.pop();
            }
            active.pop();
        }
        crate::KIND_SET => {
            active.push(value);
            let set = unsafe { &*(value as *const crate::OliveHashSet) };
            for index in 0..set.len {
                let element = unsafe { *set.ptr.add(index) };
                path.push(PathSegment::SetValue(element));
                collect_dynamic(element, path, snapshots, active);
                path.pop();
            }
            active.pop();
        }
        _ => {}
    }
}

unsafe fn python_keys_equal(left: i64, right: i64) -> bool {
    unsafe {
        let left = crate::python::python_coerce::olive_any_to_py(left);
        let right = crate::python::python_coerce::olive_any_to_py(right);
        if left.is_null() || right.is_null() {
            if !left.is_null() {
                PY_DEC_REF(left);
            }
            if !right.is_null() {
                PY_DEC_REF(right);
            }
            PY_ERR_CLEAR();
            return false;
        }
        let equal = PY_OBJECT_RICHCOMPAREBOOL(left, right, 2);
        PY_DEC_REF(left);
        PY_DEC_REF(right);
        if equal < 0 {
            PY_ERR_CLEAR();
            false
        } else {
            equal == 1
        }
    }
}

unsafe fn record_any_dict(
    value: i64,
    path: &[PathSegment],
    snapshots: &mut Vec<NestedAnySnapshot>,
) {
    if !super::any_dict_needs_snapshot(value) {
        return;
    }
    let object = unsafe { &*(value as *const crate::OliveObj) };
    let mut seen = Vec::<i64>::new();
    let mut entries = Vec::new();
    for (key, &child) in &object.fields {
        let visible = !seen.iter().any(|prior| python_keys_equal(*prior, key.0));
        seen.push(key.0);
        let mut visited = FxHashMap::default();
        let copied_key = crate::copy_typed::copy_any(key.0, &mut visited);
        let copied_value = crate::copy_typed::copy_any(child, &mut visited);
        entries.push(NestedEntry {
            key: copied_key,
            value: copied_value,
            visible,
        });
    }
    snapshots.push(NestedAnySnapshot {
        path: path.to_vec(),
        entries,
    });
}

unsafe fn skip_lp(descriptor: *const u8, pos: &mut usize) {
    let len = byte(descriptor, *pos) as usize - 13;
    *pos += 1 + len;
}

unsafe fn navigate_olive(root: i64, path: &[PathSegment]) -> i64 {
    let mut current = root;
    for segment in path {
        if current == 0 || !crate::slab::slot_is_live(current) {
            return 0;
        }
        current = match segment {
            PathSegment::Dict(key) => {
                let object = unsafe { &*(current as *const crate::OliveObj) };
                object
                    .fields
                    .get(&crate::OliveStringKey(*key))
                    .copied()
                    .unwrap_or(0)
            }
            PathSegment::Index(index) => crate::olive_list_get(current, *index as i64),
            PathSegment::SetValue(value) => {
                let set = unsafe { &*(current as *const crate::OliveHashSet) };
                let mut found = 0;
                for index in 0..set.len {
                    let element = unsafe { *set.ptr.add(index) };
                    if element == *value || crate::olive_any_eq(element, *value) != 0 {
                        found = element;
                        break;
                    }
                }
                found
            }
        };
    }
    current
}

unsafe fn navigate_python(root: PyObject, path: &[PathSegment]) -> Option<PyObject> {
    unsafe {
        if root.is_null() {
            return None;
        }
        let mut current = root;
        PY_INC_REF(current);
        for segment in path {
            let next = match segment {
                PathSegment::Dict(key) => {
                    let py_key = crate::python::python_coerce::olive_any_to_py(*key);
                    if py_key.is_null() {
                        PY_ERR_CLEAR();
                        PY_DEC_REF(current);
                        return None;
                    }
                    let value = PY_OBJECT_GET_ITEM(current, py_key);
                    PY_DEC_REF(py_key);
                    value
                }
                PathSegment::Index(index) => {
                    let ty = raw_ob_type(current);
                    let item = if !ty.is_null()
                        && ty == PY_LIST_TYPE
                        && *index < PY_OBJECT_LENGTH(current).max(0) as usize
                    {
                        PY_LIST_GET_ITEM(current, *index as isize)
                    } else if !ty.is_null() && ty == PY_TUPLE_TYPE {
                        PY_TUPLE_GET_ITEM(current, *index as isize)
                    } else {
                        std::ptr::null_mut()
                    };
                    if item.is_null() {
                        PY_DEC_REF(current);
                        return None;
                    }
                    PY_INC_REF(item);
                    item
                }
                PathSegment::SetValue(value) => {
                    let target = crate::python::python_coerce::olive_any_to_py(*value);
                    if target.is_null() {
                        PY_DEC_REF(current);
                        return None;
                    }
                    let iter = PY_OBJECT_GET_ITER(current);
                    if iter.is_null() {
                        PY_DEC_REF(target);
                        PY_DEC_REF(current);
                        return None;
                    }
                    let mut found = std::ptr::null_mut();
                    loop {
                        let item = PY_ITER_NEXT(iter);
                        if item.is_null() {
                            break;
                        }
                        let equal = PY_OBJECT_RICHCOMPAREBOOL(item, target, 2);
                        if equal == 1 {
                            found = item;
                            break;
                        }
                        PY_DEC_REF(item);
                        if equal < 0 {
                            PY_ERR_CLEAR();
                        }
                    }
                    PY_DEC_REF(target);
                    PY_DEC_REF(iter);
                    if found.is_null() {
                        PY_DEC_REF(current);
                        return None;
                    }
                    found
                }
            };
            PY_DEC_REF(current);
            if next.is_null() {
                PY_ERR_CLEAR();
                return None;
            }
            current = next;
        }
        Some(current)
    }
}

unsafe fn python_dict_has_exact_key(dict: PyObject, key: i64) -> bool {
    unsafe {
        let wanted = crate::python::python_coerce::olive_any_to_py(key);
        if wanted.is_null() {
            return false;
        }
        let wanted_type = raw_ob_type(wanted);
        let mut pos: isize = 0;
        let mut candidate = std::ptr::null_mut();
        let mut value = std::ptr::null_mut();
        let mut found = false;
        while PY_DICT_NEXT(dict, &mut pos, &mut candidate, &mut value) != 0 {
            if !candidate.is_null()
                && raw_ob_type(candidate) == wanted_type
                && PY_OBJECT_RICHCOMPAREBOOL(candidate, wanted, 2) == 1
            {
                found = true;
                break;
            }
        }
        if !PY_ERR_OCCURRED().is_null() {
            PY_ERR_CLEAR();
        }
        PY_DEC_REF(wanted);
        found
    }
}

pub(super) unsafe fn restore(pair: &WritebackPair, new_value: i64, _descriptor: &[u8]) {
    unsafe {
        for snapshot in &pair.nested_snapshots {
            let Some(python_dict) = navigate_python(pair.py_obj, &snapshot.path) else {
                continue;
            };
            let olive_dict = navigate_olive(new_value, &snapshot.path);
            if olive_dict == 0
                || !crate::slab::slot_is_live(olive_dict)
                || *(olive_dict as *const i64) != crate::KIND_OBJ
            {
                PY_DEC_REF(python_dict);
                continue;
            }
            let object = &mut *(olive_dict as *mut crate::OliveObj);
            for entry in &snapshot.entries {
                if !super::is_python_equality_scalar_key(entry.key) {
                    continue;
                }
                let exact = python_dict_has_exact_key(python_dict, entry.key);
                if entry.visible || exact {
                    continue;
                }
                let already_present = crate::hash_typed::with_key_descriptor(
                    ANY_DESCRIPTOR.0.as_ptr() as i64,
                    || {
                        object
                            .fields
                            .contains_key(&crate::OliveStringKey(entry.key))
                    },
                );
                if already_present {
                    continue;
                }
                let restored_key =
                    crate::copy_typed::copy_any(entry.key, &mut FxHashMap::default());
                let restored_value =
                    crate::copy_typed::copy_any(entry.value, &mut FxHashMap::default());
                object
                    .fields
                    .insert(crate::OliveStringKey(restored_key), restored_value);
            }
            PY_DEC_REF(python_dict);
        }
    }
}
