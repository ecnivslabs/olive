use crate::python::*;
use std::ffi::CStr;
use std::os::raw::c_long;

pub unsafe fn fetch_py_traceback() -> String {
    unsafe {
        if PY_ERR_OCCURRED().is_null() {
            return String::new();
        }
        let mut ptype = std::ptr::null_mut();
        let mut pvalue = std::ptr::null_mut();
        let mut ptraceback = std::ptr::null_mut();
        PY_ERR_FETCH(&mut ptype, &mut pvalue, &mut ptraceback);
        PY_ERR_NORMALIZE_EXCEPTION(&mut ptype, &mut pvalue, &mut ptraceback);

        let mut tb_msg = String::new();

        if !ptraceback.is_null() {
            let fmt_func = PY_TRACEBACK_FORMAT_EXCEPTION;
            if !fmt_func.is_null() {
                let py_args = PY_TUPLE_NEW(3);
                PY_TUPLE_SET_ITEM(py_args, 0, ptype);
                PY_TUPLE_SET_ITEM(py_args, 1, pvalue);
                PY_TUPLE_SET_ITEM(py_args, 2, ptraceback);
                ptype = std::ptr::null_mut();
                pvalue = std::ptr::null_mut();
                ptraceback = std::ptr::null_mut();
                PY_ERR_CLEAR();
                let py_list = PY_OBJECT_CALL_OBJECT(fmt_func, py_args);
                if !py_list.is_null() {
                    let len = PY_OBJECT_LENGTH(py_list) as usize;
                    for i in 0..len {
                        let idx_obj = PY_LONG_FROM_LONG(i as c_long);
                        let py_item = PY_OBJECT_GET_ITEM(py_list, idx_obj);
                        if !py_item.is_null() {
                            let s = PY_UNICODE_AS_UTF8(py_item);
                            if !s.is_null() {
                                tb_msg.push_str(&CStr::from_ptr(s).to_string_lossy());
                            }
                            PY_DEC_REF(py_item);
                        }
                        PY_DEC_REF(idx_obj);
                    }
                    PY_DEC_REF(py_list);
                }
                PY_DEC_REF(py_args);
            }
        }

        if tb_msg.is_empty() {
            let mut err_msg = "Unknown Python Exception".to_string();
            if !pvalue.is_null() {
                let str_obj = PY_OBJECT_STR(pvalue);
                if !str_obj.is_null() {
                    let utf8_ptr = PY_UNICODE_AS_UTF8(str_obj);
                    if !utf8_ptr.is_null() {
                        err_msg = CStr::from_ptr(utf8_ptr).to_string_lossy().into_owned();
                    }
                    PY_DEC_REF(str_obj);
                }
            }
            tb_msg = format!("Python Exception: {}", err_msg);
        }

        PY_ERR_CLEAR();
        if !ptype.is_null() {
            PY_DEC_REF(ptype);
        }
        if !pvalue.is_null() {
            PY_DEC_REF(pvalue);
        }
        if !ptraceback.is_null() {
            PY_DEC_REF(ptraceback);
        }
        tb_msg
    }
}

pub unsafe fn handle_py_error() {
    unsafe {
        let tb_msg = fetch_py_traceback();
        if tb_msg.is_empty() {
            let ptr = crate::olive_str_internal("Python Exception: <unknown>");
            crate::olive_panic(ptr);
        } else {
            let ptr = crate::olive_str_internal(&tb_msg);
            crate::olive_panic(ptr);
        }
    }
}

pub unsafe fn catch_py_exception_msg() -> Option<String> {
    unsafe {
        let msg = fetch_py_traceback();
        if msg.is_empty() { None } else { Some(msg) }
    }
}
