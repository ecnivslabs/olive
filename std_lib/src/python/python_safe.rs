use crate::python::*;
use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::atomic::Ordering;

/// Err result for a failed olive-to-Python argument conversion.
pub(crate) unsafe fn conversion_err() -> i64 {
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
    unsafe {
        call_kw_dict_safe(
            unwrapped_func,
            args_list,
            coll_tags,
            kwargs_dict,
            kw_coll_tags,
        )
    }
}

/// `Result`-returning dict-building kwargs call, shared by
/// `olive_py_call_kw_safe` and the R15 vectorcall entry points' fallback
/// lane. `unwrapped_func` is already unwrapped and checked non-null.
pub(crate) unsafe fn call_kw_dict_safe(
    unwrapped_func: PyObject,
    args_list: i64,
    coll_tags: i64,
    kwargs_dict: i64,
    kw_coll_tags: i64,
) -> i64 {
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

/// `Result`-returning counterpart to `python_call::call_with_raw_args`,
/// shared by `olive_py_call_t_safe` and the arity-specialized
/// `olive_py_call0..4_safe` shells. `Err` means argument conversion failed
/// before the Python call ever ran: every partially-converted slot has
/// already been released and `pairs` abandoned, so the caller need only
/// propagate the encoded error.
pub(crate) unsafe fn call_with_raw_args_safe(
    unwrapped_func: PyObject,
    coll_tags: i64,
    arg_tags: i64,
    args: &mut [i64],
) -> Result<PyObject, i64> {
    unsafe {
        let mut pairs = Vec::new();

        let res = if HAS_VECTORCALL.load(Ordering::Relaxed) {
            let mut buf: [PyObject; 17] = [std::ptr::null_mut(); 17];
            for (i, slot) in args.iter_mut().enumerate() {
                let coll_tag = tag_at(coll_tags, i);
                let arg_tag = arg_tag_at(arg_tags, i);
                let py_v = convert_arg_tagged(*slot, coll_tag, arg_tag, &mut pairs);
                // See `olive_py_call_safe`: zero a tagged, aliased slot
                // before any early return, ahead of this list's own drop.
                if coll_tag != TAG_NONE {
                    *slot = 0;
                }
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    if !py_v.is_null() {
                        PY_DEC_REF(py_v);
                    }
                    for s in &buf[1..=i] {
                        if !s.is_null() {
                            PY_DEC_REF(*s);
                        }
                    }
                    abandon_pairs(&pairs);
                    return Err(conversion_err());
                }
                buf[i + 1] = py_v;
            }
            let nargsf = args.len() | PY_VECTORCALL_ARGUMENTS_OFFSET;
            let r = PY_VECTORCALL(
                unwrapped_func,
                buf.as_ptr().add(1),
                nargsf,
                std::ptr::null_mut(),
            );
            for slot in &buf[1..=args.len()] {
                if !slot.is_null() {
                    PY_DEC_REF(*slot);
                }
            }
            r
        } else {
            let py_args = PY_TUPLE_NEW(args.len() as isize);
            for (i, slot) in args.iter_mut().enumerate() {
                let coll_tag = tag_at(coll_tags, i);
                let arg_tag = arg_tag_at(arg_tags, i);
                let py_v = convert_arg_tagged(*slot, coll_tag, arg_tag, &mut pairs);
                // The compiler aliased a tagged slot from the caller's own
                // allocation (not a defensive copy) so `sync_back` mutates
                // the value the caller keeps using; zero it here, before
                // any early return, so this list's own drop -- which frees
                // every live-looking `Any` element -- doesn't also free the
                // caller's copy out from under it.
                if coll_tag != TAG_NONE {
                    *slot = 0;
                }
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    if !py_v.is_null() {
                        PY_DEC_REF(py_v);
                    }
                    PY_DEC_REF(py_args);
                    abandon_pairs(&pairs);
                    return Err(conversion_err());
                }
                PY_TUPLE_SET_ITEM(py_args, i as isize, py_v);
            }
            let r = PY_OBJECT_CALL_OBJECT(unwrapped_func, py_args);
            PY_DEC_REF(py_args);
            r
        };

        sync_back(&pairs);
        Ok(res)
    }
}

/// Wraps a `call_with_raw_args_safe` outcome into the `Result<PyObject, ()>`
/// wire encoding every `_safe` py-call entry point returns. `ret_tag` fuses
/// the `Ok` payload exactly like the non-`_safe` twin: `RET_HANDLE` wraps a
/// handle as before, anything else converts and decrefs the raw result
/// before it's packed into the `Ok` slot.
pub(crate) unsafe fn finish_call_safe(outcome: Result<PyObject, i64>, ret_tag: i64) -> i64 {
    unsafe {
        let res = match outcome {
            Ok(res) => res,
            Err(err) => return err,
        };
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
        if ret_tag == RET_HANDLE {
            let wrapped = olive_py_wrap_owned(res);
            crate::result::olive_result_ok(wrapped as i64)
        } else {
            crate::result::olive_result_ok(finish_ret(res, ret_tag))
        }
    }
}

/// Tagged fast path, `Result`-returning; see `olive_py_call_t`. Vectorcall-
/// aware, same as the non-safe twin. Kept for arity 5-16; arity 0-4 uses
/// `olive_py_call0..4_safe` instead.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_t_safe(
    func: PyObject,
    args_list: i64,
    coll_tags: i64,
    arg_tags: i64,
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
        let outcome = if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            let args = std::slice::from_raw_parts_mut(sv.ptr, sv.len);
            call_with_raw_args_safe(unwrapped_func, coll_tags, arg_tags, args)
        } else {
            call_with_raw_args_safe(unwrapped_func, coll_tags, arg_tags, &mut [])
        };
        finish_call_safe(outcome, RET_HANDLE)
    })
}

/// `Result`-returning arity-specialized shells; see `olive_py_call0..4`.
/// `arg_tags`'s top 4 bits carry `ret_tag`, exactly like the non-`_safe`
/// twin; `olive_py_call0_safe` takes the word purely to carry it.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call0_safe(func: PyObject, arg_tags: i64) -> i64 {
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
        let outcome = call_with_raw_args_safe(unwrapped_func, 0, 0, &mut []);
        finish_call_safe(outcome, ret_tag_of(arg_tags))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call1_safe(
    func: PyObject,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
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
        let mut args = [a0];
        let outcome = call_with_raw_args_safe(unwrapped_func, coll_tags, arg_tags, &mut args);
        finish_call_safe(outcome, ret_tag_of(arg_tags))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call2_safe(
    func: PyObject,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    a1: i64,
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
        let mut args = [a0, a1];
        let outcome = call_with_raw_args_safe(unwrapped_func, coll_tags, arg_tags, &mut args);
        finish_call_safe(outcome, ret_tag_of(arg_tags))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call3_safe(
    func: PyObject,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    a1: i64,
    a2: i64,
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
        let mut args = [a0, a1, a2];
        let outcome = call_with_raw_args_safe(unwrapped_func, coll_tags, arg_tags, &mut args);
        finish_call_safe(outcome, ret_tag_of(arg_tags))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call4_safe(
    func: PyObject,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    a3: i64,
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
        let mut args = [a0, a1, a2, a3];
        let outcome = call_with_raw_args_safe(unwrapped_func, coll_tags, arg_tags, &mut args);
        finish_call_safe(outcome, ret_tag_of(arg_tags))
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
    let attr_ptr = (attr & !1) as *const c_char;
    with_gil(|| unsafe {
        let r = if HAS_INTERN.load(Ordering::Relaxed) {
            let name = interned_attr(attr_ptr);
            if name.is_null() {
                std::ptr::null_mut()
            } else {
                PY_OBJECT_GET_ATTR(unwrapped_obj, name)
            }
        } else {
            PY_OBJECT_GET_ATTR_STRING(unwrapped_obj, attr_ptr)
        };
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
    let attr_ptr = (attr & !1) as *const c_char;
    with_gil(|| unsafe {
        let py_val = olive_to_py(val);
        if py_val.is_null() || !PY_ERR_OCCURRED().is_null() {
            if !py_val.is_null() {
                PY_DEC_REF(py_val);
            }
            return conversion_err();
        }
        let res = if HAS_INTERN.load(Ordering::Relaxed) {
            let name = interned_attr(attr_ptr);
            if name.is_null() {
                -1
            } else {
                PY_OBJECT_SET_ATTR(unwrapped_obj, name, py_val)
            }
        } else {
            PY_OBJECT_SET_ATTR_STRING(unwrapped_obj, attr_ptr, py_val)
        };
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

    /// Forces `HAS_VECTORCALL` to `want` for the duration of `f`, restoring
    /// the previous value after. Mirrors `python_call::tests`'s helper of the
    /// same name; must run under `pyobject_slab_test_lock`.
    fn with_forced_vectorcall<R>(want: bool, f: impl FnOnce() -> R) -> R {
        let prev = HAS_VECTORCALL.load(Ordering::SeqCst);
        HAS_VECTORCALL.store(want, Ordering::SeqCst);
        let r = f();
        HAS_VECTORCALL.store(prev, Ordering::SeqCst);
        r
    }

    #[test]
    fn arity_safe_shells_round_trip_scalars() {
        let _guard = crate::python::python_coerce::pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let c_src = CString::new(
                "def __t_safe_h0():\n    return 5\ndef __t_safe_h2(a, b):\n    return a * b\n",
            )
            .unwrap();
            let (h0, h2) = with_gil(|| {
                PY_RUN_SIMPLE_STRING(c_src.as_ptr());
                let main_mod = PY_IMPORT_IMPORT_MODULE(b"__main__\0".as_ptr() as *const _);
                let f0 = PY_OBJECT_GET_ATTR_STRING(main_mod, b"__t_safe_h0\0".as_ptr() as *const _);
                let f2 = PY_OBJECT_GET_ATTR_STRING(main_mod, b"__t_safe_h2\0".as_ptr() as *const _);
                PY_DEC_REF(main_mod);
                (olive_py_wrap_owned(f0), olive_py_wrap_owned(f2))
            });

            let r0 = olive_py_call0_safe(h0, 0);
            assert_eq!(crate::result::olive_result_is_err(r0), 0);
            let ok0 = crate::result::olive_result_unwrap(r0);
            assert_eq!(
                with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(ok0 as PyObject))),
                5
            );
            olive_py_decref(ok0 as PyObject);

            let tags2 = ARG_INT | (ARG_INT << 4);
            let r2 = olive_py_call2_safe(h2, 0, tags2, 6, 7);
            assert_eq!(crate::result::olive_result_is_err(r2), 0);
            let ok2 = crate::result::olive_result_unwrap(r2);
            assert_eq!(
                with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(ok2 as PyObject))),
                42
            );
            olive_py_decref(ok2 as PyObject);

            olive_py_decref(h0);
            olive_py_decref(h2);
        }
    }

    #[test]
    fn arity1_safe_rejects_bad_str_arg_and_clears_error() {
        let _guard = crate::python::python_coerce::pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        for &forced in &[true, false] {
            with_forced_vectorcall(forced, || unsafe {
                let bad = crate::string_slab::str_alloc(&[0xe0]);
                let func = with_gil(|| {
                    let builtins = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr() as *const _);
                    let f = PY_OBJECT_GET_ATTR_STRING(builtins, b"len\0".as_ptr() as *const _);
                    PY_DEC_REF(builtins);
                    olive_py_wrap_owned(f)
                });

                let res = olive_py_call1_safe(func, 0, ARG_STR, bad);
                assert_eq!(
                    crate::result::olive_result_is_err(res),
                    1,
                    "corrupt argument must fail the call via {} path",
                    if forced { "vectorcall" } else { "tuple" }
                );
                let msg = crate::olive_str_from_ptr(crate::result::olive_result_err_msg(res));
                assert!(
                    msg.contains("UnicodeDecodeError") || msg.contains("utf-8"),
                    "error names the decode failure: {msg}"
                );
                with_gil(|| {
                    assert!(
                        PY_ERR_OCCURRED().is_null(),
                        "no exception may stay pending to poison later calls"
                    );
                });
                olive_py_decref(func);
            });
        }
    }
}
