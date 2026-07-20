//! Keyword-argument fast path (R15): `PyObject_Vectorcall`/
//! `PyObject_VectorcallMethod` with a cached `kwnames` tuple
//! (`python_kwnames.rs`), replacing the dict `olive_py_call_kw` used to
//! build for every kwargs call. Positional and keyword values convert
//! into one shared stack buffer -- the vectorcall convention for a
//! keyword argument's value is that it sits right after the positional
//! args, in the same order as the paired `kwnames` tuple. Falls back to
//! building a real `dict` (the pre-R15 path) when vectorcall or interning
//! isn't available, exactly like every other vectorcall-gated entry point.

use crate::python::python_call_kw_core::{
    call_kw_v_core, call_kw_v_core_safe, call_kw_v_method_core, call_kw_v_method_core_safe,
};
use crate::python::python_call_kw_legacy::{
    legacy_call_kw, legacy_call_kw_safe, legacy_call_method_kw, legacy_call_method_kw_safe,
};
use crate::python::python_kwnames::kwnames_tuple;
use crate::python::*;
use std::os::raw::c_char;
use std::sync::atomic::Ordering;

/// Whether the vectorcall fast path can run at all: `kwnames_tuple` itself
/// needs interning (R8) to build its tuple.
pub(crate) fn has_kw_vectorcall() -> bool {
    HAS_VECTORCALL.load(Ordering::Relaxed) && use_interned_names()
}

/// Converts `len` raw tagged words at `sv_ptr` into `buf[base..base+len]`.
/// Aborts the process on the first bad conversion -- matches every other
/// `_unsafe` tagged-call entry point, which never cleans up before
/// `handle_py_error` because that call never returns.
pub(crate) unsafe fn convert_segment_unsafe(
    sv_ptr: *mut i64,
    len: usize,
    coll_tags: i64,
    arg_tags: i64,
    buf: &mut [PyObject],
    base: usize,
    pairs: &mut Vec<WritebackPair>,
) {
    unsafe {
        for i in 0..len {
            let coll_tag = tag_at(coll_tags, i);
            let arg_tag = arg_tag_at(arg_tags, i);
            let val = *sv_ptr.add(i);
            let py_v = convert_arg_tagged(val, coll_tag, arg_tag, pairs);
            if coll_tag != TAG_NONE {
                *sv_ptr.add(i) = 0;
            }
            if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                handle_py_error();
            }
            buf[base + i] = py_v;
        }
    }
}

/// `Result`-returning counterpart: on failure, decrefs every slot this call
/// itself converted (`buf[base..]`, never touching a method call's `self`
/// at `buf[1]`, which sits below `base` and isn't ours), abandons `pairs`,
/// and reports failure so the caller can encode it instead of aborting.
pub(crate) unsafe fn convert_segment_safe(
    sv_ptr: *mut i64,
    len: usize,
    coll_tags: i64,
    arg_tags: i64,
    buf: &mut [PyObject],
    base: usize,
    pairs: &mut Vec<WritebackPair>,
) -> bool {
    unsafe {
        for i in 0..len {
            let coll_tag = tag_at(coll_tags, i);
            let arg_tag = arg_tag_at(arg_tags, i);
            let val = *sv_ptr.add(i);
            let py_v = convert_arg_tagged(val, coll_tag, arg_tag, pairs);
            if coll_tag != TAG_NONE {
                *sv_ptr.add(i) = 0;
            }
            if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                if !py_v.is_null() {
                    PY_DEC_REF(py_v);
                }
                for s in &buf[base..base + i] {
                    if !s.is_null() {
                        PY_DEC_REF(*s);
                    }
                }
                abandon_pairs(pairs);
                return false;
            }
            buf[base + i] = py_v;
        }
        true
    }
}

pub(crate) unsafe fn stable_vec(list: i64) -> (*mut i64, usize) {
    if list == 0 {
        (std::ptr::null_mut(), 0)
    } else {
        let sv = unsafe { &*(list as *const crate::StableVec) };
        (sv.ptr, sv.len)
    }
}

/// `func(*positional, **keyword)`. `args_list`/`kwvals_list` are the
/// tagged-fast-path `StableVec` aggregates (positional values, keyword
/// values respectively -- keyword *names* live only in `kwnames_key`, a
/// packed constant `kwnames_tuple` resolves once and caches forever).
///
/// `loc` (R17): the call site's `file:line:col` constant, written to the
/// error-reporting thread-local as the first action -- see `olive_py_call0`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_kw_v(
    func: PyObject,
    args_list: i64,
    coll_tags: i64,
    arg_tags: i64,
    kwnames_key: i64,
    kwvals_list: i64,
    kw_coll_tags: i64,
    kw_arg_tags: i64,
    loc: i64,
) -> PyObject {
    set_py_call_loc(loc);
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        if !has_kw_vectorcall() {
            return legacy_call_kw(
                unwrapped_func,
                args_list,
                coll_tags,
                kwnames_key,
                kwvals_list,
                kw_coll_tags,
            );
        }
        with_gil(|| {
            let (pos_ptr, pos_len) = stable_vec(args_list);
            let (kw_ptr, kw_len) = stable_vec(kwvals_list);
            let kwnames = kwnames_tuple(crate::string_slab::str_body(kwnames_key) as *const c_char);
            if kwnames.is_null() {
                return legacy_call_kw(
                    unwrapped_func,
                    args_list,
                    coll_tags,
                    kwnames_key,
                    kwvals_list,
                    kw_coll_tags,
                );
            }
            call_kw_v_core(
                unwrapped_func,
                pos_ptr,
                pos_len,
                coll_tags,
                arg_tags,
                kwnames,
                kw_ptr,
                kw_len,
                kw_coll_tags,
                kw_arg_tags,
            )
        })
    }
}

/// `Result`-returning twin of `olive_py_call_kw_v`; see it for the shape.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_kw_v_safe(
    func: PyObject,
    args_list: i64,
    coll_tags: i64,
    arg_tags: i64,
    kwnames_key: i64,
    kwvals_list: i64,
    kw_coll_tags: i64,
    kw_arg_tags: i64,
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
        if !has_kw_vectorcall() {
            return legacy_call_kw_safe(
                unwrapped_func,
                args_list,
                coll_tags,
                kwnames_key,
                kwvals_list,
                kw_coll_tags,
            );
        }
        with_gil(|| {
            let (pos_ptr, pos_len) = stable_vec(args_list);
            let (kw_ptr, kw_len) = stable_vec(kwvals_list);
            let kwnames = kwnames_tuple(crate::string_slab::str_body(kwnames_key) as *const c_char);
            if kwnames.is_null() {
                return legacy_call_kw_safe(
                    unwrapped_func,
                    args_list,
                    coll_tags,
                    kwnames_key,
                    kwvals_list,
                    kw_coll_tags,
                );
            }
            call_kw_v_core_safe(
                unwrapped_func,
                pos_ptr,
                pos_len,
                coll_tags,
                arg_tags,
                kwnames,
                kw_ptr,
                kw_len,
                kw_coll_tags,
                kw_arg_tags,
            )
        })
    }
}

/// `obj.attr(*positional, **keyword)`. Same relationship to
/// `__olive_py_call_method{0..4}` as `olive_py_call_kw_v` has to
/// `__olive_py_call{0..4}`: `PyObject_VectorcallMethod` resolves the bound
/// method and calls it in one step, `self` passed straight through
/// (borrowed, never decref'd -- it stays owned by the caller's own local).
///
/// `loc` (R17): see `olive_py_call_kw_v`'s doc comment.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_method_kw_v(
    obj: PyObject,
    attr: i64,
    args_list: i64,
    coll_tags: i64,
    arg_tags: i64,
    kwnames_key: i64,
    kwvals_list: i64,
    kw_coll_tags: i64,
    kw_arg_tags: i64,
    loc: i64,
) -> PyObject {
    set_py_call_loc(loc);
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        if !has_kw_vectorcall() {
            return legacy_call_method_kw(
                unwrapped_obj,
                attr,
                args_list,
                coll_tags,
                kwnames_key,
                kwvals_list,
                kw_coll_tags,
            );
        }
        with_gil(|| {
            let name = interned_attr(crate::string_slab::str_body(attr) as *const c_char);
            if name.is_null() {
                return legacy_call_method_kw(
                    unwrapped_obj,
                    attr,
                    args_list,
                    coll_tags,
                    kwnames_key,
                    kwvals_list,
                    kw_coll_tags,
                );
            }
            let (pos_ptr, pos_len) = stable_vec(args_list);
            let (kw_ptr, kw_len) = stable_vec(kwvals_list);
            let kwnames = kwnames_tuple(crate::string_slab::str_body(kwnames_key) as *const c_char);
            if kwnames.is_null() {
                return legacy_call_method_kw(
                    unwrapped_obj,
                    attr,
                    args_list,
                    coll_tags,
                    kwnames_key,
                    kwvals_list,
                    kw_coll_tags,
                );
            }
            call_kw_v_method_core(
                unwrapped_obj,
                name,
                pos_ptr,
                pos_len,
                coll_tags,
                arg_tags,
                kwnames,
                kw_ptr,
                kw_len,
                kw_coll_tags,
                kw_arg_tags,
            )
        })
    }
}

/// `Result`-returning twin of `olive_py_call_method_kw_v`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_method_kw_v_safe(
    obj: PyObject,
    attr: i64,
    args_list: i64,
    coll_tags: i64,
    arg_tags: i64,
    kwnames_key: i64,
    kwvals_list: i64,
    kw_coll_tags: i64,
    kw_arg_tags: i64,
) -> i64 {
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
    unsafe {
        if !has_kw_vectorcall() {
            return legacy_call_method_kw_safe(
                unwrapped_obj,
                attr,
                args_list,
                coll_tags,
                kwnames_key,
                kwvals_list,
                kw_coll_tags,
            );
        }
        with_gil(|| {
            let name = interned_attr(crate::string_slab::str_body(attr) as *const c_char);
            if name.is_null() {
                return legacy_call_method_kw_safe(
                    unwrapped_obj,
                    attr,
                    args_list,
                    coll_tags,
                    kwnames_key,
                    kwvals_list,
                    kw_coll_tags,
                );
            }
            let (pos_ptr, pos_len) = stable_vec(args_list);
            let (kw_ptr, kw_len) = stable_vec(kwvals_list);
            let kwnames = kwnames_tuple(crate::string_slab::str_body(kwnames_key) as *const c_char);
            if kwnames.is_null() {
                return legacy_call_method_kw_safe(
                    unwrapped_obj,
                    attr,
                    args_list,
                    coll_tags,
                    kwnames_key,
                    kwvals_list,
                    kw_coll_tags,
                );
            }
            call_kw_v_method_core_safe(
                unwrapped_obj,
                name,
                pos_ptr,
                pos_len,
                coll_tags,
                arg_tags,
                kwnames,
                kw_ptr,
                kw_len,
                kw_coll_tags,
                kw_arg_tags,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::python_coerce::{pyobject_slab_test_lock, static_attr_name};
    use crate::python::python_writeback::ARG_INT;

    fn with_forced_vectorcall<R>(want: bool, f: impl FnOnce() -> R) -> R {
        let prev = HAS_VECTORCALL.load(Ordering::SeqCst);
        HAS_VECTORCALL.store(want, Ordering::SeqCst);
        let r = f();
        HAS_VECTORCALL.store(prev, Ordering::SeqCst);
        r
    }

    fn make_int_list(vals: &[i64]) -> i64 {
        let list_ptr = crate::olive_list_new(vals.len() as i64);
        unsafe {
            let sv = &mut *(list_ptr as *mut crate::StableVec);
            for (i, &v) in vals.iter().enumerate() {
                *sv.ptr.add(i) = v;
            }
        }
        list_ptr
    }

    #[test]
    fn kwargs_only_call_reaches_the_right_values() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let func = with_gil(|| {
                let m = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr() as *const c_char);
                let d = PY_OBJECT_GET_ATTR_STRING(m, b"dict\0".as_ptr() as *const c_char);
                olive_py_wrap_owned(d)
            });
            let kwvals = make_int_list(&[1, 2]);
            let kwnames = static_attr_name("a,b");
            let kw_arg_tags = ARG_INT | (ARG_INT << 4);
            let res = olive_py_call_kw_v(func, 0, 0, 0, kwnames, kwvals, 0, kw_arg_tags, 0);
            assert!(!res.is_null());
            let a_val = with_gil(|| {
                let unwrapped = olive_py_unwrap(res);
                let key = PY_UNICODE_FROM_STRING(b"a\0".as_ptr() as *const c_char);
                let v = PY_OBJECT_GET_ITEM(unwrapped, key);
                let r = PY_LONG_AS_LONG(v);
                PY_DEC_REF(key);
                PY_DEC_REF(v);
                r
            });
            assert_eq!(a_val, 1);
            olive_py_decref(func);
            olive_py_decref(res);
        }
    }

    #[test]
    fn mixed_positional_and_kwargs_call_works() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let func = with_gil(|| {
                let m = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr() as *const c_char);
                olive_py_wrap_owned(PY_OBJECT_GET_ATTR_STRING(
                    m,
                    b"round\0".as_ptr() as *const c_char,
                ))
            });
            let float_bits = 3.456_f64.to_bits() as i64;
            let pos_float = crate::olive_list_new(1);
            {
                let sv = &mut *(pos_float as *mut crate::StableVec);
                *sv.ptr.add(0) = float_bits;
            }
            const ARG_FLOAT_TAG: i64 = 2;
            let kwvals = make_int_list(&[1]);
            let kwnames = static_attr_name("ndigits");
            let res = olive_py_call_kw_v(
                func,
                pos_float,
                0,
                ARG_FLOAT_TAG,
                kwnames,
                kwvals,
                0,
                ARG_INT,
                0,
            );
            assert!(!res.is_null());
            let val = with_gil(|| PY_FLOAT_AS_DOUBLE(olive_py_unwrap(res)));
            assert!((val - 3.5).abs() < 1e-9);
            olive_py_decref(func);
            olive_py_decref(res);
        }
    }

    #[test]
    fn kwargs_on_method_call_works() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            // `str.format` genuinely accepts `**kwargs` (Argument Clinic
            // makes several str methods positional-only, `format` is not
            // one of them), so `"{x}".format(x=5)` is a realistic kwargs
            // method call rather than a contrived one.
            let obj = with_gil(|| {
                olive_py_wrap_owned(PY_UNICODE_FROM_STRING(b"{x}\0".as_ptr() as *const c_char))
            });
            let unwrapped_obj = olive_py_unwrap(obj);
            let kwvals = make_int_list(&[5]);
            let kwnames = static_attr_name("x");
            let attr = static_attr_name("format");
            let res = olive_py_call_method_kw_v(
                unwrapped_obj,
                attr,
                0,
                0,
                0,
                kwnames,
                kwvals,
                0,
                ARG_INT,
                0,
            );
            assert!(!res.is_null());
            let out = with_gil(|| {
                let s = PY_UNICODE_AS_UTF8(olive_py_unwrap(res));
                std::ffi::CStr::from_ptr(s).to_string_lossy().to_string()
            });
            assert_eq!(out, "5");
            olive_py_decref(obj);
            olive_py_decref(res);
        }
    }

    #[test]
    fn repeated_call_site_reuses_cached_kwnames_tuple() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let func = with_gil(|| {
                let m = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr() as *const c_char);
                olive_py_wrap_owned(PY_OBJECT_GET_ATTR_STRING(
                    m,
                    b"dict\0".as_ptr() as *const c_char,
                ))
            });
            let kwnames = static_attr_name("repeat_key");
            let first_tuple = with_gil(|| kwnames_tuple(crate::string_slab::str_body(kwnames) as *const c_char));
            for _ in 0..1000 {
                let kwvals = make_int_list(&[7]);
                let res = olive_py_call_kw_v(func, 0, 0, 0, kwnames, kwvals, 0, ARG_INT, 0);
                assert!(!res.is_null());
                olive_py_decref(res);
            }
            let same_tuple = with_gil(|| kwnames_tuple(crate::string_slab::str_body(kwnames) as *const c_char));
            assert_eq!(
                first_tuple, same_tuple,
                "1000 calls sharing one kwnames key must reuse the same cached tuple"
            );
            olive_py_decref(func);
        }
    }

    #[test]
    fn missing_vectorcall_falls_back_to_dict_path_with_identical_results() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_forced_vectorcall(false, || unsafe {
            let func = with_gil(|| {
                let m = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr() as *const c_char);
                olive_py_wrap_owned(PY_OBJECT_GET_ATTR_STRING(
                    m,
                    b"dict\0".as_ptr() as *const c_char,
                ))
            });
            let kwvals = make_int_list(&[42]);
            let kwnames = static_attr_name("fallback_key");
            let res = olive_py_call_kw_v(func, 0, 0, 0, kwnames, kwvals, 0, 0, 0);
            assert!(!res.is_null());
            let val = with_gil(|| {
                let unwrapped = olive_py_unwrap(res);
                let key = PY_UNICODE_FROM_STRING(b"fallback_key\0".as_ptr() as *const c_char);
                let v = PY_OBJECT_GET_ITEM(unwrapped, key);
                let r = PY_LONG_AS_LONG(v);
                PY_DEC_REF(key);
                PY_DEC_REF(v);
                r
            });
            assert_eq!(val, 42);
            olive_py_decref(func);
            olive_py_decref(res);
        });
    }

    #[test]
    fn conversion_error_inside_kwargs_cleans_up() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let func = with_gil(|| {
                let m = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr() as *const c_char);
                olive_py_wrap_owned(PY_OBJECT_GET_ATTR_STRING(
                    m,
                    b"dict\0".as_ptr() as *const c_char,
                ))
            });
            // A null PyObject-tagged arg (ARG_PYOBJECT=0, value 0):
            // `olive_py_unwrap` short-circuits on a null input, so this
            // reaches `py_v.is_null()` cleanly, no crash, and forces the
            // conversion-failure path in the `_safe` flavor.
            let kwvals = make_int_list(&[0]);
            let kwnames = static_attr_name("bad_key");
            let res = olive_py_call_kw_v_safe(func, 0, 0, 0, kwnames, kwvals, 0, 0);
            assert_eq!(
                crate::result::olive_result_is_err(res),
                1,
                "an unconvertible kwarg value must surface as an Err, not crash"
            );
            olive_py_decref(func);
        }
    }
}
