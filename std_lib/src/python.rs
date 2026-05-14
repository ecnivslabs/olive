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
    fn test_realize_makes_real_dict() {
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
