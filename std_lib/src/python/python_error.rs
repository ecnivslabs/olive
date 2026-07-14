use crate::python::*;
use std::ffi::CStr;

thread_local! {
    /// `file:line:col` of the Olive call site currently invoking Python, so an
    /// uncaught Python exception can be traced back to the exact Olive source line.
    /// Held as the call site's `Constant::Str` pointer -- compiler-deduplicated
    /// static rodata, stable for the process's life -- so recording a call site
    /// copies one word instead of rebuilding the string; it is decoded only when
    /// an abort actually needs it.
    static PY_CALL_LOC: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
}

/// Writes the Olive call site into the thread-local directly -- shared by
/// `olive_py_set_loc` (legacy calls, still a separate MIR statement) and the
/// R7/R9/R15 fast-path entry points, which call this as their first action
/// instead of paying a separate runtime call for it.
pub(crate) fn set_py_call_loc(ptr: i64) {
    PY_CALL_LOC.with(|l| l.set(ptr));
}

/// Records the Olive call site about to invoke a Python callable. Emitted by the
/// MIR builder immediately before a legacy (non-fast-path) Python call.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_set_loc(ptr: i64) {
    set_py_call_loc(ptr);
}

pub(crate) fn py_call_loc() -> String {
    let ptr = PY_CALL_LOC.with(|l| l.get());
    if ptr == 0 {
        String::new()
    } else {
        crate::olive_str_from_ptr(ptr)
    }
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
                        // The fetched triplet stays alive until process exit;
                        // nothing after `exit` runs, so no decref is owed.
                        std::process::exit(exit_code);
                    }
                }
                PY_DEC_REF(name_obj);
            } else if !PY_ERR_OCCURRED().is_null() {
                PY_ERR_CLEAR();
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
                // Legacy three-argument form; each slot needs its own
                // reference because `PyTuple_SetItem` steals.
                let args = PY_TUPLE_NEW(3);
                if !args.is_null() {
                    let inc_none = || {
                        PY_INC_REF(_PY_NONE_STRUCT);
                        _PY_NONE_STRUCT
                    };
                    PY_TUPLE_SET_ITEM(args, 0, if ptype.is_null() { inc_none() } else { ptype });
                    PY_TUPLE_SET_ITEM(args, 1, inc_none());
                    PY_TUPLE_SET_ITEM(
                        args,
                        2,
                        if ptraceback.is_null() { inc_none() } else { ptraceback },
                    );
                    ptype = std::ptr::null_mut();
                    ptraceback = std::ptr::null_mut();
                }
                args
            };

            if py_args.is_null() {
                PY_ERR_CLEAR();
            } else {
                PY_ERR_CLEAR();
                let py_list = PY_OBJECT_CALL_OBJECT(fmt_func, py_args);
                PY_DEC_REF(py_args);
                if py_list.is_null() {
                    PY_ERR_CLEAR();
                } else {
                    // A list's elements are borrowed (`PyList_GetItem`
                    // semantics), so read them directly -- no PyLong key and
                    // no generic `__getitem__` dispatch per line.
                    let len = PY_OBJECT_LENGTH(py_list).max(0) as usize;
                    for i in 0..len {
                        let py_item = PY_LIST_GET_ITEM(py_list, i as isize);
                        if !py_item.is_null() {
                            let s = PY_UNICODE_AS_UTF8(py_item);
                            if !s.is_null() {
                                tb_msg.push_str(&CStr::from_ptr(s).to_string_lossy());
                            }
                        }
                    }
                    PY_DEC_REF(py_list);
                }
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
                } else if !PY_ERR_OCCURRED().is_null() {
                    PY_ERR_CLEAR();
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
