//! Dict-building fallback for the R15 keyword-argument fast path. Reached
//! only when vectorcall or interning is unavailable, or a kwnames tuple
//! failed to build -- correctness over speed on this lane, taken by both
//! `python_call_kw_v.rs`'s list-based entry points and
//! `python_call_kw_arity.rs`'s arity-specialized shells (which build a
//! throwaway list of their own first, since they never have one to begin
//! with).

use crate::python::python_call_kw_v::stable_vec;
use crate::python::*;
use std::os::raw::c_char;

/// Splits `kwnames_key`'s packed, comma-joined names and zips them back up
/// with `kwvals_list`'s values into the interleaved `[name, value, ...]`
/// shape `call_kw_dict` expects, then defers to it entirely.
pub(crate) unsafe fn legacy_call_kw(
    unwrapped_func: PyObject,
    args_list: i64,
    coll_tags: i64,
    kwnames_key: i64,
    kwvals_list: i64,
    kw_coll_tags: i64,
) -> PyObject {
    unsafe {
        let interleaved = build_interleaved_kwargs(kwnames_key, kwvals_list);
        let res = crate::python::python_call::call_kw_dict(
            unwrapped_func,
            args_list,
            coll_tags,
            interleaved,
            kw_coll_tags,
        );
        if interleaved != 0 {
            crate::olive_free_list(interleaved);
        }
        res
    }
}

pub(crate) unsafe fn legacy_call_kw_safe(
    unwrapped_func: PyObject,
    args_list: i64,
    coll_tags: i64,
    kwnames_key: i64,
    kwvals_list: i64,
    kw_coll_tags: i64,
) -> i64 {
    unsafe {
        let interleaved = build_interleaved_kwargs(kwnames_key, kwvals_list);
        let res = crate::python::python_safe::call_kw_dict_safe(
            unwrapped_func,
            args_list,
            coll_tags,
            interleaved,
            kw_coll_tags,
        );
        if interleaved != 0 {
            crate::olive_free_list(interleaved);
        }
        res
    }
}

pub(crate) unsafe fn legacy_call_method_kw(
    obj: PyObject,
    attr: i64,
    args_list: i64,
    coll_tags: i64,
    kwnames_key: i64,
    kwvals_list: i64,
    kw_coll_tags: i64,
) -> PyObject {
    unsafe {
        let bound = with_gil(|| {
            if use_interned_names() {
                let name = interned_attr(crate::string_slab::str_body(attr) as *const c_char);
                if name.is_null() {
                    std::ptr::null_mut()
                } else {
                    PY_OBJECT_GET_ATTR(obj, name)
                }
            } else {
                PY_OBJECT_GET_ATTR_STRING(obj, crate::string_slab::str_body(attr) as *const c_char)
            }
        });
        if bound.is_null() {
            with_gil(|| handle_py_error());
        }
        let res = legacy_call_kw(
            bound,
            args_list,
            coll_tags,
            kwnames_key,
            kwvals_list,
            kw_coll_tags,
        );
        with_gil(|| PY_DEC_REF(bound));
        res
    }
}

pub(crate) unsafe fn legacy_call_method_kw_safe(
    obj: PyObject,
    attr: i64,
    args_list: i64,
    coll_tags: i64,
    kwnames_key: i64,
    kwvals_list: i64,
    kw_coll_tags: i64,
) -> i64 {
    unsafe {
        let bound = with_gil(|| {
            if use_interned_names() {
                let name = interned_attr(crate::string_slab::str_body(attr) as *const c_char);
                if name.is_null() {
                    std::ptr::null_mut()
                } else {
                    PY_OBJECT_GET_ATTR(obj, name)
                }
            } else {
                PY_OBJECT_GET_ATTR_STRING(obj, crate::string_slab::str_body(attr) as *const c_char)
            }
        });
        if bound.is_null() {
            let err_str_ptr = with_gil(|| {
                catch_py_exception_msg().unwrap_or_else(|| "attribute lookup failed".to_string())
            });
            return crate::result::olive_result_err(crate::olive_str_internal(&err_str_ptr));
        }
        let res = legacy_call_kw_safe(
            bound,
            args_list,
            coll_tags,
            kwnames_key,
            kwvals_list,
            kw_coll_tags,
        );
        with_gil(|| PY_DEC_REF(bound));
        res
    }
}

/// Rebuilds the interleaved `[name, value, name, value, ...]` `StableVec`
/// the pre-R15 dict-building path expects, from the packed name string and
/// the values-only list this phase's fast path uses instead. Only reached
/// on the fallback lane, so paying one extra list allocation here doesn't
/// touch the fast path's cost at all.
unsafe fn build_interleaved_kwargs(kwnames_key: i64, kwvals_list: i64) -> i64 {
    unsafe {
        let (kw_ptr, kw_len) = stable_vec(kwvals_list);
        if kw_len == 0 {
            return 0;
        }
        let packed =
            std::ffi::CStr::from_ptr(crate::string_slab::str_body(kwnames_key) as *const c_char).to_string_lossy();
        let names: Vec<&str> = packed.split(',').collect();
        let list_ptr = crate::olive_list_new((kw_len * 2) as i64);
        let sv = &mut *(list_ptr as *mut crate::StableVec);
        for i in 0..kw_len {
            let name_ptr = crate::olive_str_internal(names.get(i).copied().unwrap_or(""));
            *sv.ptr.add(i * 2) = name_ptr;
            *sv.ptr.add(i * 2 + 1) = *kw_ptr.add(i);
        }
        list_ptr
    }
}
