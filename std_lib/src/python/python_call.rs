use crate::python::*;
use std::ffi::CString;

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call(func: PyObject, args_list: i64, coll_tags: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();

        let mut pairs = Vec::new();
        let mut py_args = std::ptr::null_mut();
        if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            py_args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let tag = tag_at(coll_tags, i);
                let v = *sv.ptr.add(i);
                let py_v = convert_arg(v, tag, &mut pairs);
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    handle_py_error();
                }
                PY_TUPLE_SET_ITEM(py_args, i as isize, py_v);
                // The compiler aliased this slot from the caller's own
                // allocation (not a defensive copy) so `sync_back` mutates
                // the value the caller keeps using; zero it here so this
                // list's own drop, which frees every live-looking `Any`
                // element, doesn't also free the caller's copy out from
                // under it.
                if tag != TAG_NONE {
                    *sv.ptr.add(i) = 0;
                }
            }
        }

        let res = PY_OBJECT_CALL_OBJECT(unwrapped_func, py_args);
        sync_back(&pairs);

        if res.is_null() {
            handle_py_error();
        } else if !PY_ERR_OCCURRED().is_null() {
            // Some libraries handle exceptions internally yet leave the indicator set.
            PY_ERR_CLEAR();
        }

        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(res)
    }
}

/// R5 tagged fast path: `args_list` holds raw, unconverted words (an `int`
/// local's own bits, an `f64` local's bits, a raw string pointer, ...)
/// instead of pre-wrapped `PyObject` handles. `arg_tags` tells `convert_arg_tagged`
/// how to decode each one under this call's single GIL region, replacing the
/// per-arg `__olive_py_from_int`/`_float`/`_str` round trips the legacy path
/// pays before the call even starts. `coll_tags` is unchanged from the
/// legacy entry point and still drives copy-out.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_t(
    func: PyObject,
    args_list: i64,
    coll_tags: i64,
    arg_tags: i64,
) -> PyObject {
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();

        let mut pairs = Vec::new();
        let mut py_args = std::ptr::null_mut();
        if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            py_args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let coll_tag = tag_at(coll_tags, i);
                let arg_tag = arg_tag_at(arg_tags, i);
                let v = *sv.ptr.add(i);
                let py_v = convert_arg_tagged(v, coll_tag, arg_tag, &mut pairs);
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    handle_py_error();
                }
                PY_TUPLE_SET_ITEM(py_args, i as isize, py_v);
                // See `olive_py_call`: a tagged slot aliases the caller's own
                // allocation, so clear it before this list's own drop frees
                // every live-looking `Any` element.
                if coll_tag != TAG_NONE {
                    *sv.ptr.add(i) = 0;
                }
            }
        }

        let res = PY_OBJECT_CALL_OBJECT(unwrapped_func, py_args);
        sync_back(&pairs);

        if res.is_null() {
            handle_py_error();
        } else if !PY_ERR_OCCURRED().is_null() {
            // Some libraries handle exceptions internally yet leave the indicator set.
            PY_ERR_CLEAR();
        }

        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(res)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_kw(
    func: PyObject,
    args_list: i64,
    coll_tags: i64,
    kwargs_dict: i64,
    kw_coll_tags: i64,
) -> PyObject {
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();

        let mut pairs = Vec::new();
        let py_args = if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            let args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let tag = tag_at(coll_tags, i);
                let v = *sv.ptr.add(i);
                let py_v = convert_arg(v, tag, &mut pairs);
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    handle_py_error();
                }
                PY_TUPLE_SET_ITEM(args, i as isize, py_v);
                // See `olive_py_call`: this slot aliases the caller's own
                // allocation, so clear it before this list's own drop frees
                // every live-looking `Any` element.
                if tag != TAG_NONE {
                    *sv.ptr.add(i) = 0;
                }
            }
            args
        } else {
            PY_TUPLE_NEW(0)
        };

        let mut py_kwargs = std::ptr::null_mut();
        if kwargs_dict != 0 {
            let sv = &*(kwargs_dict as *const crate::StableVec);
            py_kwargs = PY_DICT_NEW();
            for (kw_i, i) in (0..sv.len).step_by(2).enumerate() {
                let k_ptr = *sv.ptr.add(i);
                let tag = tag_at(kw_coll_tags, kw_i);
                let v = *sv.ptr.add(i + 1);

                let k_str = crate::olive_str_from_ptr(k_ptr);
                let k_cstr = CString::new(k_str).unwrap();
                let py_v = convert_arg(v, tag, &mut pairs);
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    handle_py_error();
                }

                PY_DICT_SET_ITEM_STRING(py_kwargs, k_cstr.as_ptr(), py_v);
                PY_DEC_REF(py_v);
                if tag != TAG_NONE {
                    *sv.ptr.add(i + 1) = 0;
                }
            }
        }

        let res = PY_OBJECT_CALL(unwrapped_func, py_args, py_kwargs);
        sync_back(&pairs);

        if res.is_null() {
            handle_py_error();
        } else if !PY_ERR_OCCURRED().is_null() {
            // Some libraries handle exceptions internally yet leave the indicator set.
            PY_ERR_CLEAR();
        }

        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }
        if !py_kwargs.is_null() {
            PY_DEC_REF(py_kwargs);
        }

        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(res)
    }
}

/// R5 tagged fast path for a keyword call; see `olive_py_call_t`. Positional
/// and keyword raw words each carry their own encode-tag word, mirroring the
/// legacy pair's separate `coll_tags`/`kw_coll_tags`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_kw_t(
    func: PyObject,
    args_list: i64,
    coll_tags: i64,
    arg_tags: i64,
    kwargs_dict: i64,
    kw_coll_tags: i64,
    kw_arg_tags: i64,
) -> PyObject {
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();

        let mut pairs = Vec::new();
        let py_args = if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            let args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let coll_tag = tag_at(coll_tags, i);
                let arg_tag = arg_tag_at(arg_tags, i);
                let v = *sv.ptr.add(i);
                let py_v = convert_arg_tagged(v, coll_tag, arg_tag, &mut pairs);
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    handle_py_error();
                }
                PY_TUPLE_SET_ITEM(args, i as isize, py_v);
                if coll_tag != TAG_NONE {
                    *sv.ptr.add(i) = 0;
                }
            }
            args
        } else {
            PY_TUPLE_NEW(0)
        };

        let mut py_kwargs = std::ptr::null_mut();
        if kwargs_dict != 0 {
            let sv = &*(kwargs_dict as *const crate::StableVec);
            py_kwargs = PY_DICT_NEW();
            for (kw_i, i) in (0..sv.len).step_by(2).enumerate() {
                let k_ptr = *sv.ptr.add(i);
                let coll_tag = tag_at(kw_coll_tags, kw_i);
                let arg_tag = arg_tag_at(kw_arg_tags, kw_i);
                let v = *sv.ptr.add(i + 1);

                let k_str = crate::olive_str_from_ptr(k_ptr);
                let k_cstr = CString::new(k_str).unwrap();
                let py_v = convert_arg_tagged(v, coll_tag, arg_tag, &mut pairs);
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    handle_py_error();
                }

                PY_DICT_SET_ITEM_STRING(py_kwargs, k_cstr.as_ptr(), py_v);
                PY_DEC_REF(py_v);
                if coll_tag != TAG_NONE {
                    *sv.ptr.add(i + 1) = 0;
                }
            }
        }

        let res = PY_OBJECT_CALL(unwrapped_func, py_args, py_kwargs);
        sync_back(&pairs);

        if res.is_null() {
            handle_py_error();
        } else if !PY_ERR_OCCURRED().is_null() {
            // Some libraries handle exceptions internally yet leave the indicator set.
            PY_ERR_CLEAR();
        }

        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }
        if !py_kwargs.is_null() {
            PY_DEC_REF(py_kwargs);
        }

        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(res)
    }
}
