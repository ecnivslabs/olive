//! Descriptor-driven Python-to-Olive collection import.

use super::*;
use crate::format::{
    D_ANY, D_BACKREF, D_BOOL, D_BYTES, D_DICT, D_F32, D_FLOAT, D_INT, D_LIST, D_NULL, D_NULLABLE,
    D_SET, D_STR, D_TUPLE, D_U64, byte, skip,
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

unsafe fn decref_sequence_from(items: &mut Vec<PyObject>, start: usize) {
    for item in items.drain(start..) {
        unsafe { PY_DEC_REF(item) };
    }
}

unsafe fn decref_dict_entries_from(entries: &mut Vec<(PyObject, PyObject)>, start: usize) {
    for (key, value) in entries.drain(start..) {
        unsafe {
            PY_DEC_REF(key);
            PY_DEC_REF(value);
        }
    }
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
                decref_sequence_from(&mut items, 0);
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
                decref_dict_entries_from(&mut entries, 0);
                return None;
            }
            PY_INC_REF(key);
            PY_INC_REF(val);
            entries.push((key, val));
        }
        if !PY_ERR_OCCURRED().is_null() {
            decref_dict_entries_from(&mut entries, 0);
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
                    decref_sequence_from(&mut items, 0);
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

pub(crate) unsafe fn convert_at(value: PyObject, desc: *const u8, pos: &mut usize) -> Option<i64> {
    unsafe {
        if value.is_null() {
            return None;
        }
        let _conversion_guard = crate::python::python_coerce::ConversionGuard::enter(value)?;
        let node_start = *pos;
        let tag = byte(desc, *pos);
        *pos += 1;
        match tag {
            D_NULLABLE => {
                if value == _PY_NONE_STRUCT {
                    skip(desc, pos);
                    Some(0)
                } else {
                    convert_at(value, desc, pos)
                }
            }
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
            D_ANY => {
                let converted = py_to_any_internal(value);
                if crate::python::python_coerce::conversion_failed() {
                    None
                } else {
                    Some(converted)
                }
            }
            D_BACKREF => {
                let hi = byte(desc, *pos) as usize;
                let lo = byte(desc, *pos + 1) as usize;
                *pos += 2;
                let mut target = (hi << 8) | lo;
                convert_at(value, desc, &mut target)
            }
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
                let mut items = snapshot_sequence(value)?;
                let list = crate::olive_list_new(items.len() as i64);
                if byte(desc, element_start) == D_ANY {
                    crate::olive_list_mark_any(list);
                }
                for index in 0..items.len() {
                    let item = items[index];
                    let mut element_pos = element_start;
                    let Some(converted) = convert_at(item, desc, &mut element_pos) else {
                        free_at(list, desc, node_start);
                        decref_sequence_from(&mut items, index);
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
                let mut items = snapshot_sequence(value)?;
                if items.len() != count {
                    decref_sequence_from(&mut items, 0);
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
                for index in 0..items.len() {
                    let item = items[index];
                    let Some(converted) = convert_at(item, desc, &mut cursor) else {
                        free_at(tuple, desc, node_start);
                        decref_sequence_from(&mut items, index);
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
                let mut items = snapshot_iterable(value)?;
                let set = crate::olive_set_new(items.len() as i64);
                for index in 0..items.len() {
                    let item = items[index];
                    let mut element_pos = element_start;
                    let Some(converted) = convert_at(item, desc, &mut element_pos) else {
                        free_at(set, desc, node_start);
                        decref_sequence_from(&mut items, index);
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
                let mut entries = snapshot_dict(value)?;
                let object = crate::olive_obj_new();
                for index in 0..entries.len() {
                    let (key_object, value_object) = entries[index];
                    let mut key_pos = key_start;
                    let key = if byte(desc, key_start) == D_STR {
                        py_to_string_key(key_object)
                    } else {
                        convert_at(key_object, desc, &mut key_pos)
                    };
                    let Some(key) = key else {
                        free_at(object, desc, node_start);
                        decref_dict_entries_from(&mut entries, index);
                        return None;
                    };
                    let mut value_pos = value_start;
                    let Some(converted_value) = convert_at(value_object, desc, &mut value_pos)
                    else {
                        free_at(key, desc, key_start);
                        free_at(object, desc, node_start);
                        decref_dict_entries_from(&mut entries, index);
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
            None => {
                let message =
                    crate::python::python_coerce::take_conversion_error().unwrap_or_else(|| {
                        "Python value cannot be converted to declared Olive type".to_string()
                    });
                import_error(&message)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn raw_refcnt(value: PyObject) -> isize {
        unsafe { *(value as *const isize) }
    }

    unsafe fn assert_tail_ref_released(source: PyObject, tail: PyObject, desc: &[u8]) {
        unsafe {
            let baseline = raw_refcnt(tail);
            let mut pos = 0;
            let result = convert_at(source, desc.as_ptr(), &mut pos);
            let after = raw_refcnt(tail);
            PY_DEC_REF(source);
            assert!(result.is_none());
            assert_eq!(after, baseline);
        }
    }

    #[test]
    fn cyclic_python_collections_are_rejected_without_recursing() {
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            with_gil(|| {
                let source = PY_LIST_NEW(1);
                assert!(!source.is_null());
                PY_INC_REF(source);
                assert_eq!(PY_LIST_SET_ITEM(source, 0, source), 0);
                let descriptor = [D_LIST, D_ANY];
                let mut pos = 0usize;
                assert!(convert_at(source, descriptor.as_ptr(), &mut pos).is_none());
                let _ = crate::python::python_coerce::take_conversion_error();
                PY_DEC_REF(source);
                PY_DEC_REF(source);
            });
        }
    }

    #[test]
    fn collection_failures_release_remaining_snapshot_refs() {
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            with_gil(|| {
                let cases: &[(bool, &[u8])] = &[
                    (false, &[D_LIST, D_BOOL]),
                    (true, &[D_TUPLE, 4, D_BOOL, D_BOOL, D_BOOL]),
                    (false, &[D_SET, D_BOOL]),
                ];
                for &(tuple_source, desc) in cases {
                    let tail = PY_LONG_FROM_LONG(100_001);
                    let source = if tuple_source {
                        PY_TUPLE_NEW(3)
                    } else {
                        PY_LIST_NEW(3)
                    };
                    let set_item = if tuple_source {
                        PY_TUPLE_SET_ITEM
                    } else {
                        PY_LIST_SET_ITEM
                    };
                    set_item(source, 0, PY_BOOL_FROM_LONG(1));
                    set_item(source, 1, PY_LONG_FROM_LONG(100_002));
                    set_item(source, 2, tail);
                    assert_tail_ref_released(source, tail, desc);
                }
            });
        }
    }

    #[test]
    fn dict_failure_releases_remaining_snapshot_refs() {
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            with_gil(|| {
                for fail_value in [false, true] {
                    let tail_key = PY_BOOL_FROM_LONG(0);
                    let tail_value = PY_LONG_FROM_LONG(100_003);
                    let good_key = PY_BOOL_FROM_LONG(1);
                    let good_value = if fail_value {
                        PY_BOOL_FROM_LONG(1)
                    } else {
                        PY_LONG_FROM_LONG(1)
                    };
                    let bad_key = if fail_value {
                        PY_BOOL_FROM_LONG(1)
                    } else {
                        PY_LONG_FROM_LONG(100_004)
                    };
                    let bad_value = if fail_value {
                        PY_LONG_FROM_LONG(100_004)
                    } else {
                        PY_LONG_FROM_LONG(2)
                    };
                    let source = PY_DICT_NEW();
                    assert_eq!(PY_OBJECT_SET_ITEM(source, good_key, good_value), 0);
                    assert_eq!(PY_OBJECT_SET_ITEM(source, bad_key, bad_value), 0);
                    assert_eq!(PY_OBJECT_SET_ITEM(source, tail_key, tail_value), 0);
                    PY_DEC_REF(good_key);
                    PY_DEC_REF(good_value);
                    PY_DEC_REF(bad_key);
                    PY_DEC_REF(bad_value);
                    let key_baseline = raw_refcnt(tail_key);
                    let value_baseline = raw_refcnt(tail_value);

                    let desc = [D_DICT, D_BOOL, if fail_value { D_BOOL } else { D_ANY }];
                    let mut pos = 0;
                    let result = convert_at(source, desc.as_ptr(), &mut pos);
                    let key_after = raw_refcnt(tail_key);
                    let value_after = raw_refcnt(tail_value);
                    PY_DEC_REF(source);

                    assert!(result.is_none());
                    assert_eq!(key_after, key_baseline);
                    assert_eq!(value_after, value_baseline);
                }
            });
        }
    }

    #[test]
    fn iterator_error_releases_yielded_refs() {
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            with_gil(|| {
                let source = b"__olive_coerce_probe = 100005\ndef __olive_coerce_broken_iter():\n    yield __olive_coerce_probe\n    raise RuntimeError('broken iterator')\n\0";
                assert_eq!(PY_RUN_SIMPLE_STRING(source.as_ptr().cast()), 0);
                let main = PY_IMPORT_IMPORT_MODULE(b"__main__\0".as_ptr().cast());
                let factory = PY_OBJECT_GET_ATTR_STRING(
                    main,
                    b"__olive_coerce_broken_iter\0".as_ptr().cast(),
                );
                let probe =
                    PY_OBJECT_GET_ATTR_STRING(main, b"__olive_coerce_probe\0".as_ptr().cast());
                PY_DEC_REF(main);
                let args = PY_TUPLE_NEW(0);
                let iterator = PY_OBJECT_CALL_OBJECT(factory, args);
                PY_DEC_REF(args);
                PY_DEC_REF(factory);
                assert!(!iterator.is_null());
                assert!(!probe.is_null());

                let baseline = raw_refcnt(probe);
                let result = snapshot_iterable(iterator);
                let was_none = result.is_none();
                let after = raw_refcnt(probe);
                let had_error = !PY_ERR_OCCURRED().is_null();
                PY_ERR_CLEAR();
                if let Some(items) = result {
                    for item in items {
                        PY_DEC_REF(item);
                    }
                }
                PY_DEC_REF(probe);

                assert!(had_error);
                assert!(was_none);
                assert_eq!(after, baseline);
            });
        }
    }
}
