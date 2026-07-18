//! Shared conversion+vectorcall bodies for the R15 keyword-argument fast
//! path. `python_call_kw_v.rs`'s list-based entry points and
//! `python_call_kw_arity.rs`'s arity-specialized, allocation-free shells
//! both bottom out here -- the only difference between the two callers is
//! where `pos`/`kw`'s backing words live (a `StableVec` vs a disposable
//! stack array) and, for the method variants, that `name`/`kwnames` are
//! already resolved before this file ever runs. Caller holds the GIL.

use crate::python::python_call_kw_v::{convert_segment_safe, convert_segment_unsafe};
use crate::python::*;

/// `func(*pos, **kw)` via `PyObject_Vectorcall`. `kwnames` must already be
/// resolved (non-null) -- callers own the fallback-to-dict decision, since
/// only the list-based caller can retry through `legacy_call_kw` (the
/// arity shells build a throwaway list of their own for that rare lane).
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn call_kw_v_core(
    unwrapped_func: PyObject,
    pos_ptr: *mut i64,
    pos_len: usize,
    coll_tags: i64,
    arg_tags: i64,
    kwnames: PyObject,
    kw_ptr: *mut i64,
    kw_len: usize,
    kw_coll_tags: i64,
    kw_arg_tags: i64,
) -> PyObject {
    unsafe {
        let mut pairs = Vec::new();
        let mut buf: [PyObject; 34] = [std::ptr::null_mut(); 34];
        convert_segment_unsafe(
            pos_ptr, pos_len, coll_tags, arg_tags, &mut buf, 1, &mut pairs,
        );
        convert_segment_unsafe(
            kw_ptr,
            kw_len,
            kw_coll_tags,
            kw_arg_tags,
            &mut buf,
            1 + pos_len,
            &mut pairs,
        );
        let nargsf = pos_len | PY_VECTORCALL_ARGUMENTS_OFFSET;
        let res = PY_VECTORCALL(unwrapped_func, buf.as_ptr().add(1), nargsf, kwnames);
        for slot in &buf[1..1 + pos_len + kw_len] {
            if !slot.is_null() {
                PY_DEC_REF(*slot);
            }
        }
        sync_back(&pairs);
        if res.is_null() {
            handle_py_error();
        } else if !PY_ERR_OCCURRED().is_null() {
            PY_ERR_CLEAR();
        }
        olive_py_wrap_owned(res)
    }
}

/// `Result`-returning twin of `call_kw_v_core`.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn call_kw_v_core_safe(
    unwrapped_func: PyObject,
    pos_ptr: *mut i64,
    pos_len: usize,
    coll_tags: i64,
    arg_tags: i64,
    kwnames: PyObject,
    kw_ptr: *mut i64,
    kw_len: usize,
    kw_coll_tags: i64,
    kw_arg_tags: i64,
) -> i64 {
    unsafe {
        let mut pairs = Vec::new();
        let mut buf: [PyObject; 34] = [std::ptr::null_mut(); 34];
        if !convert_segment_safe(
            pos_ptr, pos_len, coll_tags, arg_tags, &mut buf, 1, &mut pairs,
        ) {
            return conversion_err();
        }
        if !convert_segment_safe(
            kw_ptr,
            kw_len,
            kw_coll_tags,
            kw_arg_tags,
            &mut buf,
            1 + pos_len,
            &mut pairs,
        ) {
            return conversion_err();
        }
        let nargsf = pos_len | PY_VECTORCALL_ARGUMENTS_OFFSET;
        let res = PY_VECTORCALL(unwrapped_func, buf.as_ptr().add(1), nargsf, kwnames);
        for slot in &buf[1..1 + pos_len + kw_len] {
            if !slot.is_null() {
                PY_DEC_REF(*slot);
            }
        }
        sync_back(&pairs);
        finish_call_safe(Ok(res), RET_HANDLE)
    }
}

/// `obj.attr(*pos, **kw)` via `PyObject_VectorcallMethod`. `name` is the
/// already-resolved (interned) attribute, `kwnames` the already-resolved
/// tuple -- same division of responsibility as `call_kw_v_core`.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn call_kw_v_method_core(
    unwrapped_obj: PyObject,
    name: PyObject,
    pos_ptr: *mut i64,
    pos_len: usize,
    coll_tags: i64,
    arg_tags: i64,
    kwnames: PyObject,
    kw_ptr: *mut i64,
    kw_len: usize,
    kw_coll_tags: i64,
    kw_arg_tags: i64,
) -> PyObject {
    unsafe {
        let mut pairs = Vec::new();
        let mut buf: [PyObject; 34] = [std::ptr::null_mut(); 34];
        buf[1] = unwrapped_obj;
        convert_segment_unsafe(
            pos_ptr, pos_len, coll_tags, arg_tags, &mut buf, 2, &mut pairs,
        );
        convert_segment_unsafe(
            kw_ptr,
            kw_len,
            kw_coll_tags,
            kw_arg_tags,
            &mut buf,
            2 + pos_len,
            &mut pairs,
        );
        let nargsf = (pos_len + 1) | PY_VECTORCALL_ARGUMENTS_OFFSET;
        let res = PY_VECTORCALL_METHOD(name, buf.as_ptr().add(1), nargsf, kwnames);
        for slot in &buf[2..2 + pos_len + kw_len] {
            if !slot.is_null() {
                PY_DEC_REF(*slot);
            }
        }
        sync_back(&pairs);
        if res.is_null() {
            handle_py_error();
        } else if !PY_ERR_OCCURRED().is_null() {
            PY_ERR_CLEAR();
        }
        olive_py_wrap_owned(res)
    }
}

/// `Result`-returning twin of `call_kw_v_method_core`.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn call_kw_v_method_core_safe(
    unwrapped_obj: PyObject,
    name: PyObject,
    pos_ptr: *mut i64,
    pos_len: usize,
    coll_tags: i64,
    arg_tags: i64,
    kwnames: PyObject,
    kw_ptr: *mut i64,
    kw_len: usize,
    kw_coll_tags: i64,
    kw_arg_tags: i64,
) -> i64 {
    unsafe {
        let mut pairs = Vec::new();
        let mut buf: [PyObject; 34] = [std::ptr::null_mut(); 34];
        buf[1] = unwrapped_obj;
        if !convert_segment_safe(
            pos_ptr, pos_len, coll_tags, arg_tags, &mut buf, 2, &mut pairs,
        ) {
            return conversion_err();
        }
        if !convert_segment_safe(
            kw_ptr,
            kw_len,
            kw_coll_tags,
            kw_arg_tags,
            &mut buf,
            2 + pos_len,
            &mut pairs,
        ) {
            return conversion_err();
        }
        let nargsf = (pos_len + 1) | PY_VECTORCALL_ARGUMENTS_OFFSET;
        let res = PY_VECTORCALL_METHOD(name, buf.as_ptr().add(1), nargsf, kwnames);
        for slot in &buf[2..2 + pos_len + kw_len] {
            if !slot.is_null() {
                PY_DEC_REF(*slot);
            }
        }
        sync_back(&pairs);
        finish_call_safe(Ok(res), RET_HANDLE)
    }
}
