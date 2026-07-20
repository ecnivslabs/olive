//! Arity-specialized kwargs fast path: the keyword-argument analogue of
//! `python_call.rs`'s `olive_py_call0..4` (positional-only). The compiler
//! emits one of these for a call site with `positional + keyword <= 4`
//! args (the common case for a real kwargs call), passing every value
//! straight in a register instead of building the `args_list`/`kwvals_list`
//! `List(Any)` aggregates `olive_py_call_kw_v`/`olive_py_call_method_kw_v`
//! need -- no Olive list allocation per call at all on the hot path.
//!
//! The rare fallback lane (`!has_kw_vectorcall()`, or a kwnames tuple that
//! fails to build) has no such list to hand `legacy_call_kw` -- it never
//! had one -- so it builds a throwaway one on the spot and frees it right
//! after, same cost the list-based entry points always paid on this lane.
//! `regs_to_list`'s elements are the same raw tagged words the compiler's
//! own `List(Any)` aggregate holds, and `olive_free_list`'s per-element
//! walk is documented sound for arbitrary words (`is_active_object` tests
//! slab membership, not a guessed tag bit), so freeing this hand-built
//! list the normal way is exactly as safe as freeing the MIR-emitted one.

use crate::python::python_call_kw_core::{
    call_kw_v_core, call_kw_v_core_safe, call_kw_v_method_core, call_kw_v_method_core_safe,
};
use crate::python::python_call_kw_legacy::{
    legacy_call_kw, legacy_call_kw_safe, legacy_call_method_kw, legacy_call_method_kw_safe,
};
use crate::python::python_call_kw_v::has_kw_vectorcall;
use crate::python::python_kwnames::kwnames_tuple;
use crate::python::*;
use std::os::raw::c_char;

unsafe fn regs_to_list(vals: &[i64]) -> i64 {
    if vals.is_empty() {
        return 0;
    }
    unsafe {
        let list_ptr = crate::olive_list_new(vals.len() as i64);
        let sv = &mut *(list_ptr as *mut crate::StableVec);
        for (i, &v) in vals.iter().enumerate() {
            *sv.ptr.add(i) = v;
        }
        list_ptr
    }
}

unsafe fn free_regs_list(list: i64) {
    if list != 0 {
        crate::olive_free_list(list);
    }
}

/// Generates the `unsafe`/`_safe` extern-C shell pair for one `(P, K)`
/// positional/keyword arity, for a plain `func(*pos, **kw)` call.
macro_rules! define_kw_arity_call {
    ($name:ident, $safe_name:ident, $p:literal, $k:literal, [$($pn:ident),*], [$($kn:ident),*]) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(
            func: PyObject,
            coll_tags: i64,
            arg_tags: i64,
            kwnames_key: i64,
            kw_coll_tags: i64,
            kw_arg_tags: i64,
            $($pn: i64,)*
            $($kn: i64,)*
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
                    let args_list = regs_to_list(&[$($pn),*]);
                    let kwvals_list = regs_to_list(&[$($kn),*]);
                    let res = legacy_call_kw(
                        unwrapped_func, args_list, coll_tags, kwnames_key, kwvals_list,
                        kw_coll_tags,
                    );
                    free_regs_list(args_list);
                    free_regs_list(kwvals_list);
                    return res;
                }
                with_gil(|| {
                    let kwnames = kwnames_tuple(crate::string_slab::str_body(kwnames_key) as *const c_char);
                    if kwnames.is_null() {
                        let args_list = regs_to_list(&[$($pn),*]);
                        let kwvals_list = regs_to_list(&[$($kn),*]);
                        let res = legacy_call_kw(
                            unwrapped_func, args_list, coll_tags, kwnames_key, kwvals_list,
                            kw_coll_tags,
                        );
                        free_regs_list(args_list);
                        free_regs_list(kwvals_list);
                        return res;
                    }
                    let mut pos: [i64; $p] = [$($pn),*];
                    let mut kw: [i64; $k] = [$($kn),*];
                    call_kw_v_core(
                        unwrapped_func, pos.as_mut_ptr(), $p, coll_tags, arg_tags, kwnames,
                        kw.as_mut_ptr(), $k, kw_coll_tags, kw_arg_tags,
                    )
                })
            }
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn $safe_name(
            func: PyObject,
            coll_tags: i64,
            arg_tags: i64,
            kwnames_key: i64,
            kw_coll_tags: i64,
            kw_arg_tags: i64,
            $($pn: i64,)*
            $($kn: i64,)*
        ) -> i64 {
            if !is_python_available() {
                let err_str_ptr = crate::olive_str_internal(
                    "Python interop unavailable: libpython3 could not be loaded",
                );
                return crate::result::olive_result_err(err_str_ptr);
            }
            let unwrapped_func = unsafe { olive_py_unwrap(func) };
            if unwrapped_func.is_null() {
                let err_str_ptr = crate::olive_str_internal("Null function pointer");
                return crate::result::olive_result_err(err_str_ptr);
            }
            unsafe {
                if !has_kw_vectorcall() {
                    let args_list = regs_to_list(&[$($pn),*]);
                    let kwvals_list = regs_to_list(&[$($kn),*]);
                    let res = legacy_call_kw_safe(
                        unwrapped_func, args_list, coll_tags, kwnames_key, kwvals_list,
                        kw_coll_tags,
                    );
                    free_regs_list(args_list);
                    free_regs_list(kwvals_list);
                    return res;
                }
                with_gil(|| {
                    let kwnames = kwnames_tuple(crate::string_slab::str_body(kwnames_key) as *const c_char);
                    if kwnames.is_null() {
                        let args_list = regs_to_list(&[$($pn),*]);
                        let kwvals_list = regs_to_list(&[$($kn),*]);
                        let res = legacy_call_kw_safe(
                            unwrapped_func, args_list, coll_tags, kwnames_key, kwvals_list,
                            kw_coll_tags,
                        );
                        free_regs_list(args_list);
                        free_regs_list(kwvals_list);
                        return res;
                    }
                    let mut pos: [i64; $p] = [$($pn),*];
                    let mut kw: [i64; $k] = [$($kn),*];
                    call_kw_v_core_safe(
                        unwrapped_func, pos.as_mut_ptr(), $p, coll_tags, arg_tags, kwnames,
                        kw.as_mut_ptr(), $k, kw_coll_tags, kw_arg_tags,
                    )
                })
            }
        }
    };
}

/// Generates the `unsafe`/`_safe` extern-C shell pair for one `(P, K)`
/// arity, for a bound `obj.attr(*pos, **kw)` call.
macro_rules! define_kw_arity_method_call {
    ($name:ident, $safe_name:ident, $p:literal, $k:literal, [$($pn:ident),*], [$($kn:ident),*]) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(
            obj: PyObject,
            attr: i64,
            coll_tags: i64,
            arg_tags: i64,
            kwnames_key: i64,
            kw_coll_tags: i64,
            kw_arg_tags: i64,
            $($pn: i64,)*
            $($kn: i64,)*
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
                    let args_list = regs_to_list(&[$($pn),*]);
                    let kwvals_list = regs_to_list(&[$($kn),*]);
                    let res = legacy_call_method_kw(
                        unwrapped_obj, attr, args_list, coll_tags, kwnames_key, kwvals_list,
                        kw_coll_tags,
                    );
                    free_regs_list(args_list);
                    free_regs_list(kwvals_list);
                    return res;
                }
                with_gil(|| {
                    let name = interned_attr(crate::string_slab::str_body(attr) as *const c_char);
                    if name.is_null() {
                        let args_list = regs_to_list(&[$($pn),*]);
                        let kwvals_list = regs_to_list(&[$($kn),*]);
                        let res = legacy_call_method_kw(
                            unwrapped_obj, attr, args_list, coll_tags, kwnames_key, kwvals_list,
                            kw_coll_tags,
                        );
                        free_regs_list(args_list);
                        free_regs_list(kwvals_list);
                        return res;
                    }
                    let kwnames = kwnames_tuple(crate::string_slab::str_body(kwnames_key) as *const c_char);
                    if kwnames.is_null() {
                        let args_list = regs_to_list(&[$($pn),*]);
                        let kwvals_list = regs_to_list(&[$($kn),*]);
                        let res = legacy_call_method_kw(
                            unwrapped_obj, attr, args_list, coll_tags, kwnames_key, kwvals_list,
                            kw_coll_tags,
                        );
                        free_regs_list(args_list);
                        free_regs_list(kwvals_list);
                        return res;
                    }
                    let mut pos: [i64; $p] = [$($pn),*];
                    let mut kw: [i64; $k] = [$($kn),*];
                    call_kw_v_method_core(
                        unwrapped_obj, name, pos.as_mut_ptr(), $p, coll_tags, arg_tags, kwnames,
                        kw.as_mut_ptr(), $k, kw_coll_tags, kw_arg_tags,
                    )
                })
            }
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn $safe_name(
            obj: PyObject,
            attr: i64,
            coll_tags: i64,
            arg_tags: i64,
            kwnames_key: i64,
            kw_coll_tags: i64,
            kw_arg_tags: i64,
            $($pn: i64,)*
            $($kn: i64,)*
        ) -> i64 {
            if !is_python_available() {
                let err_str_ptr = crate::olive_str_internal(
                    "Python interop unavailable: libpython3 could not be loaded",
                );
                return crate::result::olive_result_err(err_str_ptr);
            }
            let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
            if unwrapped_obj.is_null() {
                let err_str_ptr = crate::olive_str_internal("Null object pointer");
                return crate::result::olive_result_err(err_str_ptr);
            }
            unsafe {
                if !has_kw_vectorcall() {
                    let args_list = regs_to_list(&[$($pn),*]);
                    let kwvals_list = regs_to_list(&[$($kn),*]);
                    let res = legacy_call_method_kw_safe(
                        unwrapped_obj, attr, args_list, coll_tags, kwnames_key, kwvals_list,
                        kw_coll_tags,
                    );
                    free_regs_list(args_list);
                    free_regs_list(kwvals_list);
                    return res;
                }
                with_gil(|| {
                    let name = interned_attr(crate::string_slab::str_body(attr) as *const c_char);
                    if name.is_null() {
                        let args_list = regs_to_list(&[$($pn),*]);
                        let kwvals_list = regs_to_list(&[$($kn),*]);
                        let res = legacy_call_method_kw_safe(
                            unwrapped_obj, attr, args_list, coll_tags, kwnames_key, kwvals_list,
                            kw_coll_tags,
                        );
                        free_regs_list(args_list);
                        free_regs_list(kwvals_list);
                        return res;
                    }
                    let kwnames = kwnames_tuple(crate::string_slab::str_body(kwnames_key) as *const c_char);
                    if kwnames.is_null() {
                        let args_list = regs_to_list(&[$($pn),*]);
                        let kwvals_list = regs_to_list(&[$($kn),*]);
                        let res = legacy_call_method_kw_safe(
                            unwrapped_obj, attr, args_list, coll_tags, kwnames_key, kwvals_list,
                            kw_coll_tags,
                        );
                        free_regs_list(args_list);
                        free_regs_list(kwvals_list);
                        return res;
                    }
                    let mut pos: [i64; $p] = [$($pn),*];
                    let mut kw: [i64; $k] = [$($kn),*];
                    call_kw_v_method_core_safe(
                        unwrapped_obj, name, pos.as_mut_ptr(), $p, coll_tags, arg_tags, kwnames,
                        kw.as_mut_ptr(), $k, kw_coll_tags, kw_arg_tags,
                    )
                })
            }
        }
    };
}

// Every (positional, keyword) shape with positional + keyword <= 4 and
// keyword >= 1 (keyword == 0 is the plain, no-kwargs arity path already
// covered by `python_call.rs`'s `olive_py_call0..4`). Arity 5+ falls back
// to the list-based `olive_py_call_kw_v`/`olive_py_call_method_kw_v`,
// unchanged, the same way positional arity 5+ already falls back to
// `olive_py_call_t`.
define_kw_arity_call!(
    olive_py_call_kw_v_p0_k1,
    olive_py_call_kw_v_p0_k1_safe,
    0,
    1,
    [],
    [k0]
);
define_kw_arity_call!(
    olive_py_call_kw_v_p0_k2,
    olive_py_call_kw_v_p0_k2_safe,
    0,
    2,
    [],
    [k0, k1]
);
define_kw_arity_call!(
    olive_py_call_kw_v_p0_k3,
    olive_py_call_kw_v_p0_k3_safe,
    0,
    3,
    [],
    [k0, k1, k2]
);
define_kw_arity_call!(
    olive_py_call_kw_v_p0_k4,
    olive_py_call_kw_v_p0_k4_safe,
    0,
    4,
    [],
    [k0, k1, k2, k3]
);
define_kw_arity_call!(
    olive_py_call_kw_v_p1_k1,
    olive_py_call_kw_v_p1_k1_safe,
    1,
    1,
    [p0],
    [k0]
);
define_kw_arity_call!(
    olive_py_call_kw_v_p1_k2,
    olive_py_call_kw_v_p1_k2_safe,
    1,
    2,
    [p0],
    [k0, k1]
);
define_kw_arity_call!(
    olive_py_call_kw_v_p1_k3,
    olive_py_call_kw_v_p1_k3_safe,
    1,
    3,
    [p0],
    [k0, k1, k2]
);
define_kw_arity_call!(
    olive_py_call_kw_v_p2_k1,
    olive_py_call_kw_v_p2_k1_safe,
    2,
    1,
    [p0, p1],
    [k0]
);
define_kw_arity_call!(
    olive_py_call_kw_v_p2_k2,
    olive_py_call_kw_v_p2_k2_safe,
    2,
    2,
    [p0, p1],
    [k0, k1]
);
define_kw_arity_call!(
    olive_py_call_kw_v_p3_k1,
    olive_py_call_kw_v_p3_k1_safe,
    3,
    1,
    [p0, p1, p2],
    [k0]
);

define_kw_arity_method_call!(
    olive_py_call_method_kw_v_p0_k1,
    olive_py_call_method_kw_v_p0_k1_safe,
    0,
    1,
    [],
    [k0]
);
define_kw_arity_method_call!(
    olive_py_call_method_kw_v_p0_k2,
    olive_py_call_method_kw_v_p0_k2_safe,
    0,
    2,
    [],
    [k0, k1]
);
define_kw_arity_method_call!(
    olive_py_call_method_kw_v_p0_k3,
    olive_py_call_method_kw_v_p0_k3_safe,
    0,
    3,
    [],
    [k0, k1, k2]
);
define_kw_arity_method_call!(
    olive_py_call_method_kw_v_p0_k4,
    olive_py_call_method_kw_v_p0_k4_safe,
    0,
    4,
    [],
    [k0, k1, k2, k3]
);
define_kw_arity_method_call!(
    olive_py_call_method_kw_v_p1_k1,
    olive_py_call_method_kw_v_p1_k1_safe,
    1,
    1,
    [p0],
    [k0]
);
define_kw_arity_method_call!(
    olive_py_call_method_kw_v_p1_k2,
    olive_py_call_method_kw_v_p1_k2_safe,
    1,
    2,
    [p0],
    [k0, k1]
);
define_kw_arity_method_call!(
    olive_py_call_method_kw_v_p1_k3,
    olive_py_call_method_kw_v_p1_k3_safe,
    1,
    3,
    [p0],
    [k0, k1, k2]
);
define_kw_arity_method_call!(
    olive_py_call_method_kw_v_p2_k1,
    olive_py_call_method_kw_v_p2_k1_safe,
    2,
    1,
    [p0, p1],
    [k0]
);
define_kw_arity_method_call!(
    olive_py_call_method_kw_v_p2_k2,
    olive_py_call_method_kw_v_p2_k2_safe,
    2,
    2,
    [p0, p1],
    [k0, k1]
);
define_kw_arity_method_call!(
    olive_py_call_method_kw_v_p3_k1,
    olive_py_call_method_kw_v_p3_k1_safe,
    3,
    1,
    [p0, p1, p2],
    [k0]
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::python_coerce::{pyobject_slab_test_lock, static_attr_name};
    use crate::python::python_writeback::ARG_INT;

    #[test]
    fn plain_call_arity_p2_k2_reaches_the_right_values() {
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
            let kwnames = static_attr_name("a,b");
            let kw_tags = ARG_INT | (ARG_INT << 4);
            let res = olive_py_call_kw_v_p0_k2(func, 0, 0, kwnames, 0, kw_tags, 1, 2, 0);
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
    fn method_call_arity_p1_k1_mixes_positional_and_keyword() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let obj = with_gil(|| {
                olive_py_wrap_owned(PY_UNICODE_FROM_STRING(
                    b"{0}-{y}\0".as_ptr() as *const c_char
                ))
            });
            let unwrapped_obj = olive_py_unwrap(obj);
            let attr = static_attr_name("format");
            let kwnames = static_attr_name("y");
            let res = olive_py_call_method_kw_v_p1_k1(
                unwrapped_obj,
                attr,
                0,
                ARG_INT,
                kwnames,
                0,
                ARG_INT,
                5,
                9,
                0,
            );
            assert!(!res.is_null());
            let out = with_gil(|| {
                let s = PY_UNICODE_AS_UTF8(olive_py_unwrap(res));
                std::ffi::CStr::from_ptr(s).to_string_lossy().to_string()
            });
            assert_eq!(out, "5-9");
            olive_py_decref(obj);
            olive_py_decref(res);
        }
    }

    #[test]
    fn conversion_error_inside_arity_kwargs_cleans_up() {
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
            let kwnames = static_attr_name("bad_key");
            // ARG_PYOBJECT tag (0) with a null handle value: `olive_py_unwrap`
            // short-circuits, forcing the conversion-failure path.
            let res = olive_py_call_kw_v_p0_k1_safe(func, 0, 0, kwnames, 0, 0, 0);
            assert_eq!(
                crate::result::olive_result_is_err(res),
                1,
                "an unconvertible kwarg value must surface as an Err, not crash"
            );
            olive_py_decref(func);
        }
    }

    #[test]
    fn missing_vectorcall_falls_back_without_leaking_the_throwaway_list() {
        use std::sync::atomic::Ordering;
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        let prev = HAS_VECTORCALL.load(Ordering::SeqCst);
        HAS_VECTORCALL.store(false, Ordering::SeqCst);
        unsafe {
            let func = with_gil(|| {
                let m = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr() as *const c_char);
                olive_py_wrap_owned(PY_OBJECT_GET_ATTR_STRING(
                    m,
                    b"dict\0".as_ptr() as *const c_char,
                ))
            });
            let kwnames = static_attr_name("fallback_arity_key");
            let res = olive_py_call_kw_v_p0_k1(func, 0, 0, kwnames, 0, 0, 77, 0);
            assert!(!res.is_null());
            let val = with_gil(|| {
                let unwrapped = olive_py_unwrap(res);
                let key = PY_UNICODE_FROM_STRING(b"fallback_arity_key\0".as_ptr() as *const c_char);
                let v = PY_OBJECT_GET_ITEM(unwrapped, key);
                let r = PY_LONG_AS_LONG(v);
                PY_DEC_REF(key);
                PY_DEC_REF(v);
                r
            });
            assert_eq!(val, 77);
            olive_py_decref(func);
            olive_py_decref(res);
        }
        HAS_VECTORCALL.store(prev, Ordering::SeqCst);
    }
}
