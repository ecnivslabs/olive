use crate::python::*;

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_iter(obj: PyObject) -> PyObject {
    check_python_loaded();
    let unwrapped = unsafe { olive_py_unwrap(obj) };
    if unwrapped.is_null() {
        return std::ptr::null_mut();
    }
    with_gil(|| unsafe {
        let it = PY_OBJECT_GET_ITER(unwrapped);
        if it.is_null() {
            handle_py_error();
        }
        // `handle_py_error` never returns, so `it` is non-null here.
        olive_py_wrap_owned(it)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_iter_next(iter: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped = unsafe { olive_py_unwrap(iter) };
    if unwrapped.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        let item = PY_ITER_NEXT(unwrapped);
        if item.is_null() {
            if PY_ERR_OCCURRED().is_null() {
                return 0;
            }
            handle_py_error();
        }
        let val = py_to_olive_internal(item);
        PY_DEC_REF(item);
        if crate::python::python_coerce::conversion_failed() {
            let msg = crate::python::python_coerce::take_conversion_error()
                .unwrap_or_else(|| {
                    "Python conversion graph is cyclic or too deeply nested".to_string()
                });
            crate::panic::abort_py_coerce(&msg);
        }
        val
    })
}

/// Raw next for `has_next` peeking: returns an owned reference, or null on
/// exhaustion. A Python `None` element is a non-null `Py_None` pointer, so
/// null unambiguously means exhausted, unlike the converted `0` word which
/// collides with `None`. Real errors abort with the Python traceback.
pub(crate) unsafe fn olive_py_iter_next_raw(iter_unwrapped: PyObject) -> PyObject {
    unsafe {
        let item = PY_ITER_NEXT(iter_unwrapped);
        if item.is_null() {
            if PY_ERR_OCCURRED().is_null() {
                return std::ptr::null_mut();
            }
            crate::python::python_error::handle_py_error();
        }
        item
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_iter_safe(obj: PyObject) -> i64 {
    if !is_python_available() {
        let err =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err);
    }
    let unwrapped = unsafe { olive_py_unwrap(obj) };
    if unwrapped.is_null() {
        let err = crate::olive_str_internal("Null object pointer");
        return crate::result::olive_result_err(err);
    }
    with_gil(|| unsafe {
        let it = PY_OBJECT_GET_ITER(unwrapped);
        if it.is_null()
            && let Some(msg) = catch_py_exception_msg()
        {
            let err = crate::olive_str_internal(&msg);
            return crate::result::olive_result_err(err);
        }
        // The iterator's own reference moves into the handle; nothing else
        // owns it, so a null iterator must stay null rather than be wrapped.
        let wrapped = olive_py_wrap_owned(it);
        crate::result::olive_result_ok(wrapped as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_has_next(_iter: PyObject) -> i64 {
    0
}

/// Whether unwrapped iterator `it` is exhausted, without converting the
/// peeked element: consumes one raw item and hands ownership to the caller
/// via `out_raw` (null means exhausted). Lets `olive_has_next` separate a
/// `None` element (non-null raw) from exhaustion (null raw).
pub(crate) unsafe fn olive_py_iter_peek_raw(it: PyObject, out_raw: &mut PyObject) -> bool {
    unsafe {
        let item = olive_py_iter_next_raw(it);
        if item.is_null() {
            *out_raw = std::ptr::null_mut();
            false
        } else {
            *out_raw = item;
            true
        }
    }
}
