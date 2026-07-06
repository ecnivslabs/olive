use crate::python::*;
use std::ffi::CString;

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call(func: PyObject, args_list: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();

        let mut py_args = std::ptr::null_mut();
        if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            py_args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let v = *sv.ptr.add(i);
                let mut py_v = crate::python::olive_py_to_sequence(v);
                if py_v.is_null() {
                    py_v = olive_to_py(v);
                }
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    handle_py_error();
                }
                PY_TUPLE_SET_ITEM(py_args, i as isize, py_v);
            }
        }

        let res = PY_OBJECT_CALL_OBJECT(unwrapped_func, py_args);

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
pub extern "C" fn olive_py_call_kw(func: PyObject, args_list: i64, kwargs_dict: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();

        let py_args = if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            let args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let v = *sv.ptr.add(i);
                let mut py_v = crate::python::olive_py_to_sequence(v);
                if py_v.is_null() {
                    py_v = olive_to_py(v);
                }
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    handle_py_error();
                }
                PY_TUPLE_SET_ITEM(args, i as isize, py_v);
            }
            args
        } else {
            PY_TUPLE_NEW(0)
        };

        let mut py_kwargs = std::ptr::null_mut();
        if kwargs_dict != 0 {
            let sv = &*(kwargs_dict as *const crate::StableVec);
            py_kwargs = PY_DICT_NEW();
            for i in (0..sv.len).step_by(2) {
                let k_ptr = *sv.ptr.add(i);
                let v = *sv.ptr.add(i + 1);

                let k_str = crate::olive_str_from_ptr(k_ptr);
                let k_cstr = CString::new(k_str).unwrap();
                let mut py_v = crate::python::olive_py_to_sequence(v);
                if py_v.is_null() {
                    py_v = olive_to_py(v);
                }
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    handle_py_error();
                }

                PY_DICT_SET_ITEM_STRING(py_kwargs, k_cstr.as_ptr(), py_v);
                PY_DEC_REF(py_v);
            }
        }

        let res = PY_OBJECT_CALL(unwrapped_func, py_args, py_kwargs);
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
