use crate::python::*;
use std::ffi::CString;
use std::sync::atomic::Ordering;

/// CPython's `PY_VECTORCALL_ARGUMENTS_OFFSET`: a flag OR'd into `nargsf`
/// telling the callee it may treat `args[-1]` as scratch space (used for
/// bound-method optimizations). Not an exported symbol -- a fixed bit
/// pattern from the stable vectorcall ABI, safe to hardcode.
pub(crate) const PY_VECTORCALL_ARGUMENTS_OFFSET: usize = 1usize << (usize::BITS - 1);

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
/// legacy entry point and still drives copy-out. R6: when `HAS_VECTORCALL`,
/// converted args go straight into a stack array for `PyObject_Vectorcall`
/// instead of an allocated tuple; falls back to the tuple path otherwise.
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

        let res = if HAS_VECTORCALL.load(Ordering::Relaxed) {
            // Vectorcall borrows every arg (unlike `PyTuple_SetItem`, which
            // steals), so each converted slot needs its own decref after the
            // call instead of one decref of an owning tuple.
            let mut buf: [PyObject; 17] = [std::ptr::null_mut(); 17];
            let nargs = if args_list != 0 {
                let sv = &*(args_list as *const crate::StableVec);
                for i in 0..sv.len {
                    let coll_tag = tag_at(coll_tags, i);
                    let arg_tag = arg_tag_at(arg_tags, i);
                    let v = *sv.ptr.add(i);
                    let py_v = convert_arg_tagged(v, coll_tag, arg_tag, &mut pairs);
                    if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                        handle_py_error();
                    }
                    buf[i + 1] = py_v;
                    // See `olive_py_call`: a tagged slot aliases the caller's
                    // own allocation, so clear it before this list's own drop
                    // frees every live-looking `Any` element.
                    if coll_tag != TAG_NONE {
                        *sv.ptr.add(i) = 0;
                    }
                }
                sv.len
            } else {
                0
            };
            // Slot 0 is reserved scratch space per the ARGUMENTS_OFFSET
            // contract; the real args start at `buf[1]`.
            let nargsf = nargs | PY_VECTORCALL_ARGUMENTS_OFFSET;
            let r = PY_VECTORCALL(
                unwrapped_func,
                buf.as_ptr().add(1),
                nargsf,
                std::ptr::null_mut(),
            );
            for slot in &buf[1..=nargs] {
                if !slot.is_null() {
                    PY_DEC_REF(*slot);
                }
            }
            r
        } else {
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
                    if coll_tag != TAG_NONE {
                        *sv.ptr.add(i) = 0;
                    }
                }
            }
            let r = PY_OBJECT_CALL_OBJECT(unwrapped_func, py_args);
            if !py_args.is_null() {
                PY_DEC_REF(py_args);
            }
            r
        };

        sync_back(&pairs);

        if res.is_null() {
            handle_py_error();
        } else if !PY_ERR_OCCURRED().is_null() {
            // Some libraries handle exceptions internally yet leave the indicator set.
            PY_ERR_CLEAR();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::python_coerce::pyobject_slab_test_lock;

    /// Forces `HAS_VECTORCALL` to `want` for the duration of `f`, restoring
    /// the previous value after. Must run under `pyobject_slab_test_lock`:
    /// the flag is a process-global static shared with every other
    /// Python-touching test.
    fn with_forced_vectorcall<R>(want: bool, f: impl FnOnce() -> R) -> R {
        let prev = HAS_VECTORCALL.load(Ordering::SeqCst);
        HAS_VECTORCALL.store(want, Ordering::SeqCst);
        let r = f();
        HAS_VECTORCALL.store(prev, Ordering::SeqCst);
        r
    }

    /// `ob_refcnt` is CPython's first `PyObject` field on every non-free-
    /// threaded build; `raw_ob_type` in `python_coerce.rs` already relies on
    /// this same layout for the field right after it.
    unsafe fn raw_refcnt(obj: PyObject) -> isize {
        unsafe { *(obj as *const isize) }
    }

    #[test]
    fn scalar_call_round_trips_both_vectorcall_and_tuple_paths() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        for &forced in &[true, false] {
            with_forced_vectorcall(forced, || unsafe {
                let abs_fn = with_gil(|| {
                    let builtins = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr() as *const _);
                    let f = PY_OBJECT_GET_ATTR_STRING(builtins, b"abs\0".as_ptr() as *const _);
                    PY_DEC_REF(builtins);
                    olive_py_wrap_owned(f)
                });

                let args = crate::olive_list_new(0);
                crate::olive_list_append(args, -7i64);

                let res = olive_py_call_t(abs_fn, args, 0, ARG_INT);
                let val = with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(res)));
                assert_eq!(
                    val,
                    7,
                    "abs(-7) via {} path",
                    if forced { "vectorcall" } else { "tuple" }
                );
                olive_py_decref(res);
                olive_py_decref(abs_fn);
            });
        }
    }

    #[test]
    fn call_t_refcount_stable_across_many_calls_both_paths() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        for &forced in &[true, false] {
            with_forced_vectorcall(forced, || unsafe {
                let (len_fn, target_handle, target_raw) = with_gil(|| {
                    let builtins = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr() as *const _);
                    let len_fn = PY_OBJECT_GET_ATTR_STRING(builtins, b"len\0".as_ptr() as *const _);
                    PY_DEC_REF(builtins);
                    // A fresh list, never interned/cached: its refcount moves
                    // only from what this test does to it.
                    let target = PY_LIST_NEW(0);
                    let handle = olive_py_wrap_owned(target);
                    (olive_py_wrap_owned(len_fn), handle, target)
                });

                let baseline = with_gil(|| raw_refcnt(target_raw));

                for _ in 0..100_000 {
                    let args = crate::olive_list_new(0);
                    crate::olive_list_append(args, target_handle as i64);
                    let res = olive_py_call_t(len_fn, args, 0, ARG_PYOBJECT);
                    olive_py_decref(res);
                }

                let after = with_gil(|| raw_refcnt(target_raw));
                assert_eq!(
                    after,
                    baseline,
                    "refcount leak or over-release across repeated calls via {} path",
                    if forced { "vectorcall" } else { "tuple" }
                );

                olive_py_decref(target_handle);
                olive_py_decref(len_fn);
            });
        }
    }
}
