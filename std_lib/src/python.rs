pub mod python_async;
pub mod python_bindings;
pub mod python_call;
pub mod python_coerce;
pub mod python_coerce_ffi;
pub mod python_compat;
pub mod python_error;
pub mod python_iter;
pub mod python_lifecycle;
pub mod python_math;
pub mod python_noop;
pub mod python_safe;

pub use python_async::*;
pub use python_bindings::*;
pub use python_call::*;
pub use python_coerce::*;
pub use python_coerce_ffi::*;
pub use python_compat::*;
pub use python_error::*;
pub use python_iter::*;
pub use python_lifecycle::*;
pub use python_math::*;
pub use python_noop::*;
pub use python_safe::*;

use std::os::raw::c_void;

pub type PyObject = *mut c_void;

std::thread_local! {
    pub static GIL_HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn with_gil<R, F: FnOnce() -> R>(f: F) -> R {
    GIL_HELD.with(|held| {
        if held.get() {
            f()
        } else {
            held.set(true);
            unsafe {
                let gil = PY_GILSTATE_ENSURE();
                let res = f();
                PY_GILSTATE_RELEASE(gil);
                held.set(false);
                res
            }
        }
    })
}

pub fn is_readable_ptr(ptr: *const c_void) -> bool {
    crate::is_active_object(ptr as i64)
}

pub fn is_python_available() -> bool {
    if !INITIALIZED.load(std::sync::atomic::Ordering::Relaxed) {
        olive_py_initialize();
    }
    INITIALIZED.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn check_python_loaded() {
    if !is_python_available() {
        let msg =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        crate::olive_panic(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::raw::c_char;

    #[test]
    fn test_any_to_py_unboxes_but_raw_passes_through() {
        let _guard = crate::python::python_coerce::arena_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        // olive_any_to_py decodes tagged ints; olive_to_py must NOT (raw 2 shares tag bits).
        let boxed = crate::boxed::olive_box_int(7);
        assert_ne!(boxed, 7, "small int should be inline-tagged");
        let py = with_gil(|| crate::python::olive_any_to_py(boxed));
        let back = with_gil(|| unsafe { PY_LONG_AS_LONG(py) });
        assert_eq!(back, 7);
        with_gil(|| unsafe { PY_DEC_REF(py) });

        let raw = with_gil(|| crate::python::olive_to_py(2));
        let raw_back = with_gil(|| unsafe { PY_LONG_AS_LONG(raw) });
        assert_eq!(raw_back, 2, "a raw int must pass through unchanged");
        with_gil(|| unsafe { PY_DEC_REF(raw) });
    }

    #[test]
    fn test_dict_get_default_on_pyobject() {
        let _guard = crate::python::python_coerce::arena_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        // `dict.get(key, default)` on a Python dict must look the key up by item,
        // not attribute. A real `dict` has no `.title` attribute, so a getattr
        // would raise; the correct behaviour returns the item or the default.
        use crate::python::python_coerce_ffi::olive_py_dict_get_default;
        unsafe {
            let handle = with_gil(|| {
                let d = PY_DICT_NEW();
                let key = PY_UNICODE_FROM_STRING(b"title\0".as_ptr() as *const _);
                let val = PY_UNICODE_FROM_STRING(b"hello\0".as_ptr() as *const _);
                PY_DICT_SET_ITEM_STRING(d, b"title\0".as_ptr() as *const _, val);
                PY_DEC_REF(key);
                PY_DEC_REF(val);
                olive_py_wrap_owned(d)
            });
            let title_key = crate::olive_str_internal("title") | 1;
            let missing_key = crate::olive_str_internal("nope") | 1;
            let default = crate::olive_str_internal("DEFAULT") | 1;

            let got = olive_py_dict_get_default(handle as i64, title_key, default);
            assert_eq!(
                crate::olive_str_from_ptr(got),
                "hello",
                "present key returns its value, not a getattr error"
            );
            let miss = olive_py_dict_get_default(handle as i64, missing_key, default);
            assert_eq!(
                crate::olive_str_from_ptr(miss),
                "DEFAULT",
                "absent key returns the default"
            );
            olive_py_decref(handle);

            // A numeric value must come back boxed into `Any` so `float()`/`int()`
            // on it unbox correctly instead of reading a raw word as a pointer.
            let nhandle = with_gil(|| {
                let d = PY_DICT_NEW();
                PY_DICT_SET_ITEM_STRING(d, b"dur\0".as_ptr() as *const _, PY_LONG_FROM_LONG(31));
                olive_py_wrap_owned(d)
            });
            let dur_key = crate::olive_str_internal("dur") | 1;
            let boxed = olive_py_dict_get_default(nhandle as i64, dur_key, default);
            assert_eq!(
                crate::boxed::olive_unbox_float(boxed),
                31.0,
                "an int value boxes into Any so float() reads it"
            );
            olive_py_decref(nhandle);
        }
    }

    #[test]
    fn test_unbox_float_int_on_pyobject_in_any() {
        let _guard = crate::python::python_coerce::arena_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        // A `{str: Any}` value read from Python is a KIND_PYOBJECT handle. `float`
        // and `int` on it must unwrap and convert, not read the handle pointer.
        unsafe {
            let fhandle = with_gil(|| olive_py_wrap_owned(PY_FLOAT_FROM_DOUBLE(3.5)));
            assert_eq!(crate::boxed::olive_unbox_float(fhandle as i64), 3.5);
            assert_eq!(crate::boxed::olive_unbox_int(fhandle as i64), 3);
            olive_py_decref(fhandle);
        }
    }

    #[test]
    fn test_get_index_any_on_pyobject_nested_list() {
        let _guard = crate::python::python_coerce::arena_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        // Indexing a PyObject through `olive_get_index_any` calls `olive_py_getitem`
        // (which returns a wrapped arena handle) then converts the element. The
        // handle must be unwrapped before conversion, else `py_to_olive_internal`
        // reads its `py_ptr` field as an `ob_type` and crashes on a list element.
        unsafe {
            let outer = with_gil(|| {
                let inner = PY_LIST_NEW(2);
                PY_LIST_SET_ITEM(inner, 0, PY_LONG_FROM_LONG(42));
                PY_LIST_SET_ITEM(inner, 1, PY_LONG_FROM_LONG(99));
                let pylist = PY_LIST_NEW(1);
                PY_LIST_SET_ITEM(pylist, 0, inner);
                olive_py_wrap_owned(pylist)
            });
            let elem = crate::olive_get_index_any(outer as i64, 0, 0);
            assert!(
                crate::is_active_object(elem),
                "indexed element must be an object"
            );
            assert_eq!(
                *(elem as *const i64),
                crate::KIND_LIST,
                "a nested Python list element converts to an Olive list"
            );
            assert_eq!(crate::olive_list_len(elem), 2);
            assert_eq!(crate::olive_list_get(elem, 0), 42);
            assert_eq!(crate::olive_list_get(elem, 1), 99);
            crate::olive_free_list(elem);
            olive_py_decref(outer);
        }
    }

    #[test]
    fn test_realize_makes_real_dict() {
        let _guard = crate::python::python_coerce::arena_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let obj = crate::olive_obj_new();
            crate::olive_obj_set(
                obj,
                crate::olive_str_internal("k") | 1,
                crate::boxed::olive_box_int(5),
            );
            let realized = crate::python::python_coerce_ffi::olive_py_realize(obj);
            // The realized value is a wrapped Olive PyObject; unwrap to the raw
            // Python dict and confirm its type.
            let raw = olive_py_unwrap(realized);
            let is_dict = with_gil(|| {
                let ty = PY_OBJECT_TYPE(raw);
                !PY_DICT_TYPE.is_null()
                    && (ty == PY_DICT_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_DICT_TYPE) != 0)
            });
            assert!(is_dict, "realize must produce a real dict");
            olive_py_decref(realized);
            crate::olive_free_obj(obj);
        }
    }

    #[test]
    fn test_zero_copy_proxy_and_safe_boundaries() {
        let _guard = crate::python::python_coerce::arena_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }

        let py_num = olive_py_from_int(42);
        assert_eq!(olive_py_to_int(py_num), 42);
        olive_py_decref(py_num);

        let err_res = olive_py_import_safe(crate::olive_str_internal("non_existent_module_xyz"));
        assert_eq!(crate::result::olive_result_is_err(err_res), 1);

        let err_msg_ptr = crate::result::olive_result_unwrap_err(err_res);
        let err_msg = crate::olive_str_from_ptr(err_msg_ptr);
        assert!(err_msg.contains("No module named 'non_existent_module_xyz'"));
        crate::olive_free_any(err_res);

        unsafe {
            let list_ptr = crate::olive_list_new(2);
            let sv = &mut *(list_ptr as *mut crate::StableVec);
            *sv.ptr.add(0) = crate::olive_str_internal("hello");
            *sv.ptr.add(1) = crate::olive_str_internal("world");

            let py_proxy = olive_py_conv_to_py(list_ptr);
            assert!(!py_proxy.is_null());

            let sys_mod = with_gil(|| PY_IMPORT_IMPORT_MODULE(b"sys\0".as_ptr() as *const c_char));
            assert!(!sys_mod.is_null());

            let py_len = olive_py_len(py_proxy) as i64;
            assert_eq!(py_len, 2);

            let hello_ptr = crate::olive_list_get(list_ptr, 0);

            let idx_0 = with_gil(|| PY_LONG_FROM_LONG(0));
            let val_0 = with_gil(|| PY_UNICODE_FROM_STRING(b"world\0".as_ptr() as *const c_char));
            let setitem_res = with_gil(|| PY_OBJECT_SET_ITEM(py_proxy, idx_0, val_0));
            assert_ne!(setitem_res, -1);
            with_gil(|| {
                PY_DEC_REF(idx_0);
                PY_DEC_REF(val_0);
            });

            let olive_val_0 = crate::olive_list_get(list_ptr, 0);
            assert_eq!(crate::olive_str_from_ptr(olive_val_0), "world");

            let val_insert_olive = crate::olive_str_internal("inserted");
            crate::olive_list_insert(list_ptr, 1, val_insert_olive);

            assert_eq!(crate::olive_list_len(list_ptr), 3);
            let val_at_1 = crate::olive_list_get(list_ptr, 1);
            assert_eq!(crate::olive_str_from_ptr(val_at_1), "inserted");

            let idx_to_del = with_gil(|| PY_LONG_FROM_LONG(1));
            let del_res = with_gil(|| PY_OBJECT_DEL_ITEM(py_proxy, idx_to_del));
            assert_ne!(del_res, -1);
            with_gil(|| PY_DEC_REF(idx_to_del));

            assert_eq!(crate::olive_list_len(list_ptr), 2);
            assert_ne!(
                crate::olive_str_from_ptr(crate::olive_list_get(list_ptr, 1)),
                "inserted"
            );

            let dict_ptr = crate::olive_obj_new();
            let py_dict = olive_py_conv_to_py(dict_ptr);

            let dict_key =
                with_gil(|| PY_UNICODE_FROM_STRING(b"testkey\0".as_ptr() as *const c_char));
            let dict_val = with_gil(|| PY_LONG_FROM_LONG(9876));
            assert_ne!(
                with_gil(|| PY_OBJECT_SET_ITEM(py_dict, dict_key, dict_val)),
                -1
            );
            with_gil(|| {
                PY_DEC_REF(dict_key);
                PY_DEC_REF(dict_val);
            });

            assert_eq!(
                crate::olive_obj_get(dict_ptr, crate::olive_str_internal("testkey")),
                9876
            );

            let dict_del_key =
                with_gil(|| PY_UNICODE_FROM_STRING(b"testkey\0".as_ptr() as *const c_char));
            let dict_del_res = with_gil(|| PY_OBJECT_DEL_ITEM(py_dict, dict_del_key));
            assert_ne!(dict_del_res, -1);
            with_gil(|| PY_DEC_REF(dict_del_key));

            assert_eq!(
                crate::olive_obj_get(dict_ptr, crate::olive_str_internal("testkey")),
                0
            );

            olive_py_decref(py_dict);
            crate::olive_free_obj(dict_ptr);

            with_gil(|| {
                PY_DEC_REF(sys_mod);
            });
            olive_py_decref(py_proxy);

            crate::olive_free_any(hello_ptr);
            crate::olive_free_list(list_ptr);
        }
    }

    #[test]
    fn test_interop_leak_prevention() {
        let _guard = crate::python::python_coerce::arena_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        let mut allocated_ptrs = Vec::new();

        for i in 0..100 {
            let py_num = olive_py_from_int(i);
            assert_eq!(olive_py_to_int(py_num), i);
            allocated_ptrs.push(py_num as i64);

            let py_dict = unsafe {
                let d = with_gil(|| PY_DICT_NEW());
                olive_py_wrap_owned(d)
            };
            allocated_ptrs.push(py_dict as i64);

            olive_py_decref(py_num);
            olive_py_decref(py_dict);
        }

        for ptr in allocated_ptrs {
            assert!(
                olive_py_is_valid_proxy(ptr) == 0,
                "Active object (ptr={:#x}) leaked in interop wrapping!",
                ptr
            );
        }
    }
}
