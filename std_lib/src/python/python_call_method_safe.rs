use crate::python::*;
use std::os::raw::c_char;
use std::sync::atomic::Ordering;

/// `Result`-returning counterpart to `python_call_method::call_method_with_raw_args`.
/// Mirrors `call_with_raw_args_safe`'s conversion-failure discipline exactly:
/// `Err` means argument conversion failed before any Python call ran, every
/// partially-converted slot has already been released and `pairs` abandoned.
/// A null bound method or null interned name is not distinguished here --
/// it flows through as `Ok(null)`, and `finish_call_safe` recovers the real
/// exception message the same way it does for every other entry point.
unsafe fn call_method_with_raw_args_safe(
    obj: PyObject,
    attr: *const c_char,
    coll_tags: i64,
    arg_tags: i64,
    args: &mut [i64],
) -> Result<PyObject, i64> {
    unsafe {
        if HAS_VECTORCALL.load(Ordering::Relaxed) && use_interned_names() {
            let name = interned_attr(attr);
            if name.is_null() {
                return Ok(std::ptr::null_mut());
            }
            let mut pairs = Vec::new();
            let mut buf: [PyObject; 6] = [std::ptr::null_mut(); 6];
            buf[1] = obj;
            for (i, slot) in args.iter_mut().enumerate() {
                let coll_tag = tag_at(coll_tags, i);
                let arg_tag = arg_tag_at(arg_tags, i);
                let py_v = convert_arg_tagged(*slot, coll_tag, arg_tag, &mut pairs);
                if coll_tag != TAG_NONE {
                    *slot = 0;
                }
                if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                    if !py_v.is_null() {
                        PY_DEC_REF(py_v);
                    }
                    for s in &buf[2..2 + i] {
                        if !s.is_null() {
                            PY_DEC_REF(*s);
                        }
                    }
                    abandon_pairs(&pairs);
                    return Err(conversion_err());
                }
                buf[i + 2] = py_v;
            }
            let nargsf = (args.len() + 1) | PY_VECTORCALL_ARGUMENTS_OFFSET;
            let res = PY_VECTORCALL_METHOD(name, buf.as_ptr().add(1), nargsf, std::ptr::null_mut());
            for slot in &buf[2..=args.len() + 1] {
                if !slot.is_null() {
                    PY_DEC_REF(*slot);
                }
            }
            sync_back(&pairs);
            Ok(res)
        } else {
            let bound = if use_interned_names() {
                let name = interned_attr(attr);
                if name.is_null() {
                    std::ptr::null_mut()
                } else {
                    PY_OBJECT_GET_ATTR(obj, name)
                }
            } else {
                PY_OBJECT_GET_ATTR_STRING(obj, attr)
            };
            if bound.is_null() {
                return Ok(std::ptr::null_mut());
            }
            let outcome = call_with_raw_args_safe(bound, coll_tags, arg_tags, args);
            PY_DEC_REF(bound);
            outcome
        }
    }
}

/// `Result`-returning arity-specialized shells; see `olive_py_call_method0..4`.
/// `arg_tags`'s top 4 bits carry `ret_tag`, mirroring the non-`_safe` twin.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_method0_safe(obj: PyObject, name: i64, arg_tags: i64) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null object pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let attr_ptr = (name & !1) as *const c_char;
    with_gil(|| unsafe {
        let outcome = call_method_with_raw_args_safe(unwrapped_obj, attr_ptr, 0, 0, &mut []);
        finish_call_safe(outcome, ret_tag_of(arg_tags))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_method1_safe(
    obj: PyObject,
    name: i64,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null object pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let attr_ptr = (name & !1) as *const c_char;
    with_gil(|| unsafe {
        let mut args = [a0];
        let outcome =
            call_method_with_raw_args_safe(unwrapped_obj, attr_ptr, coll_tags, arg_tags, &mut args);
        finish_call_safe(outcome, ret_tag_of(arg_tags))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_method2_safe(
    obj: PyObject,
    name: i64,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    a1: i64,
) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null object pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let attr_ptr = (name & !1) as *const c_char;
    with_gil(|| unsafe {
        let mut args = [a0, a1];
        let outcome =
            call_method_with_raw_args_safe(unwrapped_obj, attr_ptr, coll_tags, arg_tags, &mut args);
        finish_call_safe(outcome, ret_tag_of(arg_tags))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_method3_safe(
    obj: PyObject,
    name: i64,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    a1: i64,
    a2: i64,
) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null object pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let attr_ptr = (name & !1) as *const c_char;
    with_gil(|| unsafe {
        let mut args = [a0, a1, a2];
        let outcome =
            call_method_with_raw_args_safe(unwrapped_obj, attr_ptr, coll_tags, arg_tags, &mut args);
        finish_call_safe(outcome, ret_tag_of(arg_tags))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_method4_safe(
    obj: PyObject,
    name: i64,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    a3: i64,
) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null object pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let attr_ptr = (name & !1) as *const c_char;
    with_gil(|| unsafe {
        let mut args = [a0, a1, a2, a3];
        let outcome =
            call_method_with_raw_args_safe(unwrapped_obj, attr_ptr, coll_tags, arg_tags, &mut args);
        finish_call_safe(outcome, ret_tag_of(arg_tags))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::python_coerce::{pyobject_slab_test_lock, static_attr_name};

    fn with_forced_fusion<R>(vectorcall: bool, intern: bool, f: impl FnOnce() -> R) -> R {
        let prev_vc = HAS_VECTORCALL.load(Ordering::SeqCst);
        let prev_in = HAS_INTERN.load(Ordering::SeqCst);
        HAS_VECTORCALL.store(vectorcall, Ordering::SeqCst);
        HAS_INTERN.store(intern, Ordering::SeqCst);
        let r = f();
        HAS_VECTORCALL.store(prev_vc, Ordering::SeqCst);
        HAS_INTERN.store(prev_in, Ordering::SeqCst);
        r
    }

    unsafe fn eval_main_obj(src: &str, name: &str) -> PyObject {
        unsafe {
            let c_src = std::ffi::CString::new(src).unwrap();
            PY_RUN_SIMPLE_STRING(c_src.as_ptr());
            let main_mod = PY_IMPORT_IMPORT_MODULE(b"__main__\0".as_ptr() as *const _);
            let c_name = std::ffi::CString::new(name).unwrap();
            let obj = PY_OBJECT_GET_ATTR_STRING(main_mod, c_name.as_ptr());
            PY_DEC_REF(main_mod);
            olive_py_wrap_owned(obj)
        }
    }

    #[test]
    fn safe_method_call_round_trips_both_fusion_states() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        for &(vc, intern) in &[(true, true), (false, false)] {
            with_forced_fusion(vc, intern, || unsafe {
                let obj = with_gil(|| {
                    eval_main_obj(
                        "class __TMCSafe:\n    def m0(self):\n        return 5\n    def m2(self, a, b):\n        return a * b\n__tmc_safe_obj = __TMCSafe()\n",
                        "__tmc_safe_obj",
                    )
                });

                let n0 = static_attr_name("m0");
                let r0 = olive_py_call_method0_safe(obj, n0, 0);
                assert_eq!(crate::result::olive_result_is_err(r0), 0);
                let ok0 = crate::result::olive_result_unwrap(r0);
                assert_eq!(
                    with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(ok0 as PyObject))),
                    5
                );
                olive_py_decref(ok0 as PyObject);

                let n2 = static_attr_name("m2");
                let tags2 = ARG_INT | (ARG_INT << 4);
                let r2 = olive_py_call_method2_safe(obj, n2, 0, tags2, 6, 7);
                assert_eq!(crate::result::olive_result_is_err(r2), 0);
                let ok2 = crate::result::olive_result_unwrap(r2);
                assert_eq!(
                    with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(ok2 as PyObject))),
                    42
                );
                olive_py_decref(ok2 as PyObject);

                olive_py_decref(obj);
            });
        }
    }

    #[test]
    fn safe_method_call_rejects_bad_str_arg_and_clears_error_both_fusion_states() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        for &(vc, intern) in &[(true, true), (false, false)] {
            with_forced_fusion(vc, intern, || unsafe {
                let obj = with_gil(|| {
                    eval_main_obj(
                        "class __TMCSafeBad:\n    def echo(self, x):\n        return x\n__tmc_safe_bad_obj = __TMCSafeBad()\n",
                        "__tmc_safe_bad_obj",
                    )
                });
                let bad = crate::string_slab::str_alloc(&[0xe0]);
                let name = static_attr_name("echo");

                let res = olive_py_call_method1_safe(obj, name, 0, ARG_STR, bad);
                assert_eq!(
                    crate::result::olive_result_is_err(res),
                    1,
                    "corrupt argument must fail the call"
                );
                let msg = crate::olive_str_from_ptr(crate::result::olive_result_err_msg(res));
                assert!(
                    msg.contains("UnicodeDecodeError") || msg.contains("utf-8"),
                    "error names the decode failure: {msg}"
                );
                with_gil(|| {
                    assert!(
                        PY_ERR_OCCURRED().is_null(),
                        "no exception may stay pending to poison later calls"
                    );
                });
                olive_py_decref(obj);
            });
        }
    }

    #[test]
    fn safe_method_call_on_missing_attr_reports_cleanly() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        for &(vc, intern) in &[(true, true), (false, false)] {
            with_forced_fusion(vc, intern, || unsafe {
                let obj = with_gil(|| olive_py_wrap_owned(PY_DICT_NEW()));
                let name = static_attr_name("__no_such_method_xyz");
                let res = olive_py_call_method0_safe(obj, name, 0);
                assert_eq!(crate::result::olive_result_is_err(res), 1);
                let msg = crate::olive_str_from_ptr(crate::result::olive_result_err_msg(res));
                assert!(
                    msg.contains("AttributeError") || msg.contains("has no attribute"),
                    "error names the missing attribute: {msg}"
                );
                with_gil(|| {
                    assert!(
                        PY_ERR_OCCURRED().is_null(),
                        "no exception may stay pending to poison later calls"
                    );
                });
                olive_py_decref(obj);
            });
        }
    }
}
