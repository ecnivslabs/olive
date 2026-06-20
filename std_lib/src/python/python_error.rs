use crate::python::*;
use std::ffi::CStr;
use std::os::raw::c_long;

thread_local! {
    /// `file:line:col` of the Olive call site currently invoking Python, so an
    /// uncaught Python exception can be traced back to the exact Olive source line.
    static PY_CALL_LOC: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Records the Olive call site about to invoke a Python callable. Emitted by the
/// MIR builder immediately before every Python call.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_set_loc(ptr: i64) {
    let loc = crate::olive_str_from_ptr(ptr);
    PY_CALL_LOC.with(|l| *l.borrow_mut() = loc);
}

fn py_call_loc() -> String {
    PY_CALL_LOC.with(|l| l.borrow().clone())
}

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

        if !ptype.is_null() {
            let name_obj = PY_OBJECT_GET_ATTR_STRING(ptype, b"__name__\0".as_ptr() as *const _);
            if !name_obj.is_null() {
                let s = PY_UNICODE_AS_UTF8(name_obj);
                if !s.is_null() {
                    let name = CStr::from_ptr(s).to_string_lossy();
                    if name == "SystemExit" {
                        let mut exit_code = 0;
                        if !pvalue.is_null() {
                            let code_obj =
                                PY_OBJECT_GET_ATTR_STRING(pvalue, b"code\0".as_ptr() as *const _);
                            if !code_obj.is_null() {
                                exit_code = PY_LONG_AS_LONG(code_obj) as i32;
                                if !PY_ERR_OCCURRED().is_null() {
                                    PY_ERR_CLEAR();
                                    exit_code = 0;
                                }
                                PY_DEC_REF(code_obj);
                            }
                        }
                        std::process::exit(exit_code);
                    }
                }
                PY_DEC_REF(name_obj);
            }
        }

        let mut tb_msg = String::new();

        let fmt_func = PY_TRACEBACK_FORMAT_EXCEPTION;
        if !fmt_func.is_null() {
            let py_args = if !pvalue.is_null() {
                let args = PY_TUPLE_NEW(1);
                PY_TUPLE_SET_ITEM(args, 0, pvalue);
                pvalue = std::ptr::null_mut();
                args
            } else {
                let args = PY_TUPLE_NEW(3);
                let safe_type = if !ptype.is_null() {
                    ptype
                } else {
                    PY_INC_REF(_PY_NONE_STRUCT);
                    _PY_NONE_STRUCT
                };
                PY_INC_REF(_PY_NONE_STRUCT);
                let safe_value = _PY_NONE_STRUCT;
                let safe_tb = if !ptraceback.is_null() {
                    ptraceback
                } else {
                    PY_INC_REF(_PY_NONE_STRUCT);
                    _PY_NONE_STRUCT
                };
                PY_TUPLE_SET_ITEM(args, 0, safe_type);
                PY_TUPLE_SET_ITEM(args, 1, safe_value);
                PY_TUPLE_SET_ITEM(args, 2, safe_tb);
                ptype = std::ptr::null_mut();
                ptraceback = std::ptr::null_mut();
                args
            };

            PY_ERR_CLEAR();
            let py_list = PY_OBJECT_CALL_OBJECT(fmt_func, py_args);
            if py_list.is_null() {
                PY_ERR_PRINT();
            } else {
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
        let body = if tb_msg.is_empty() {
            "Python Exception: <unknown>".to_string()
        } else {
            tb_msg
        };
        let loc = py_call_loc();
        let msg = format!("uncaught Python exception\n{}", body.trim_end());
        if loc.is_empty() {
            crate::panic::abort_python(&msg, None)
        } else {
            crate::panic::abort_python(&msg, Some(&loc))
        }
    }
}

pub unsafe fn catch_py_exception_msg() -> Option<String> {
    unsafe {
        let msg = fetch_py_traceback();
        if msg.is_empty() { None } else { Some(msg) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_loc_roundtrips() {
        olive_py_set_loc(crate::olive_str_internal("/tmp/x.liv:3:15"));
        assert_eq!(py_call_loc(), "/tmp/x.liv:3:15");
    }

    #[test]
    fn loc_overwrites() {
        olive_py_set_loc(crate::olive_str_internal("a:1:1"));
        olive_py_set_loc(crate::olive_str_internal("b:2:2"));
        assert_eq!(py_call_loc(), "b:2:2");
    }
}
