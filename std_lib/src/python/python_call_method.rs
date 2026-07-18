use crate::python::*;
use std::os::raw::c_char;
use std::sync::atomic::Ordering;

/// Shared body for `olive_py_call_method0..4`: fuses `obj.attr(args...)`
/// into one call, one GIL region, no intermediate bound-method object, when
/// both `PyObject_VectorcallMethod` (R6) and interning (R8) are available.
/// `PyObject_VectorcallMethod` borrows every arg including `self` (unlike
/// `PyTuple_SetItem`, which steals), so each converted arg needs its own
/// decref after the call; `obj` itself is never decref'd, it stays owned by
/// the caller's own local. Falls back to a plain `GetAttr` (interned or not)
/// producing a real bound-method object, then the existing tagged call body
/// -- still one GIL region, just missing the fusion.
pub(crate) unsafe fn call_method_with_raw_args(
    obj: PyObject,
    attr: *const c_char,
    coll_tags: i64,
    arg_tags: i64,
    args: &mut [i64],
) -> PyObject {
    unsafe {
        if HAS_VECTORCALL.load(Ordering::Relaxed) && use_interned_names() {
            let name = interned_attr(attr);
            let mut pairs = Vec::new();
            let res = if name.is_null() {
                std::ptr::null_mut()
            } else {
                // Slot 0 is reserved scratch space per the ARGUMENTS_OFFSET
                // contract, slot 1 is `self`; converted args start at 2.
                let mut buf: [PyObject; 6] = [std::ptr::null_mut(); 6];
                buf[1] = obj;
                for (i, slot) in args.iter_mut().enumerate() {
                    let coll_tag = tag_at(coll_tags, i);
                    let arg_tag = arg_tag_at(arg_tags, i);
                    let py_v = convert_arg_tagged(*slot, coll_tag, arg_tag, &mut pairs);
                    if py_v.is_null() || !PY_ERR_OCCURRED().is_null() {
                        handle_py_error();
                    }
                    buf[i + 2] = py_v;
                    if coll_tag != TAG_NONE {
                        *slot = 0;
                    }
                }
                let nargsf = (args.len() + 1) | PY_VECTORCALL_ARGUMENTS_OFFSET;
                let r =
                    PY_VECTORCALL_METHOD(name, buf.as_ptr().add(1), nargsf, std::ptr::null_mut());
                for slot in &buf[2..=args.len() + 1] {
                    if !slot.is_null() {
                        PY_DEC_REF(*slot);
                    }
                }
                r
            };
            sync_back(&pairs);
            if res.is_null() {
                handle_py_error();
            } else if !PY_ERR_OCCURRED().is_null() {
                // Some libraries handle exceptions internally yet leave the indicator set.
                PY_ERR_CLEAR();
            }
            res
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
                handle_py_error();
            }
            let res = call_with_raw_args(bound, coll_tags, arg_tags, args);
            PY_DEC_REF(bound);
            res
        }
    }
}

/// Arity-specialized fused method-call entry points: the compiler emits
/// these for `obj.attr(...)` with 0-4 positional arguments and no keywords,
/// the shape covering nearly every real method call. Thin shells over
/// `call_method_with_raw_args`; no logic lives here beyond assembling the
/// fixed-size local array and finishing the result per `ret_tag` (packed
/// into `arg_tags`'s top 4 bits, mirroring `olive_py_call0..4` exactly --
/// `olive_py_call_method0` takes the word purely to carry it, having no real
/// argument tags of its own). Arity 5+ and any kwargs call keep the original
/// separate getattr-then-call path, always unfused.
///
/// `loc` (R17): see `olive_py_call0`'s doc comment -- same fold-in, same
/// first-action-of-the-function contract.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_method0(
    obj: PyObject,
    name: i64,
    arg_tags: i64,
    loc: i64,
) -> PyObject {
    set_py_call_loc(loc);
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    let attr_ptr = (name & !1) as *const c_char;
    unsafe {
        olive_py_gil_begin();
        let res = call_method_with_raw_args(unwrapped_obj, attr_ptr, 0, 0, &mut []);
        let ret_tag = ret_tag_of(arg_tags);
        if ret_tag == RET_HANDLE {
            olive_py_gil_end();
            return olive_py_wrap_owned(res);
        }
        let out = finish_ret(res, ret_tag);
        olive_py_gil_end();
        out as PyObject
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_method1(
    obj: PyObject,
    name: i64,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    loc: i64,
) -> PyObject {
    set_py_call_loc(loc);
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    let attr_ptr = (name & !1) as *const c_char;
    unsafe {
        olive_py_gil_begin();
        let mut args = [a0];
        let res =
            call_method_with_raw_args(unwrapped_obj, attr_ptr, coll_tags, arg_tags, &mut args);
        let ret_tag = ret_tag_of(arg_tags);
        if ret_tag == RET_HANDLE {
            olive_py_gil_end();
            return olive_py_wrap_owned(res);
        }
        let out = finish_ret(res, ret_tag);
        olive_py_gil_end();
        out as PyObject
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_method2(
    obj: PyObject,
    name: i64,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    a1: i64,
    loc: i64,
) -> PyObject {
    set_py_call_loc(loc);
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    let attr_ptr = (name & !1) as *const c_char;
    unsafe {
        olive_py_gil_begin();
        let mut args = [a0, a1];
        let res =
            call_method_with_raw_args(unwrapped_obj, attr_ptr, coll_tags, arg_tags, &mut args);
        let ret_tag = ret_tag_of(arg_tags);
        if ret_tag == RET_HANDLE {
            olive_py_gil_end();
            return olive_py_wrap_owned(res);
        }
        let out = finish_ret(res, ret_tag);
        olive_py_gil_end();
        out as PyObject
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_method3(
    obj: PyObject,
    name: i64,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    loc: i64,
) -> PyObject {
    set_py_call_loc(loc);
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    let attr_ptr = (name & !1) as *const c_char;
    unsafe {
        olive_py_gil_begin();
        let mut args = [a0, a1, a2];
        let res =
            call_method_with_raw_args(unwrapped_obj, attr_ptr, coll_tags, arg_tags, &mut args);
        let ret_tag = ret_tag_of(arg_tags);
        if ret_tag == RET_HANDLE {
            olive_py_gil_end();
            return olive_py_wrap_owned(res);
        }
        let out = finish_ret(res, ret_tag);
        olive_py_gil_end();
        out as PyObject
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_method4(
    obj: PyObject,
    name: i64,
    coll_tags: i64,
    arg_tags: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    a3: i64,
    loc: i64,
) -> PyObject {
    set_py_call_loc(loc);
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    let attr_ptr = (name & !1) as *const c_char;
    unsafe {
        olive_py_gil_begin();
        let mut args = [a0, a1, a2, a3];
        let res =
            call_method_with_raw_args(unwrapped_obj, attr_ptr, coll_tags, arg_tags, &mut args);
        let ret_tag = ret_tag_of(arg_tags);
        if ret_tag == RET_HANDLE {
            olive_py_gil_end();
            return olive_py_wrap_owned(res);
        }
        let out = finish_ret(res, ret_tag);
        olive_py_gil_end();
        out as PyObject
    }
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
    fn method_call_round_trips_all_arities_both_fusion_states() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        for &(vc, intern) in &[(true, true), (true, false), (false, true), (false, false)] {
            with_forced_fusion(vc, intern, || unsafe {
                let obj = with_gil(|| {
                    eval_main_obj(
                        "class __TMC:\n    def m0(self):\n        return 111\n    def m1(self, a):\n        return a + 1\n    def m2(self, a, b):\n        return a + b\n    def m3(self, a, b, c):\n        return a + b + c\n    def m4(self, a, b, c, d):\n        return a + b + c + d\n__tmc_obj = __TMC()\n",
                        "__tmc_obj",
                    )
                });

                let n0 = static_attr_name("m0");
                let r0 = olive_py_call_method0(obj, n0, 0, 0);
                assert_eq!(with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(r0))), 111);
                olive_py_decref(r0);

                let n1 = static_attr_name("m1");
                let r1 = olive_py_call_method1(obj, n1, 0, ARG_INT, 41, 0);
                assert_eq!(with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(r1))), 42);
                olive_py_decref(r1);

                let n2 = static_attr_name("m2");
                let tags2 = ARG_INT | (ARG_INT << 4);
                let r2 = olive_py_call_method2(obj, n2, 0, tags2, 10, 20, 0);
                assert_eq!(with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(r2))), 30);
                olive_py_decref(r2);

                let n3 = static_attr_name("m3");
                let tags3 = ARG_INT | (ARG_INT << 4) | (ARG_INT << 8);
                let r3 = olive_py_call_method3(obj, n3, 0, tags3, 1, 2, 3, 0);
                assert_eq!(with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(r3))), 6);
                olive_py_decref(r3);

                let n4 = static_attr_name("m4");
                let tags4 = ARG_INT | (ARG_INT << 4) | (ARG_INT << 8) | (ARG_INT << 12);
                let r4 = olive_py_call_method4(obj, n4, 0, tags4, 1, 2, 3, 4, 0);
                assert_eq!(with_gil(|| PY_LONG_AS_LONG(olive_py_unwrap(r4))), 10);
                olive_py_decref(r4);

                olive_py_decref(obj);
            });
        }
    }

    #[test]
    fn refcount_stable_across_many_method_calls_both_fusion_states() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        for &(vc, intern) in &[(true, true), (false, false)] {
            with_forced_fusion(vc, intern, || unsafe {
                let (obj, target_handle, target_raw) = with_gil(|| {
                    let o = eval_main_obj(
                        "class __TMCRef:\n    def echo(self, x):\n        return x\n__tmc_ref_obj = __TMCRef()\n",
                        "__tmc_ref_obj",
                    );
                    let target = PY_LIST_NEW(0);
                    let handle = olive_py_wrap_owned(target);
                    (o, handle, target)
                });

                let baseline = with_gil(|| *(target_raw as *const isize));
                let name = static_attr_name("echo");

                for _ in 0..100_000 {
                    let res =
                        olive_py_call_method1(obj, name, 0, ARG_PYOBJECT, target_handle as i64, 0);
                    olive_py_decref(res);
                }

                let after = with_gil(|| *(target_raw as *const isize));
                assert_eq!(
                    after, baseline,
                    "refcount leak or over-release across repeated method calls"
                );

                olive_py_decref(target_handle);
                olive_py_decref(obj);
            });
        }
    }

    #[test]
    fn collection_arg_still_syncs_back_through_fused_path() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_forced_fusion(true, true, || unsafe {
            let obj = with_gil(|| {
                eval_main_obj(
                    "class __TMCSort:\n    def sort_arg(self, xs):\n        xs.sort()\n__tmc_sort_obj = __TMCSort()\n",
                    "__tmc_sort_obj",
                )
            });

            let xs = crate::olive_list_new(0);
            crate::olive_list_append(xs, 3i64);
            crate::olive_list_append(xs, 1i64);
            crate::olive_list_append(xs, 2i64);

            let name = static_attr_name("sort_arg");
            let res = olive_py_call_method1(obj, name, TAG_INT_LIST, ARG_PYOBJECT, xs, 0);
            olive_py_decref(res);

            assert_eq!(crate::olive_list_len(xs), 3);
            assert_eq!(crate::olive_list_get(xs, 0), 1);
            assert_eq!(crate::olive_list_get(xs, 1), 2);
            assert_eq!(crate::olive_list_get(xs, 2), 3);

            olive_py_decref(obj);
        });
    }

    /// R10 result fusion through the fused method-call path: `arg_tags`'s
    /// top 4 bits carry `ret_tag` here exactly as they do for the plain
    /// arity shells, on both the vectorcall-method and the getattr-fallback
    /// branch of `call_method_with_raw_args`.
    #[test]
    fn method_call_ret_tag_fusion_converts_correctly_both_fusion_states() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        for &(vc, intern) in &[(true, true), (false, false)] {
            with_forced_fusion(vc, intern, || unsafe {
                let obj = with_gil(|| {
                    eval_main_obj(
                        "class __TMCRet:\n    def r0(self):\n        return 111\n    def area(self, w, h):\n        return w * h * 1.0\n__tmc_ret_obj = __TMCRet()\n",
                        "__tmc_ret_obj",
                    )
                });

                let n0 = static_attr_name("r0");
                let r0 = olive_py_call_method0(obj, n0, RET_INT << 60, 0);
                assert_eq!(r0 as i64, 111);

                let n_area = static_attr_name("area");
                let tags2 = ARG_INT | (ARG_INT << 4) | (RET_FLOAT << 60);
                let area = olive_py_call_method2(obj, n_area, 0, tags2, 3, 4, 0);
                assert_eq!(f64::from_bits(area as u64), 12.0);

                olive_py_decref(obj);
            });
        }
    }
}
