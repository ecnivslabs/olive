use crate::python::*;
use std::ffi::CString;
use std::os::raw::c_char;

/// Err result for a failed olive-to-Python argument conversion.
unsafe fn conversion_err() -> i64 {
    let msg = unsafe { catch_py_exception_msg() }
        .unwrap_or_else(|| "argument conversion failed".to_string());
    crate::result::olive_result_err(crate::olive_str_internal(&msg))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_import_safe(name: i64) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    with_gil(|| unsafe {
        let m = PY_IMPORT_IMPORT_MODULE((name & !1) as *const c_char);
        if m.is_null()
            && let Some(err_msg) = catch_py_exception_msg()
        {
            let err_str_ptr = crate::olive_str_internal(&err_msg);
            return crate::result::olive_result_err(err_str_ptr);
        }
        let wrapped = olive_py_wrap_owned(m);
        crate::result::olive_result_ok(wrapped as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_safe(func: PyObject, args_list: i64, coll_tags: i64) -> i64 {
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
        let mut pairs = Vec::new();
        let mut py_args = std::ptr::null_mut();
        if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            py_args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let tag = tag_at(coll_tags, i);
                let v = *sv.ptr.add(i);
                let py_v = convert_arg(v, tag, &mut pairs);
                // The compiler aliased a tagged slot from the caller's own
                // allocation (not a defensive copy) so `sync_back` mutates
                // the value the caller keeps using; zero it here, before
                // any early return, so this list's own drop -- which frees
                // every live-looking `Any` element -- doesn't also free the
                // caller's copy out from under it.
                if tag != TAG_NONE {
                    *sv.ptr.add(i) = 0;
                }
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    if !py_v.is_null() {
                        PY_DEC_REF(py_v);
                    }
                    PY_DEC_REF(py_args);
                    abandon_pairs(&pairs);
                    return conversion_err();
                }
                PY_TUPLE_SET_ITEM(py_args, i as isize, py_v);
            }
        }

        let res = PY_OBJECT_CALL_OBJECT(unwrapped_func, py_args);
        sync_back(&pairs);
        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }

        if res.is_null()
            && let Some(err_msg) = catch_py_exception_msg()
        {
            let err_str_ptr = crate::olive_str_internal(&err_msg);
            return crate::result::olive_result_err(err_str_ptr);
        }

        // A successful call must not leave the Python error indicator set; some
        // libraries (e.g. yt-dlp) raise and handle exceptions internally yet
        // leave it lingering, which would derail the next C-API call.
        if !PY_ERR_OCCURRED().is_null() {
            PY_ERR_CLEAR();
        }

        let wrapped = olive_py_wrap_owned(res);
        crate::result::olive_result_ok(wrapped as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_kw_safe(
    func: PyObject,
    args_list: i64,
    coll_tags: i64,
    kwargs_dict: i64,
    kw_coll_tags: i64,
) -> i64 {
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
        let mut pairs = Vec::new();
        let mut py_args = std::ptr::null_mut();
        if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            py_args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let tag = tag_at(coll_tags, i);
                let v = *sv.ptr.add(i);
                let py_v = convert_arg(v, tag, &mut pairs);
                // See `olive_py_call_safe`: zero a tagged, aliased slot
                // before any early return, ahead of this list's own drop.
                if tag != TAG_NONE {
                    *sv.ptr.add(i) = 0;
                }
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    if !py_v.is_null() {
                        PY_DEC_REF(py_v);
                    }
                    PY_DEC_REF(py_args);
                    abandon_pairs(&pairs);
                    return conversion_err();
                }
                PY_TUPLE_SET_ITEM(py_args, i as isize, py_v);
            }
        }

        let mut py_kwargs = std::ptr::null_mut();
        if kwargs_dict != 0 {
            let sv = &*(kwargs_dict as *const crate::StableVec);
            py_kwargs = PY_DICT_NEW();
            let mut i = 0;
            let mut kw_i = 0;
            while i + 1 < sv.len {
                let key = *sv.ptr.add(i);
                let tag = tag_at(kw_coll_tags, kw_i);
                let val = *sv.ptr.add(i + 1);
                let k_str = crate::olive_str_from_ptr(key);
                let k_cstr = CString::new(k_str).unwrap();
                let py_v = convert_arg(val, tag, &mut pairs);
                if tag != TAG_NONE {
                    *sv.ptr.add(i + 1) = 0;
                }
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    if !py_v.is_null() {
                        PY_DEC_REF(py_v);
                    }
                    if !py_args.is_null() {
                        PY_DEC_REF(py_args);
                    }
                    PY_DEC_REF(py_kwargs);
                    abandon_pairs(&pairs);
                    return conversion_err();
                }
                PY_DICT_SET_ITEM_STRING(py_kwargs, k_cstr.as_ptr(), py_v);
                PY_DEC_REF(py_v);
                i += 2;
                kw_i += 1;
            }
        }

        let res = PY_OBJECT_CALL(unwrapped_func, py_args, py_kwargs);
        sync_back(&pairs);
        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }
        if !py_kwargs.is_null() {
            PY_DEC_REF(py_kwargs);
        }

        if res.is_null()
            && let Some(err_msg) = catch_py_exception_msg()
        {
            let err_str_ptr = crate::olive_str_internal(&err_msg);
            return crate::result::olive_result_err(err_str_ptr);
        }

        // A successful call must not leave the Python error indicator set; some
        // libraries (e.g. yt-dlp) raise and handle exceptions internally yet
        // leave it lingering, which would derail the next C-API call.
        if !PY_ERR_OCCURRED().is_null() {
            PY_ERR_CLEAR();
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
        if r.is_null()
            && let Some(err_msg) = catch_py_exception_msg()
        {
            let err_str_ptr = crate::olive_str_internal(&err_msg);
            return crate::result::olive_result_err(err_str_ptr);
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
        if py_val.is_null() || !PY_ERR_OCCURRED().is_null() {
            if !py_val.is_null() {
                PY_DEC_REF(py_val);
            }
            return conversion_err();
        }
        let res = PY_OBJECT_SET_ATTR_STRING(unwrapped_obj, (attr & !1) as *const c_char, py_val);
        PY_DEC_REF(py_val);
        if res == -1
            && let Some(err_msg) = catch_py_exception_msg()
        {
            let err_str_ptr = crate::olive_str_internal(&err_msg);
            return crate::result::olive_result_err(err_str_ptr);
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
        if r.is_null()
            && let Some(err_msg) = catch_py_exception_msg()
        {
            let err_str_ptr = crate::olive_str_internal(&err_msg);
            return crate::result::olive_result_err(err_str_ptr);
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
        if res == -1
            && let Some(err_msg) = catch_py_exception_msg()
        {
            let err_str_ptr = crate::olive_str_internal(&err_msg);
            return crate::result::olive_result_err(err_str_ptr);
        }
        crate::result::olive_result_ok(obj as i64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_safe_rejects_bad_utf8_arg_and_clears_error() {
        let _guard = crate::python::python_coerce::pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        let bad = crate::string_slab::str_alloc(&[0xe0]);
        let args = crate::olive_list_new(0);
        crate::olive_list_append(args, bad);
        let func = with_gil(|| unsafe {
            let builtins = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr() as *const _);
            let f = PY_OBJECT_GET_ATTR_STRING(builtins, b"len\0".as_ptr() as *const _);
            PY_DEC_REF(builtins);
            olive_py_wrap_owned(f)
        });
        let res = olive_py_call_safe(func, args, 0);
        assert_eq!(
            crate::result::olive_result_is_err(res),
            1,
            "corrupt argument must fail the call"
        );
        let msg = crate::olive_str_from_ptr(crate::result::olive_result_err_msg(res));
        assert!(
            msg.contains("UnicodeDecodeError") || msg.contains("utf-8"),
            "error names the decode failure: {msg}"
        );
        with_gil(|| unsafe {
            assert!(
                PY_ERR_OCCURRED().is_null(),
                "no exception may stay pending to poison later calls"
            );
        });
    }
}
