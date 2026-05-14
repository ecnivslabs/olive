use crate::python::*;
use std::ffi::CStr;
use std::os::raw::{c_char, c_double, c_long};

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_int(v: i64) -> PyObject {
    check_python_loaded();
    with_gil(|| unsafe {
        let r = PY_LONG_FROM_LONG(v as c_long);
        olive_py_wrap_borrowed(r)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_float(v: f64) -> PyObject {
    check_python_loaded();
    with_gil(|| unsafe {
        let r = PY_FLOAT_FROM_DOUBLE(v as c_double);
        olive_py_wrap_borrowed(r)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_str(s: i64) -> PyObject {
    check_python_loaded();
    with_gil(|| unsafe {
        let r = PY_UNICODE_FROM_STRING((s & !1) as *const c_char);
        olive_py_wrap_borrowed(r)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_conv_to_py(val: i64) -> PyObject {
    check_python_loaded();
    with_gil(|| olive_to_py(val))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_conv_to_olive(py_val: PyObject) -> i64 {
    check_python_loaded();
    with_gil(|| unsafe { py_to_olive_internal(py_val) })
}

/// Deep-converts an Olive value to a genuine Python object (dicts to real
/// `dict`, lists to real `list`, recursively), wrapped as an Olive `PyObject`.
/// Unlike the default boundary, which hands Python a zero-copy proxy, this yields
/// a value that satisfies `isinstance(x, dict)` and other concrete-type checks,
/// for libraries that require a real dict.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_realize(val: i64) -> PyObject {
    check_python_loaded();
    with_gil(|| unsafe { olive_py_wrap_owned(deep_to_py(val)) })
}

unsafe fn deep_to_py(val: i64) -> PyObject {
    unsafe {
        if val == 0 || !crate::is_active_object(val) {
            return olive_any_to_py(val);
        }
        let kind = *(val as *const i64);
        match kind {
            crate::KIND_OBJ => {
                let py_dict = PY_DICT_NEW();
                let keys = crate::olive_obj_keys(val);
                let n = crate::olive_list_len(keys);
                for i in 0..n {
                    let key = crate::olive_list_get(keys, i);
                    let value = crate::olive_obj_get(val, key);
                    let py_value = deep_to_py(value);
                    PY_DICT_SET_ITEM_STRING(py_dict, (key & !1) as *const c_char, py_value);
                    PY_DEC_REF(py_value);
                }
                py_dict
            }
            crate::KIND_LIST | crate::KIND_ANY_LIST => {
                let n = crate::olive_list_len(val);
                let py_list = PY_LIST_NEW(n as isize);
                for i in 0..n {
                    let elem = crate::olive_list_get(val, i);
                    let item = if kind == crate::KIND_ANY_LIST {
                        deep_to_py(elem)
                    } else {
                        olive_to_py(elem)
                    };
                    PY_LIST_SET_ITEM(py_list, i as isize, item);
                }
                py_list
            }
            _ => olive_to_py(val),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_float_bits(val: i64) -> PyObject {
    check_python_loaded();
    unsafe {
        let f = f64::from_bits(val as u64);
        with_gil(|| {
            let r = PY_FLOAT_FROM_DOUBLE(f as c_double);
            olive_py_wrap_borrowed(r)
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_eq(l: PyObject, r: PyObject) -> i64 {
    check_python_loaded();
    let un_l = unsafe { olive_py_unwrap(l) };
    let un_r = unsafe { olive_py_unwrap(r) };
    if un_l.is_null() || un_r.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        let res = PY_OBJECT_RICHCOMPAREBOOL(un_l, un_r, 2);
        if res == -1 {
            PY_ERR_CLEAR();
            0
        } else {
            res as i64
        }
    })
}

fn py_richcmp(l: PyObject, r: PyObject, op: std::ffi::c_int) -> i64 {
    check_python_loaded();
    let un_l = unsafe { olive_py_unwrap(l) };
    let un_r = unsafe { olive_py_unwrap(r) };
    if un_l.is_null() || un_r.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        let res = PY_OBJECT_RICHCOMPAREBOOL(un_l, un_r, op);
        if res == -1 {
            PY_ERR_CLEAR();
            0
        } else {
            res as i64
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_lt(l: PyObject, r: PyObject) -> i64 {
    py_richcmp(l, r, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_le(l: PyObject, r: PyObject) -> i64 {
    py_richcmp(l, r, 1)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_gt(l: PyObject, r: PyObject) -> i64 {
    py_richcmp(l, r, 4)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_ge(l: PyObject, r: PyObject) -> i64 {
    py_richcmp(l, r, 5)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_ne(l: PyObject, r: PyObject) -> i64 {
    py_richcmp(l, r, 3)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_copy_ref(arena_ptr: PyObject) -> PyObject {
    if arena_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let raw_py = unsafe { olive_py_unwrap(arena_ptr) };
    if raw_py.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { olive_py_wrap_borrowed(raw_py) }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_int(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        let int_obj = PY_NUMBER_LONG(unwrapped_obj);
        if int_obj.is_null() {
            PY_ERR_CLEAR();
            return 0;
        }
        let result = PY_LONG_AS_LONG(int_obj) as i64;
        PY_DEC_REF(int_obj);
        if !PY_ERR_OCCURRED().is_null() {
            PY_ERR_CLEAR();
            return 0;
        }
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_float(obj: PyObject) -> f64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0.0;
    }
    with_gil(|| unsafe { PY_FLOAT_AS_DOUBLE(unwrapped_obj) as f64 })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_str(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        let str_obj = PY_OBJECT_STR(unwrapped_obj);

        if !str_obj.is_null() {
            let s = PY_UNICODE_AS_UTF8(str_obj);
            let r = if !s.is_null() {
                let r_str = CStr::from_ptr(s).to_string_lossy();
                crate::olive_str_internal(&r_str)
            } else {
                0
            };
            PY_DEC_REF(str_obj);
            r
        } else {
            0
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_list(s: i64) -> PyObject {
    check_python_loaded();
    if s == 0 {
        return std::ptr::null_mut();
    }
    with_gil(|| unsafe {
        let sv = &*(s as *const crate::StableVec);
        let pyl = PY_LIST_NEW(sv.len as isize);
        for i in 0..sv.len {
            let v = *sv.ptr.add(i);
            let py_v = olive_to_py(v);
            PY_LIST_SET_ITEM(pyl, i as isize, py_v);
        }
        olive_py_wrap_owned(pyl)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_list(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    with_gil(|| unsafe { olive_py_to_list_internal(unwrapped_obj) })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_dict(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    with_gil(|| unsafe { olive_py_to_dict_internal(unwrapped_obj) })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getitem(obj: PyObject, key: PyObject) -> PyObject {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    with_gil(|| unsafe {
        let py_key = olive_to_py(key as i64);
        if py_key.is_null() {
            return std::ptr::null_mut();
        }
        let r = PY_OBJECT_GET_ITEM(unwrapped_obj, py_key);
        PY_DEC_REF(py_key);
        if r.is_null() {
            handle_py_error();
        }
        olive_py_wrap_borrowed(r)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setitem(obj: PyObject, key: PyObject, val: PyObject) {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return;
    }
    with_gil(|| unsafe {
        let py_key = olive_to_py(key as i64);
        let py_val = olive_to_py(val as i64);
        if py_key.is_null() || py_val.is_null() {
            if !py_key.is_null() {
                PY_DEC_REF(py_key);
            }
            if !py_val.is_null() {
                PY_DEC_REF(py_val);
            }
            return;
        }
        let res = PY_OBJECT_SET_ITEM(unwrapped_obj, py_key, py_val);
        PY_DEC_REF(py_key);
        PY_DEC_REF(py_val);
        if res == -1 {
            handle_py_error();
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getitem_int(obj: PyObject, key: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    with_gil(|| unsafe {
        let py_key = PY_LONG_FROM_LONG(key as std::os::raw::c_long);
        if py_key.is_null() {
            return std::ptr::null_mut();
        }
        let r = PY_OBJECT_GET_ITEM(unwrapped_obj, py_key);
        PY_DEC_REF(py_key);
        if r.is_null() {
            handle_py_error();
        }
        olive_py_wrap_borrowed(r)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setitem_int(obj: PyObject, key: i64, val: PyObject) {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return;
    }
    with_gil(|| unsafe {
        let py_key = PY_LONG_FROM_LONG(key as std::os::raw::c_long);
        let py_val = olive_to_py(val as i64);
        if py_key.is_null() || py_val.is_null() {
            if !py_key.is_null() {
                PY_DEC_REF(py_key);
            }
            if !py_val.is_null() {
                PY_DEC_REF(py_val);
            }
            return;
        }
        let res = PY_OBJECT_SET_ITEM(unwrapped_obj, py_key, py_val);
        PY_DEC_REF(py_key);
        PY_DEC_REF(py_val);
        if res == -1 {
            handle_py_error();
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_len(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    with_gil(|| unsafe { PY_OBJECT_LENGTH(unwrapped_obj) as i64 })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_none() -> PyObject {
    check_python_loaded();
    unsafe { olive_py_wrap_borrowed(_PY_NONE_STRUCT) }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_is_none(obj: PyObject) -> i64 {
    check_python_loaded();
    if (obj as i64) == 0 {
        return 1;
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() || unwrapped_obj == unsafe { _PY_NONE_STRUCT } {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_dict_keys_ffi(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        return 0;
    }
    unsafe {
        let obj = &*(obj_ptr as *const crate::OliveObj);
        let list_ptr = crate::olive_list_new(obj.fields.len() as i64);
        let sv = &mut *(list_ptr as *mut crate::StableVec);
        for (i, k) in obj.fields.keys().enumerate() {
            *sv.ptr.add(i) = k.0;
        }
        list_ptr
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_is_valid_proxy(ptr: i64) -> i64 {
    if crate::is_active_object(ptr) { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_import(name: i64) -> PyObject {
    check_python_loaded();
    with_gil(|| unsafe {
        let m = PY_IMPORT_IMPORT_MODULE((name & !1) as *const c_char);
        if m.is_null() {
            handle_py_error();
        }
        olive_py_wrap_owned(m)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getattr(obj: PyObject, attr: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    with_gil(|| unsafe {
        let a = PY_OBJECT_GET_ATTR_STRING(unwrapped_obj, (attr & !1) as *const c_char);
        if a.is_null() {
            handle_py_error();
        }
        olive_py_wrap_owned(a)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setattr(obj: PyObject, attr: i64, val: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return obj;
    }
    with_gil(|| unsafe {
        let py_val = olive_to_py(val);
        let res = PY_OBJECT_SET_ATTR_STRING(unwrapped_obj, (attr & !1) as *const c_char, py_val);
        if res == -1 {
            handle_py_error();
        }
        PY_DEC_REF(py_val);
        obj
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_bitor(l: PyObject, r: PyObject) -> i64 {
    check_python_loaded();
    let un_l = unsafe { olive_py_unwrap(l) };
    let un_r = unsafe { olive_py_unwrap(r) };
    if un_l.is_null() || un_r.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        let res = crate::python::PY_NUMBER_OR(un_l, un_r);
        if res.is_null() {
            crate::python::python_error::handle_py_error();
        }
        olive_py_wrap_owned(res) as i64
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_sequence(val: i64) -> PyObject {
    check_python_loaded();
    if val == 0 {
        return std::ptr::null_mut();
    }
    if !crate::is_active_object(val) {
        return std::ptr::null_mut();
    }
    unsafe {
        let kind = *(val as *const i64);
        if kind == crate::KIND_LIST || kind == crate::KIND_ANY_LIST {
            let sv = &*(val as *const crate::StableVec);
            let pyl = crate::python::PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let v = *sv.ptr.add(i);
                let py_v = if kind == crate::KIND_ANY_LIST {
                    olive_any_to_py(v)
                } else {
                    olive_to_py(v)
                };
                crate::python::PY_TUPLE_SET_ITEM(pyl, i as isize, py_v);
            }
            pyl
        } else {
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getslice(
    obj_handle: i64,
    start_val: i64,
    stop_val: i64,
    step_val: i64,
    flags: i64,
) -> i64 {
    check_python_loaded();
    let obj = unsafe { olive_py_unwrap(obj_handle as PyObject) };
    if obj.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        let py_none = _PY_NONE_STRUCT;
        let py_start = if flags & 1 != 0 {
            PY_LONG_FROM_LONG(start_val as std::os::raw::c_long)
        } else {
            PY_INC_REF(py_none);
            py_none
        };
        let py_stop = if flags & 2 != 0 {
            PY_LONG_FROM_LONG(stop_val as std::os::raw::c_long)
        } else {
            PY_INC_REF(py_none);
            py_none
        };
        let py_step = if flags & 4 != 0 {
            PY_LONG_FROM_LONG(step_val as std::os::raw::c_long)
        } else {
            PY_INC_REF(py_none);
            py_none
        };
        let slice = PY_SLICE_NEW(py_start, py_stop, py_step);
        PY_DEC_REF(py_start);
        PY_DEC_REF(py_stop);
        PY_DEC_REF(py_step);
        if slice.is_null() {
            handle_py_error();
            return 0;
        }
        let result = PY_OBJECT_GET_ITEM(obj, slice);
        PY_DEC_REF(slice);
        if result.is_null() {
            handle_py_error();
            return 0;
        }
        olive_py_wrap_owned(result) as i64
    })
}
