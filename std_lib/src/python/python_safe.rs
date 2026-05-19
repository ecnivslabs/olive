use crate::python::*;
use std::ffi::CString;
use std::os::raw::c_char;

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_import_safe(name: i64) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    with_gil(|| unsafe {
        let m = PY_IMPORT_IMPORT_MODULE((name & !1) as *const c_char);
        if m.is_null() {
            if let Some(err_msg) = catch_py_exception_msg() {
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }
        let wrapped = olive_py_wrap_owned(m);
        crate::result::olive_result_ok(wrapped as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_safe(func: PyObject, args_list: i64) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null function pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    with_gil(|| unsafe {
        let mut py_args = std::ptr::null_mut();
        if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            py_args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let v = *sv.ptr.add(i);
                let py_v = olive_to_py(v);
                PY_TUPLE_SET_ITEM(py_args, i as isize, py_v);
            }
        }

        let res = PY_OBJECT_CALL_OBJECT(unwrapped_func, py_args);
        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }

        if res.is_null() {
            if let Some(err_msg) = catch_py_exception_msg() {
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }

        let wrapped = olive_py_wrap_owned(res);
        crate::result::olive_result_ok(wrapped as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_kw_safe(func: PyObject, args_list: i64, kwargs_dict: i64) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null function pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    with_gil(|| unsafe {
        let mut py_args = std::ptr::null_mut();
        if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            py_args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let v = *sv.ptr.add(i);
                let py_v = olive_to_py(v);
                PY_TUPLE_SET_ITEM(py_args, i as isize, py_v);
            }
        }

        let mut py_kwargs = std::ptr::null_mut();
        if kwargs_dict != 0 {
            let obj = &*(kwargs_dict as *const crate::OliveObj);
            py_kwargs = PY_DICT_NEW();
            for (k, &v) in &obj.fields {
                let k_str = crate::olive_str_from_ptr(k.0);
                let k_cstr = CString::new(k_str).unwrap();
                let py_v = olive_to_py(v);
                PY_DICT_SET_ITEM_STRING(py_kwargs, k_cstr.as_ptr(), py_v);
                PY_DEC_REF(py_v);
            }
        }

        let res = PY_OBJECT_CALL(unwrapped_func, py_args, py_kwargs);
        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }
        if !py_kwargs.is_null() {
            PY_DEC_REF(py_kwargs);
        }

        if res.is_null() {
            if let Some(err_msg) = catch_py_exception_msg() {
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }

        let wrapped = olive_py_wrap_owned(res);
        crate::result::olive_result_ok(wrapped as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getattr_safe(obj: PyObject, attr: i64) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null object pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    with_gil(|| unsafe {
        let r = PY_OBJECT_GET_ATTR_STRING(unwrapped_obj, (attr & !1) as *const c_char);
        if r.is_null() {
            if let Some(err_msg) = catch_py_exception_msg() {
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }
        let wrapped = olive_py_wrap_owned(r);
        crate::result::olive_result_ok(wrapped as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setattr_safe(obj: PyObject, attr: i64, val: i64) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null object pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    with_gil(|| unsafe {
        let py_val = olive_to_py(val);
        let res = PY_OBJECT_SET_ATTR_STRING(unwrapped_obj, (attr & !1) as *const c_char, py_val);
        PY_DEC_REF(py_val);
        if res == -1 {
            if let Some(err_msg) = catch_py_exception_msg() {
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }
        crate::result::olive_result_ok(obj as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getitem_safe(obj: PyObject, key: PyObject) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    let unwrapped_key = unsafe { olive_py_unwrap(key) };
    if unwrapped_obj.is_null() || unwrapped_key.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null object or key pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    with_gil(|| unsafe {
        let r = PY_OBJECT_GET_ITEM(unwrapped_obj, unwrapped_key);
        if r.is_null() {
            if let Some(err_msg) = catch_py_exception_msg() {
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }
        let wrapped = olive_py_wrap_owned(r);
        crate::result::olive_result_ok(wrapped as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setitem_safe(obj: PyObject, key: PyObject, val: PyObject) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    let unwrapped_key = unsafe { olive_py_unwrap(key) };
    let unwrapped_val = unsafe { olive_py_unwrap(val) };
    if unwrapped_obj.is_null() || unwrapped_key.is_null() || unwrapped_val.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null object, key, or value pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    with_gil(|| unsafe {
        let res = PY_OBJECT_SET_ITEM(unwrapped_obj, unwrapped_key, unwrapped_val);
        if res == -1 {
            if let Some(err_msg) = catch_py_exception_msg() {
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }
        crate::result::olive_result_ok(obj as i64)
    })
}
