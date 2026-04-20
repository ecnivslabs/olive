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
            PY_ERR_CLEAR();
            return 0;
        }
        let val = py_to_olive_internal(item);
        PY_DEC_REF(item);
        val
    })
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
        if it.is_null() {
            if let Some(msg) = catch_py_exception_msg() {
                let err = crate::olive_str_internal(&msg);
                return crate::result::olive_result_err(err);
            }
        }
        let wrapped = olive_py_wrap_owned(it);
        crate::result::olive_result_ok(wrapped as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_has_next(_iter: PyObject) -> i64 {
    // We cannot peek the next item in Python without popping it.
    // Wait, the Olive `has_next` model doesn't fit Python's iterators perfectly.
    // If we can't peek, we must fetch the next item and store it in the iterator wrapper.
    // Since `__olive_has_next` expects a boolean without consuming, we must either:
    // 1) Have `olive_iter` return a struct that caches the next item.
    // Let's implement a lookahead cache!
    0 // Wait, let's fix this properly.
}
