use crate::python::*;
#[cfg(test)]
use std::ffi::CString;
use std::os::raw::c_char;
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
        olive_py_gil_begin();

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
        olive_py_gil_end();
        olive_py_wrap_owned(res)
    }
}

/// Shared body for every tagged-fast-path call entry point: the list-based
/// `olive_py_call_t` and the arity-specialized `olive_py_call0..4` shells
/// below. Converts each raw word by its tag, dispatches through
/// `PyObject_Vectorcall` when available or a tuple otherwise, and syncs
/// collection args back. Caller holds the GIL.
///
/// `args` is mutable so a collection-tagged slot that aliases the caller's
/// own list storage (`olive_py_call_t`'s `StableVec` path) can be zeroed in
/// place right after conversion, exactly where the previous per-callsite
/// loops did it -- this stops that list's own drop from freeing an element
/// this call already handed off to Python. The arity shells pass a
/// disposable local array, where the same zeroing is inert.
pub(crate) unsafe fn call_with_raw_args(
    unwrapped_func: PyObject,
    coll_tags: i64,
    arg_tags: i64,
    args: &mut [i64],
) -> PyObject {
    unsafe {
        let mut pairs = Vec::new();

        let res = if HAS_VECTORCALL.load(Ordering::Relaxed) {
            // Vectorcall borrows every arg (unlike `PyTuple_SetItem`, which
            // steals), so each converted slot needs its own decref after the
            // call instead of one decref of an owning tuple.
            let mut buf: [PyObject; 17] = [std::ptr::null_mut(); 17];
            for (i, slot) in args.iter_mut().enumerate() {
                let coll_tag = tag_at(coll_tags, i);
                let arg_tag = arg_tag_at(arg_tags, i);
                let py_v = convert_arg_tagged(*slot, coll_tag, arg_tag, &mut pairs);
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    handle_py_error();
                }
                buf[i + 1] = py_v;
                if coll_tag != TAG_NONE {
                    *slot = 0;
                }
            }
            // Slot 0 is reserved scratch space per the ARGUMENTS_OFFSET
            // contract; the real args start at `buf[1]`.
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
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    handle_py_error();
                }
                PY_TUPLE_SET_ITEM(py_args, i as isize, py_v);
                if coll_tag != TAG_NONE {
                    *slot = 0;
                }
            }
            let r = PY_OBJECT_CALL_OBJECT(unwrapped_func, py_args);
            PY_DEC_REF(py_args);
            r
        };

        sync_back(&pairs);

        if res.is_null() {
            handle_py_error();
        } else if !PY_ERR_OCCURRED().is_null() {
            // Some libraries handle exceptions internally yet leave the indicator set.
            PY_ERR_CLEAR();
        }

        res
    }
}

/// Tagged fast path: `args_list` holds raw, unconverted words (an `int`
/// local's own bits, an `f64` local's bits, a raw string pointer, ...)
/// instead of pre-wrapped `PyObject` handles. `arg_tags` tells `convert_arg_tagged`
/// how to decode each one under this call's single GIL region, replacing the
/// per-arg `__olive_py_from_int`/`_float`/`_str` round trips the legacy path
/// pays before the call even starts. `coll_tags` is unchanged from the
/// legacy entry point and still drives copy-out. When `HAS_VECTORCALL`,
/// converted args go straight into a stack array for `PyObject_Vectorcall`
/// instead of an allocated tuple; falls back to the tuple path otherwise.
/// Kept for arity 5-16 and any call site the compiler can't size at compile
/// time; arity 0-4 uses `olive_py_call0..4` instead, which pass args
/// directly in registers with no list aggregate at all.
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
        olive_py_gil_begin();
        let res = if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            let args = std::slice::from_raw_parts_mut(sv.ptr, sv.len);
            call_with_raw_args(unwrapped_func, coll_tags, arg_tags, args)
        } else {
            call_with_raw_args(unwrapped_func, coll_tags, arg_tags, &mut [])
        };
        olive_py_gil_end();
        olive_py_wrap_owned(res)
    }
}

/// Arity-specialized tagged entry points: the same fast path as
/// `olive_py_call_t`, minus the `args_list` aggregate. The compiler emits
/// these for a positional-only call with 0-4 arguments (the common case),
/// passing each argument straight in a register -- no Olive list allocation
/// per call at all. Thin shells over `call_with_raw_args`; no logic lives
/// here beyond assembling the fixed-size local array and finishing the
/// result per `ret_tag` (packed into `arg_tags`'s top 4 bits -- `olive_py_call0`
/// has no real argument tags to share that word with, so it takes one just to
/// carry `ret_tag`). A fused (non-`RET_HANDLE`) result is converted and
/// decref'd before the GIL releases; `RET_HANDLE` keeps wrapping after
/// release exactly as before, so the unfused path pays nothing extra.
///
/// `loc` (R17) is the call site's `file:line:col` string constant, written
/// to the error-reporting thread-local as the very first action -- no
/// separate `__olive_py_set_loc` call or MIR statement pair precedes these
/// entry points the way it still does for the legacy list-based path.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call0(func: PyObject, arg_tags: i64, loc: i64) -> PyObject {
    set_py_call_loc(loc);
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        olive_py_gil_begin();
        let res = call_with_raw_args(unwrapped_func, 0, 0, &mut []);
        let ret_tag = ret_tag_of(arg_tags);
        if ret_tag == RET_HANDLE {
            olive_py_gil_end();
            return olive_py_wrap_owned(res);
        }
        let out = finish_ret(res, ret_tag);
        olive_py_gil_end();
        out as PyObject
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call1(
    func: PyObject,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    loc: i64,
) -> PyObject {
    set_py_call_loc(loc);
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        olive_py_gil_begin();
        let mut args = [a0];
        let res = call_with_raw_args(unwrapped_func, coll_tags, arg_tags, &mut args);
        let ret_tag = ret_tag_of(arg_tags);
        if ret_tag == RET_HANDLE {
            olive_py_gil_end();
            return olive_py_wrap_owned(res);
        }
        let out = finish_ret(res, ret_tag);
        olive_py_gil_end();
        out as PyObject
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call2(
    func: PyObject,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    a1: i64,
    loc: i64,
) -> PyObject {
    set_py_call_loc(loc);
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        olive_py_gil_begin();
        let mut args = [a0, a1];
        let res = call_with_raw_args(unwrapped_func, coll_tags, arg_tags, &mut args);
        let ret_tag = ret_tag_of(arg_tags);
        if ret_tag == RET_HANDLE {
            olive_py_gil_end();
            return olive_py_wrap_owned(res);
        }
        let out = finish_ret(res, ret_tag);
        olive_py_gil_end();
        out as PyObject
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call3(
    func: PyObject,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    loc: i64,
) -> PyObject {
    set_py_call_loc(loc);
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        olive_py_gil_begin();
        let mut args = [a0, a1, a2];
        let res = call_with_raw_args(unwrapped_func, coll_tags, arg_tags, &mut args);
        let ret_tag = ret_tag_of(arg_tags);
        if ret_tag == RET_HANDLE {
            olive_py_gil_end();
            return olive_py_wrap_owned(res);
        }
        let out = finish_ret(res, ret_tag);
        olive_py_gil_end();
        out as PyObject
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call4(
    func: PyObject,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    a3: i64,
    loc: i64,
) -> PyObject {
    set_py_call_loc(loc);
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        olive_py_gil_begin();
        let mut args = [a0, a1, a2, a3];
        let res = call_with_raw_args(unwrapped_func, coll_tags, arg_tags, &mut args);
        let ret_tag = ret_tag_of(arg_tags);
        if ret_tag == RET_HANDLE {
            olive_py_gil_end();
            return olive_py_wrap_owned(res);
        }
        let out = finish_ret(res, ret_tag);
        olive_py_gil_end();
        out as PyObject
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
        call_kw_dict(
            unwrapped_func,
            args_list,
            coll_tags,
            kwargs_dict,
            kw_coll_tags,
        )
    }
}

/// Dict-building kwargs call, shared by `olive_py_call_kw` and the R15
/// vectorcall entry points' fallback lane (missing vectorcall/interning,
/// or a kwnames tuple that failed to build). `unwrapped_func` is already
/// unwrapped and checked non-null.
pub(crate) unsafe fn call_kw_dict(
    unwrapped_func: PyObject,
    args_list: i64,
    coll_tags: i64,
    kwargs_dict: i64,
    kw_coll_tags: i64,
) -> PyObject {
    unsafe {
        olive_py_gil_begin();

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

                let py_v = convert_arg(v, tag, &mut pairs);
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    handle_py_error();
                }

                // Olive strings are always NUL-terminated at their raw address; no copy needed.
                PY_DICT_SET_ITEM_STRING(py_kwargs, crate::string_slab::str_body(k_ptr) as *const c_char, py_v);
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

        olive_py_gil_end();
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

    /// Runs `src` in `__main__` and returns a wrapped handle to `name`
    /// defined there. Real builtins rarely have the exact arity a test
    /// wants; a throwaway `__main__` function gives an exact, known one.
    unsafe fn eval_main_fn(src: &str, name: &str) -> PyObject {
        unsafe {
            let c_src = CString::new(src).unwrap();
            PY_RUN_SIMPLE_STRING(c_src.as_ptr());
            let main_mod = PY_IMPORT_IMPORT_MODULE(b"__main__\0".as_ptr() as *const _);
            let c_name = CString::new(name).unwrap();
            let f = PY_OBJECT_GET_ATTR_STRING(main_mod, c_name.as_ptr());
            PY_DEC_REF(main_mod);
            olive_py_wrap_owned(f)
        }
    }

    #[test]
    fn arity_shells_round_trip_scalars() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let (h0, h1, h2, h3, h4) = with_gil(|| {
                (
                    eval_main_fn("def __t_h0():\n    return 111\n", "__t_h0"),
                    eval_main_fn("def __t_h1(a):\n    return a + 1\n", "__t_h1"),
                    eval_main_fn("def __t_h2(a, b):\n    return a + b\n", "__t_h2"),
                    eval_main_fn("def __t_h3(a, b, c):\n    return a + b + c\n", "__t_h3"),
                    eval_main_fn(
                        "def __t_h4(a, b, c, d):\n    return a + b + c + d\n",
                        "__t_h4",
                    ),
                )
            });

            let r0 = olive_py_call0(h0, 0, 0);
            assert_eq!(with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(r0))), 111);
            olive_py_decref(r0);

            let r1 = olive_py_call1(h1, 0, ARG_INT, 41, 0);
            assert_eq!(with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(r1))), 42);
            olive_py_decref(r1);

            let tags2 = ARG_INT | (ARG_INT << 4);
            let r2 = olive_py_call2(h2, 0, tags2, 10, 20, 0);
            assert_eq!(with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(r2))), 30);
            olive_py_decref(r2);

            let tags3 = ARG_INT | (ARG_INT << 4) | (ARG_INT << 8);
            let r3 = olive_py_call3(h3, 0, tags3, 1, 2, 3, 0);
            assert_eq!(with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(r3))), 6);
            olive_py_decref(r3);

            let tags4 = ARG_INT | (ARG_INT << 4) | (ARG_INT << 8) | (ARG_INT << 12);
            let r4 = olive_py_call4(h4, 0, tags4, 1, 2, 3, 4, 0);
            assert_eq!(with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(r4))), 10);
            olive_py_decref(r4);

            olive_py_decref(h0);
            olive_py_decref(h1);
            olive_py_decref(h2);
            olive_py_decref(h3);
            olive_py_decref(h4);
        }
    }

    #[test]
    fn arity_shell_collection_arg_still_syncs_back() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let sort_fn = with_gil(|| {
                eval_main_fn(
                    "def __t_sort_inplace(xs):\n    xs.sort()\n",
                    "__t_sort_inplace",
                )
            });

            let xs = crate::olive_list_new(0);
            crate::olive_list_append(xs, 3i64);
            crate::olive_list_append(xs, 1i64);
            crate::olive_list_append(xs, 2i64);

            let res = olive_py_call1(sort_fn, TAG_INT_LIST, ARG_PYOBJECT, xs, 0);
            olive_py_decref(res);

            assert_eq!(crate::olive_list_len(xs), 3);
            assert_eq!(crate::olive_list_get(xs, 0), 1);
            assert_eq!(crate::olive_list_get(xs, 1), 2);
            assert_eq!(crate::olive_list_get(xs, 2), 3);

            olive_py_decref(sort_fn);
        }
    }

    #[test]
    fn arity1_refcount_stable_across_many_pyobject_calls() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let (len_fn, target_handle, target_raw) = with_gil(|| {
                let builtins = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr() as *const _);
                let len_fn = PY_OBJECT_GET_ATTR_STRING(builtins, b"len\0".as_ptr() as *const _);
                PY_DEC_REF(builtins);
                let target = PY_LIST_NEW(0);
                let handle = olive_py_wrap_owned(target);
                (olive_py_wrap_owned(len_fn), handle, target)
            });

            let baseline = with_gil(|| raw_refcnt(target_raw));

            for _ in 0..100_000 {
                let res = olive_py_call1(len_fn, 0, ARG_PYOBJECT, target_handle as i64, 0);
                olive_py_decref(res);
            }

            let after = with_gil(|| raw_refcnt(target_raw));
            assert_eq!(
                after, baseline,
                "refcount leak or over-release across repeated olive_py_call1 calls"
            );

            olive_py_decref(target_handle);
            olive_py_decref(len_fn);
        }
    }
}
