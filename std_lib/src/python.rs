pub mod python_async;
pub mod python_bindings;
pub mod python_buffer;
pub mod python_call;
pub mod python_call_kw_v;
pub mod python_call_method;
pub mod python_call_method_safe;
pub mod python_callable;
pub mod python_coerce;
pub mod python_coerce_ffi;
pub mod python_compat;
pub mod python_dlpack;
pub mod python_error;
pub mod python_export_buffer;
pub mod python_intern;
pub mod python_iter;
pub mod python_kwnames;
pub mod python_lifecycle;
pub mod python_math;
pub mod python_noop;
pub mod python_pymodule;
pub mod python_ret;
pub mod python_safe;
pub(crate) mod python_subinterp;
pub mod python_writeback;

pub use python_async::*;
pub use python_bindings::*;
pub use python_call::*;
pub use python_coerce::*;
pub use python_coerce_ffi::*;
pub use python_compat::*;
pub use python_error::*;
pub(crate) use python_intern::*;
pub use python_iter::*;
pub use python_lifecycle::*;
pub use python_math::*;
pub use python_noop::*;
pub use python_pymodule::*;
pub(crate) use python_ret::*;
pub use python_safe::*;
pub(crate) use python_writeback::*;

use std::os::raw::c_void;

pub type PyObject = *mut c_void;

std::thread_local! {
    // Depth of nested GIL regions on this thread. Zero means the thread holds
    // no GIL state of its own; `with_gil` and `olive_py_gil_begin` both bump
    // it, so calls made while a fused region (R13) is open skip the real
    // ensure/release pair and just run.
    pub static GIL_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static GIL_TOKEN: std::cell::Cell<std::os::raw::c_int> = const { std::cell::Cell::new(0) };
}

/// Releases the GIL state (and resets `GIL_DEPTH`) on drop, so a panic
/// inside `with_gil`'s closure still restores both -- otherwise one
/// failing assertion anywhere under `with_gil` leaves the GIL held
/// forever and every other thread's next `with_gil` call hangs.
/// `-1` means the subinterpreter pool was used; `>=0` is a `PyGILState`
/// token.
struct GilGuard(std::os::raw::c_int);
impl Drop for GilGuard {
    fn drop(&mut self) {
        GIL_DEPTH.set(0);
        if self.0 == -1 {
            unsafe { python_subinterp::pool_release() };
        } else {
            unsafe { PY_GILSTATE_RELEASE(self.0) };
        }
    }
}

pub fn with_gil<R, F: FnOnce() -> R>(f: F) -> R {
    if GIL_DEPTH.get() > 0 {
        f()
    } else {
        unsafe {
            let token = gil_acquire();
            GIL_DEPTH.set(1);
            let _guard = GilGuard(token);
            f()
        }
    }
}

/// Acquire the GIL, routing through the subinterpreter pool when active.
/// Returns `-1` for subinterp mode (token not needed) or the `PyGILState`
/// token for main-GIL mode.
unsafe fn gil_acquire() -> std::os::raw::c_int {
    if python_subinterp::pool_is_active() && unsafe { python_subinterp::pool_ensure() } {
        -1
    } else {
        unsafe { PY_GILSTATE_ENSURE() }
    }
}

/// Opens a fused GIL region (R13): the MIR gil-fusion pass wraps a run of
/// consecutive `__olive_py_*` calls with one begin/end pair instead of one
/// pair per call. Every `__olive_py_*` entry point still calls this same
/// depth counter itself, so it composes with hand-written `with_gil` uses
/// and with re-fusion of an already-fused, inlined region.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_gil_begin() {
    check_python_loaded();
    let depth = GIL_DEPTH.get();
    if depth == 0 {
        let token = unsafe { gil_acquire() };
        GIL_TOKEN.set(token);
    }
    GIL_DEPTH.set(depth + 1);
}

/// Closes one level opened by `olive_py_gil_begin`. A fault inside a fused
/// region aborts the process (Olive faults never unwind), so an unmatched
/// begin can never leak the GIL into continued execution.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_gil_end() {
    let depth = GIL_DEPTH.get().saturating_sub(1);
    GIL_DEPTH.set(depth);
    if depth == 0 {
        let token = GIL_TOKEN.get();
        if token == -1 {
            unsafe { python_subinterp::pool_release() };
        } else {
            unsafe { PY_GILSTATE_RELEASE(token) };
        }
    }
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
        let _guard = crate::python::python_coerce::pyobject_slab_test_lock();
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
        let _guard = crate::python::python_coerce::pyobject_slab_test_lock();
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
        let _guard = crate::python::python_coerce::pyobject_slab_test_lock();
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
        let _guard = crate::python::python_coerce::pyobject_slab_test_lock();
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
        let _guard = crate::python::python_coerce::pyobject_slab_test_lock();
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
            let raw = with_gil(|| crate::python::olive_to_py(obj));
            let is_dict = with_gil(|| {
                let ty = PY_OBJECT_TYPE(raw);
                let dict_ty = PY_DICT_TYPE;
                !dict_ty.is_null() && ty == dict_ty
            });
            assert!(
                is_dict,
                "the boundary must produce a genuine dict, not a proxy"
            );
            with_gil(|| PY_DEC_REF(raw));
            crate::olive_free_obj(obj);
        }
    }

    /// R2a: the default olive-to-Python boundary always realizes a genuine
    /// `list`/`dict`, never a proxy. Mutating the realized object does not
    /// crash, and (since there is no copy-out mechanism until R2b) the
    /// mutation is simply not observed back on the Olive side.
    #[test]
    fn test_boundary_realizes() {
        let _guard = crate::python::python_coerce::pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }

        unsafe {
            let list_ptr = crate::olive_list_new(2);
            let sv = &mut *(list_ptr as *mut crate::StableVec);
            *sv.ptr.add(0) = crate::olive_str_internal("hello");
            *sv.ptr.add(1) = crate::olive_str_internal("world");

            let py_list = with_gil(|| crate::python::olive_to_py(list_ptr));
            assert!(!py_list.is_null());
            let list_ty = with_gil(|| PY_OBJECT_TYPE(py_list));
            let expected_list_ty = PY_LIST_TYPE;
            assert_eq!(
                list_ty, expected_list_ty,
                "realized list must be the genuine PyList_Type, not a proxy"
            );

            // Python-side mutation during the call must not crash.
            let new_item =
                with_gil(|| PY_UNICODE_FROM_STRING(b"changed\0".as_ptr() as *const c_char));
            let set_res = with_gil(|| PY_LIST_SET_ITEM(py_list, 0, new_item));
            assert_eq!(set_res, 0);

            // Olive side is unchanged: realize is a copy, not a live link.
            assert_eq!(
                crate::olive_str_from_ptr(crate::olive_list_get(list_ptr, 0)),
                "hello"
            );

            with_gil(|| PY_DEC_REF(py_list));
            crate::olive_free_list(list_ptr);

            let dict_ptr = crate::olive_obj_new();
            crate::olive_obj_set(
                dict_ptr,
                crate::olive_str_internal("testkey") | 1,
                crate::boxed::olive_box_int(9876),
            );
            let py_dict = with_gil(|| crate::python::olive_to_py(dict_ptr));
            assert!(!py_dict.is_null());
            let dict_ty = with_gil(|| PY_OBJECT_TYPE(py_dict));
            let expected_dict_ty = PY_DICT_TYPE;
            assert_eq!(
                dict_ty, expected_dict_ty,
                "realized dict must be the genuine PyDict_Type, not a proxy"
            );

            let new_val = with_gil(|| PY_LONG_FROM_LONG(1111));
            let set_res = with_gil(|| {
                PY_DICT_SET_ITEM_STRING(py_dict, b"testkey\0".as_ptr() as *const c_char, new_val)
            });
            assert_eq!(set_res, 0);
            with_gil(|| PY_DEC_REF(new_val));

            // Olive side is unchanged: realize is a copy, not a live link.
            assert_eq!(
                crate::olive_obj_get(dict_ptr, crate::olive_str_internal("testkey")),
                crate::boxed::olive_box_int(9876)
            );

            with_gil(|| PY_DEC_REF(py_dict));
            crate::olive_free_obj(dict_ptr);
        }
    }

    /// `isinstance(x, list)`/`isinstance(x, dict)` succeed against a
    /// realized Olive collection, both against the exact type and via
    /// Python's own `isinstance` builtin.
    #[test]
    fn test_boundary_isinstance_true() {
        let _guard = crate::python::python_coerce::pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }

        unsafe {
            let list_ptr = crate::olive_list_new(1);
            let sv = &mut *(list_ptr as *mut crate::StableVec);
            *sv.ptr.add(0) = crate::boxed::olive_box_int(1);

            let py_list = with_gil(|| crate::python::olive_to_py(list_ptr));
            let isinstance_ok = with_gil(|| {
                let list_ty = PY_LIST_TYPE;
                PY_TYPE_IS_SUBTYPE(PY_OBJECT_TYPE(py_list), list_ty) != 0
                    || PY_OBJECT_TYPE(py_list) == list_ty
            });
            assert!(isinstance_ok);
            with_gil(|| PY_DEC_REF(py_list));
            crate::olive_free_list(list_ptr);
        }

        let err_res = olive_py_import_safe(crate::olive_str_internal("non_existent_module_xyz"));
        assert_eq!(crate::result::olive_result_is_err(err_res), 1);

        let err_msg_ptr = crate::result::olive_result_unwrap_err(err_res);
        let err_msg = crate::olive_str_from_ptr(err_msg_ptr);
        assert!(err_msg.contains("No module named 'non_existent_module_xyz'"));
        crate::olive_free_any(err_res);

        let py_num = olive_py_from_int(42);
        assert_eq!(olive_py_to_int(py_num), 42);
        olive_py_decref(py_num);
    }

    #[test]
    fn test_interop_leak_prevention() {
        let _guard = crate::python::python_coerce::pyobject_slab_test_lock();
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
                !crate::is_active_object(ptr),
                "Active object (ptr={:#x}) leaked in interop wrapping!",
                ptr
            );
        }
    }

    // R13 gate measurement: cost of one PyGILState_Ensure/Release pair
    // against the py_binop bench's per-iteration time (~198ns, baseline
    // olive_mean 0.198035s / 1e6 iters), which already pays one such pair
    // per loop iteration. Not a correctness test; prints its verdict.
    #[test]
    fn bench_gil_ensure_release_pair_cost() {
        let _guard = crate::python::python_coerce::pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        const ITERS: u32 = 1_000_000;
        // Warm up: first ensure/release pair pays one-time thread-state setup.
        for _ in 0..1_000 {
            with_gil(|| ());
        }
        let start = std::time::Instant::now();
        for _ in 0..ITERS {
            with_gil(|| ());
        }
        let elapsed = start.elapsed();
        let ns_per_pair = elapsed.as_nanos() as f64 / ITERS as f64;
        let py_binop_iter_ns = 198.035_f64;
        let pct_of_loop_iter = 100.0 * ns_per_pair / py_binop_iter_ns;
        eprintln!(
            "GIL ensure/release pair: {ns_per_pair:.2}ns ({pct_of_loop_iter:.1}% of py_binop's {py_binop_iter_ns:.2}ns/iter)"
        );
    }
}
