use crate::python::*;
use std::ffi::CString;
use std::os::raw::{c_double, c_long};

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_int(v: i64) -> PyObject {
    check_python_loaded();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_LONG_FROM_LONG(v as c_long);
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(r)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_float(v: f64) -> PyObject {
    check_python_loaded();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_FLOAT_FROM_DOUBLE(v as c_double);
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(r)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_str(s: i64) -> PyObject {
    check_python_loaded();
    let r_str = crate::olive_str_from_ptr(s);
    let c = CString::new(r_str).unwrap();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_UNICODE_FROM_STRING(c.as_ptr());
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(r)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_conv_to_py(val: i64) -> PyObject {
    check_python_loaded();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = olive_to_py(val);
        PY_GILSTATE_RELEASE(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_conv_to_olive(py_val: PyObject) -> i64 {
    check_python_loaded();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = py_to_olive_internal(py_val);
        PY_GILSTATE_RELEASE(gil);
        r
    }
}

pub unsafe fn olive_py_create_list_proxy(ptr: i64) -> PyObject {
    unsafe {
        let obj = crate::python_proxy::PY_TYPE_GENERIC_ALLOC(
            crate::python_proxy::OLIVE_LIST_PROXY_TYPE,
            0,
        );
        if !obj.is_null() {
            (*(obj as *mut crate::python_proxy::NativeProxy)).ptr = ptr;
        }
        obj
    }
}

pub unsafe fn olive_py_create_dict_proxy(ptr: i64) -> PyObject {
    unsafe {
        let obj = crate::python_proxy::PY_TYPE_GENERIC_ALLOC(
            crate::python_proxy::OLIVE_DICT_PROXY_TYPE,
            0,
        );
        if !obj.is_null() {
            (*(obj as *mut crate::python_proxy::NativeProxy)).ptr = ptr;
        }
        obj
    }
}
