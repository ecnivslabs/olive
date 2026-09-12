//! Descriptor-driven Python-to-Olive collection import.

use super::*;
use crate::format::{
    D_ANY, D_BOOL, D_BYTES, D_DICT, D_F32, D_FLOAT, D_INT, D_LIST, D_NULL, D_SET, D_STR, D_TUPLE,
    D_U64, byte, skip,
};
use crate::python::python_coerce::{
    olive_py_to_bytes_internal, py_to_any_internal, py_to_typed_scalar_internal,
};

fn import_error(message: &str) -> ! {
    unsafe {
        if !PY_ERR_OCCURRED().is_null() {
            crate::python::python_error::handle_py_error();
        }
    }
    crate::panic::abort_py_coerce(message)
}

unsafe fn is_subtype(value: PyObject, expected: PyObject) -> bool {
    unsafe {
        let actual = crate::python::python_coerce::raw_ob_type(value);
        !expected.is_null()
            && !actual.is_null()
            && (actual == expected || PY_TYPE_IS_SUBTYPE(actual, expected) != 0)
    }
}

unsafe fn free_at(value: i64, desc: *const u8, start: usize) {
    let mut pos = start;
    crate::free_typed::free_val(value, desc, &mut pos);
}

unsafe fn py_to_string_key(value: PyObject) -> Option<i64> {
    unsafe {
        let ty = crate::python::python_coerce::raw_ob_type(value);
        let is_unicode = !ty.is_null()
            && !PY_UNICODE_TYPE.is_null()
            && (ty == PY_UNICODE_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_UNICODE_TYPE) != 0);
        if is_unicode {
            let result = crate::python::python_coerce::py_str_to_olive(value);
            return (result != 0).then_some(result);
        }
        let text = PY_OBJECT_STR(value);
        if text.is_null() {
            return None;
        }
        let result = crate::python::python_coerce::py_str_to_olive(text);
        PY_DEC_REF(text);
        (result != 0).then_some(result)
    }
}

unsafe fn snapshot_sequence(value: PyObject) -> Option<Vec<PyObject>> {
    unsafe {
        let ty = crate::python::python_coerce::raw_ob_type(value);
        let is_list = !ty.is_null()
            && !PY_LIST_TYPE.is_null()
            && (ty == PY_LIST_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_LIST_TYPE) != 0);
        let is_tuple = !ty.is_null()
            && !PY_TUPLE_TYPE.is_null()
            && (ty == PY_TUPLE_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_TUPLE_TYPE) != 0);
        let (source, owned) = if is_list || is_tuple {
            (value, false)
        } else {
            let list = PY_SEQUENCE_LIST(value);
            if list.is_null() {
                return None;
            }
            (list, true)
        };
        let len = PY_OBJECT_LENGTH(source);
        if len < 0 {
            if owned {
                PY_DEC_REF(source);
            }
            return None;
        }
        let mut items = Vec::with_capacity(len as usize);
        for i in 0..len {
            let item = if is_list || owned {
                PY_LIST_GET_ITEM(source, i)
            } else {
                PY_TUPLE_GET_ITEM(source, i)
            };
            if item.is_null() {
                for item in items {
                    PY_DEC_REF(item);
                }
                if owned {
                    PY_DEC_REF(source);
                }
                return None;
            }
            PY_INC_REF(item);
            items.push(item);
        }
        if owned {
            PY_DEC_REF(source);
        }
        Some(items)
    }
}

unsafe fn snapshot_dict(value: PyObject) -> Option<Vec<(PyObject, PyObject)>> {
    unsafe {
        if !is_subtype(value, PY_DICT_TYPE) {
            return None;
        }
        let mut entries = Vec::new();
        let mut pos: isize = 0;
        let mut key = std::ptr::null_mut();
        let mut val = std::ptr::null_mut();
        while PY_DICT_NEXT(value, &mut pos, &mut key, &mut val) != 0 {
            if key.is_null() || val.is_null() {
                for (key, val) in entries {
                    PY_DEC_REF(key);
                    PY_DEC_REF(val);
                }
                return None;
            }
            PY_INC_REF(key);
            PY_INC_REF(val);
            entries.push((key, val));
        }
        if !PY_ERR_OCCURRED().is_null() {
            for (key, val) in entries {
                PY_DEC_REF(key);
                PY_DEC_REF(val);
            }
            return None;
        }
        Some(entries)
    }
}

unsafe fn snapshot_iterable(value: PyObject) -> Option<Vec<PyObject>> {
    unsafe {
        let iter = PY_OBJECT_GET_ITER(value);
        if iter.is_null() {
            return None;
        }
        let mut items = Vec::new();
        loop {
            let item = PY_ITER_NEXT(iter);
            if item.is_null() {
                if !PY_ERR_OCCURRED().is_null()
                    && PY_ERR_EXCEPTION_MATCHES(PY_EXC_STOP_ITERATION) == 0
                {
                    PY_DEC_REF(iter);
                    return None;
                }
                PY_ERR_CLEAR();
                break;
            }
            items.push(item);
        }
        PY_DEC_REF(iter);
        Some(items)
    }
}

unsafe fn convert_at(value: PyObject, desc: *const u8, pos: &mut usize) -> Option<i64> {
    unsafe {
        if value.is_null() {
            return None;
        }
        let node_start = *pos;
        let tag = byte(desc, *pos);
        *pos += 1;
        match tag {
            D_INT | D_U64 | D_FLOAT | D_F32 | D_BOOL | D_STR | D_NULL => {
                let scalar_tag = match tag {
                    D_INT => 1,
                    D_U64 => 7,
                    D_FLOAT => 2,
                    D_F32 => 3,
                    D_BOOL => 4,
                    D_STR => 5,
                    D_NULL => 6,
                    _ => unreachable!(),
                };
                py_to_typed_scalar_internal(value, scalar_tag)
            }
            D_ANY => Some(py_to_any_internal(value)),
            D_BYTES => {
                if is_subtype(value, PY_BYTES_TYPE) {
                    Some(olive_py_to_bytes_internal(value))
                } else {
                    None
                }
            }
            D_LIST => {
                let element_start = *pos;
                skip(desc, pos);
                let items = snapshot_sequence(value)?;
                let list = crate::olive_list_new(items.len() as i64);
                if byte(desc, element_start) == D_ANY {
                    crate::olive_list_mark_any(list);
                }
                for (index, item) in items.into_iter().enumerate() {
                    let mut element_pos = element_start;
                    let Some(converted) = convert_at(item, desc, &mut element_pos) else {
                        PY_DEC_REF(item);
                        free_at(list, desc, node_start);
                        return None;
                    };
                    crate::olive_list_set(list, index as i64, converted);
                    PY_DEC_REF(item);
                }
                Some(list)
            }
            D_TUPLE => {
                let count = byte(desc, *pos) as usize - 1;
                *pos += 1;
                let fields_start = *pos;
                let items = snapshot_sequence(value)?;
                if items.len() != count {
                    for item in items {
                        PY_DEC_REF(item);
                    }
                    return None;
                }
                let tuple = crate::olive_list_new(count as i64);
                let mut field_pos = fields_start;
                let mut any_field = false;
                for _ in 0..count {
                    if byte(desc, field_pos) == D_ANY {
                        any_field = true;
                    }
                    skip(desc, &mut field_pos);
                }
                if any_field {
                    crate::olive_list_mark_any(tuple);
                }
                let mut cursor = fields_start;
                for (index, item) in items.into_iter().enumerate() {
                    let Some(converted) = convert_at(item, desc, &mut cursor) else {
                        PY_DEC_REF(item);
                        free_at(tuple, desc, node_start);
                        return None;
                    };
                    crate::olive_list_set(tuple, index as i64, converted);
                    PY_DEC_REF(item);
                }
                Some(tuple)
            }
            D_SET => {
                let element_start = *pos;
                skip(desc, pos);
                let items = snapshot_iterable(value)?;
                let set = crate::olive_set_new(items.len() as i64);
                for item in items {
                    let mut element_pos = element_start;
                    let Some(converted) = convert_at(item, desc, &mut element_pos) else {
                        PY_DEC_REF(item);
                        free_at(set, desc, node_start);
                        return None;
                    };
                    crate::hash_typed::with_owned_sub_descriptor(desc, element_start, |key_desc| {
                        crate::hash_typed::olive_set_add_typed(set, converted, key_desc);
                    });
                    PY_DEC_REF(item);
                }
                Some(set)
            }
            D_DICT => {
                let key_start = *pos;
                skip(desc, pos);
                let value_start = *pos;
                skip(desc, pos);
                let entries = snapshot_dict(value)?;
                let object = crate::olive_obj_new();
                for (key_object, value_object) in entries {
                    let mut key_pos = key_start;
                    let key = if byte(desc, key_start) == D_STR {
                        py_to_string_key(key_object)
                    } else {
                        convert_at(key_object, desc, &mut key_pos)
                    };
                    let Some(key) = key else {
                        PY_DEC_REF(key_object);
                        PY_DEC_REF(value_object);
                        free_at(object, desc, node_start);
                        return None;
                    };
                    let mut value_pos = value_start;
                    let Some(converted_value) = convert_at(value_object, desc, &mut value_pos)
                    else {
                        free_at(key, desc, key_start);
                        PY_DEC_REF(key_object);
                        PY_DEC_REF(value_object);
                        free_at(object, desc, node_start);
                        return None;
                    };
                    let old =
                        crate::hash_typed::with_owned_sub_descriptor(desc, key_start, |key_desc| {
                            crate::hash_typed::with_key_descriptor(key_desc, || {
                                let obj = &mut *(object as *mut crate::OliveObj);
                                obj.fields
                                    .insert(crate::OliveStringKey(key), converted_value)
                            })
                        });
                    if let Some(old_value) = old {
                        free_at(key, desc, key_start);
                        if old_value != converted_value {
                            free_at(old_value, desc, value_start);
                        }
                    }
                    PY_DEC_REF(key_object);
                    PY_DEC_REF(value_object);
                }
                Some(object)
            }
            _ => None,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_typed(obj: PyObject, desc: i64) -> i64 {
    check_python_loaded();
    let desc_ptr = crate::string_slab::str_body(desc) as *const u8;
    if !desc_ptr.is_null() && unsafe { byte(desc_ptr, 0) } == D_LIST {
        let child_tag = unsafe { byte(desc_ptr, 1) };
        let buffer_tag = match child_tag {
            D_INT => Some(1),
            D_FLOAT => Some(2),
            _ => None,
        };
        if let Some(buffer_tag) = buffer_tag {
            let fast = crate::python::python_buffer::olive_py_buffer_to_list(obj, buffer_tag);
            if fast != 0 {
                return fast;
            }
        }
    }
    let raw = unsafe { olive_py_unwrap(obj) };
    if raw.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        let desc_ptr = crate::string_slab::str_body(desc) as *const u8;
        if desc_ptr.is_null() {
            return 0;
        }
        let mut pos = 0;
        match convert_at(raw, desc_ptr, &mut pos) {
            Some(value) => value,
            None => import_error("Python value cannot be converted to declared Olive type"),
        }
    })
}
