use crate::python::*;
use std::os::raw::c_char;

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_is_coroutine(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped = unsafe { olive_py_unwrap(obj) };
    if unwrapped.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        if PY_CORO_CHECK_EXACT(unwrapped) != 0 {
            1
        } else {
            0
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_run_coroutine(coro: PyObject) -> PyObject {
    check_python_loaded();
    let unwrapped = unsafe { olive_py_unwrap(coro) };
    if unwrapped.is_null() {
        return std::ptr::null_mut();
    }
    with_gil(|| unsafe {
        let asyncio = PY_IMPORT_IMPORT_MODULE(b"asyncio\0".as_ptr() as *const c_char);
        if asyncio.is_null() {
            handle_py_error();
        }

        let run_fn = PY_OBJECT_GET_ATTR_STRING(asyncio, b"run\0".as_ptr() as *const c_char);
        let result = if !run_fn.is_null() {
            let args = PY_TUPLE_NEW(1);
            PY_INC_REF(unwrapped);
            PY_TUPLE_SET_ITEM(args, 0, unwrapped);
            let res = PY_OBJECT_CALL_OBJECT(run_fn, args);
            PY_DEC_REF(args);
            PY_DEC_REF(run_fn);
            res
        } else {
            PY_ERR_CLEAR();
            let get_loop_fn =
                PY_OBJECT_GET_ATTR_STRING(asyncio, b"get_event_loop\0".as_ptr() as *const c_char);
            if get_loop_fn.is_null() {
                PY_DEC_REF(asyncio);
                handle_py_error();
            }
            let loop_obj = PY_OBJECT_CALL_OBJECT(get_loop_fn, std::ptr::null_mut());
            PY_DEC_REF(get_loop_fn);
            if loop_obj.is_null() {
                PY_DEC_REF(asyncio);
                handle_py_error();
            }
            let run_until = PY_OBJECT_GET_ATTR_STRING(
                loop_obj,
                b"run_until_complete\0".as_ptr() as *const c_char,
            );
            if run_until.is_null() {
                PY_DEC_REF(loop_obj);
                PY_DEC_REF(asyncio);
                handle_py_error();
            }
            let args = PY_TUPLE_NEW(1);
            PY_INC_REF(unwrapped);
            PY_TUPLE_SET_ITEM(args, 0, unwrapped);
            let res = PY_OBJECT_CALL_OBJECT(run_until, args);
            PY_DEC_REF(args);
            PY_DEC_REF(run_until);
            PY_DEC_REF(loop_obj);
            res
        };

        PY_DEC_REF(asyncio);

        if result.is_null() {
            handle_py_error();
        }

        olive_py_wrap_owned(result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_run_coroutine_safe(coro: PyObject) -> i64 {
    if !is_python_available() {
        let err =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err);
    }
    let unwrapped = unsafe { olive_py_unwrap(coro) };
    if unwrapped.is_null() {
        let err = crate::olive_str_internal("Null coroutine pointer");
        return crate::result::olive_result_err(err);
    }
    with_gil(|| unsafe {
        let asyncio = PY_IMPORT_IMPORT_MODULE(b"asyncio\0".as_ptr() as *const c_char);
        if asyncio.is_null() {
            if let Some(msg) = catch_py_exception_msg() {
                return crate::result::olive_result_err(crate::olive_str_internal(&msg));
            }
            return crate::result::olive_result_err(crate::olive_str_internal(
                "Failed to import asyncio",
            ));
        }

        let run_fn = PY_OBJECT_GET_ATTR_STRING(asyncio, b"run\0".as_ptr() as *const c_char);
        let result = if !run_fn.is_null() {
            let args = PY_TUPLE_NEW(1);
            PY_INC_REF(unwrapped);
            PY_TUPLE_SET_ITEM(args, 0, unwrapped);
            let res = PY_OBJECT_CALL_OBJECT(run_fn, args);
            PY_DEC_REF(args);
            PY_DEC_REF(run_fn);
            res
        } else {
            PY_ERR_CLEAR();
            let get_loop_fn =
                PY_OBJECT_GET_ATTR_STRING(asyncio, b"get_event_loop\0".as_ptr() as *const c_char);
            if get_loop_fn.is_null() {
                PY_DEC_REF(asyncio);
                if let Some(msg) = catch_py_exception_msg() {
                    return crate::result::olive_result_err(crate::olive_str_internal(&msg));
                }
                return crate::result::olive_result_err(crate::olive_str_internal(
                    "Failed to get event loop",
                ));
            }
            let loop_obj = PY_OBJECT_CALL_OBJECT(get_loop_fn, std::ptr::null_mut());
            PY_DEC_REF(get_loop_fn);
            if loop_obj.is_null() {
                PY_DEC_REF(asyncio);
                if let Some(msg) = catch_py_exception_msg() {
                    return crate::result::olive_result_err(crate::olive_str_internal(&msg));
                }
                return crate::result::olive_result_err(crate::olive_str_internal(
                    "Failed to get event loop object",
                ));
            }
            let run_until = PY_OBJECT_GET_ATTR_STRING(
                loop_obj,
                b"run_until_complete\0".as_ptr() as *const c_char,
            );
            if run_until.is_null() {
                PY_DEC_REF(loop_obj);
                PY_DEC_REF(asyncio);
                if let Some(msg) = catch_py_exception_msg() {
                    return crate::result::olive_result_err(crate::olive_str_internal(&msg));
                }
                return crate::result::olive_result_err(crate::olive_str_internal(
                    "Failed to get run_until_complete",
                ));
            }
            let args = PY_TUPLE_NEW(1);
            PY_INC_REF(unwrapped);
            PY_TUPLE_SET_ITEM(args, 0, unwrapped);
            let res = PY_OBJECT_CALL_OBJECT(run_until, args);
            PY_DEC_REF(args);
            PY_DEC_REF(run_until);
            PY_DEC_REF(loop_obj);
            res
        };

        PY_DEC_REF(asyncio);

        if result.is_null() {
            if let Some(msg) = catch_py_exception_msg() {
                return crate::result::olive_result_err(crate::olive_str_internal(&msg));
            }
            return crate::result::olive_result_err(crate::olive_str_internal(
                "Coroutine execution failed",
            ));
        }

        let wrapped = olive_py_wrap_owned(result);
        crate::result::olive_result_ok(wrapped as i64)
    })
}
