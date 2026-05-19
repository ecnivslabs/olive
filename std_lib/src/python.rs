pub mod python_call;
pub mod python_coerce;
pub mod python_compat;
pub mod python_error;
pub mod python_lifecycle;
pub mod python_noop;

pub use python_call::*;
pub use python_coerce::*;
pub use python_compat::*;
pub use python_error::*;
pub use python_lifecycle::*;
pub use python_noop::*;

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_long, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

pub type PyObject = *mut c_void;

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static mut LIBPYTHON: *mut c_void = std::ptr::null_mut();

static mut PY_INITIALIZE: unsafe extern "C" fn() = noop_initialize;
static mut PY_FINALIZE: unsafe extern "C" fn() = noop_finalize;
static mut PY_IMPORT_IMPORT_MODULE: unsafe extern "C" fn(*const c_char) -> PyObject = noop_import;
static mut PY_OBJECT_GET_ATTR_STRING: unsafe extern "C" fn(PyObject, *const c_char) -> PyObject =
    noop_getattr;
static mut PY_OBJECT_SET_ATTR_STRING: unsafe extern "C" fn(
    PyObject,
    *const c_char,
    PyObject,
) -> c_int = noop_setattr;
static mut PY_RUN_SIMPLE_STRING: unsafe extern "C" fn(*const c_char) -> c_int =
    noop_run_simple_string;
static mut PY_OBJECT_CALL_OBJECT: unsafe extern "C" fn(PyObject, PyObject) -> PyObject = noop_call;
static mut PY_OBJECT_CALL: unsafe extern "C" fn(PyObject, PyObject, PyObject) -> PyObject =
    noop_call_kw;
pub static mut PY_DEC_REF: unsafe extern "C" fn(PyObject) = noop_decref;
static mut PY_INC_REF: unsafe extern "C" fn(PyObject) = noop_incref;
static mut PY_LONG_AS_LONG: unsafe extern "C" fn(PyObject) -> c_long = noop_as_long;
static mut PY_FLOAT_AS_DOUBLE: unsafe extern "C" fn(PyObject) -> c_double = noop_as_double;
pub static mut PY_UNICODE_AS_UTF8: unsafe extern "C" fn(PyObject) -> *const c_char = noop_as_utf8;
static mut PY_LONG_FROM_LONG: unsafe extern "C" fn(c_long) -> PyObject = noop_from_long;
static mut PY_FLOAT_FROM_DOUBLE: unsafe extern "C" fn(c_double) -> PyObject = noop_from_double;
static mut PY_UNICODE_FROM_STRING: unsafe extern "C" fn(*const c_char) -> PyObject =
    noop_from_string;
static mut PY_LIST_NEW: unsafe extern "C" fn(isize) -> PyObject = noop_list_new;
static mut PY_LIST_SET_ITEM: unsafe extern "C" fn(PyObject, isize, PyObject) -> c_int =
    noop_list_setitem;
static mut PY_OBJECT_GET_ITEM: unsafe extern "C" fn(PyObject, PyObject) -> PyObject = noop_getitem;
static mut PY_OBJECT_SET_ITEM: unsafe extern "C" fn(PyObject, PyObject, PyObject) -> c_int =
    noop_setitem;
static mut PY_OBJECT_DEL_ITEM: unsafe extern "C" fn(PyObject, PyObject) -> c_int =
    noop_dict_setitem_del;
static mut PY_OBJECT_LENGTH: unsafe extern "C" fn(PyObject) -> isize = noop_length;
static mut PY_GILSTATE_ENSURE: unsafe extern "C" fn() -> c_int = noop_gil_ensure;
static mut PY_GILSTATE_RELEASE: unsafe extern "C" fn(c_int) = noop_gil_release;
static mut PY_TUPLE_NEW: unsafe extern "C" fn(isize) -> PyObject = noop_tuple_new;
static mut PY_TUPLE_SET_ITEM: unsafe extern "C" fn(PyObject, isize, PyObject) -> c_int =
    noop_tuple_setitem;
static mut PY_DICT_NEW: unsafe extern "C" fn() -> PyObject = noop_dict_new;
static mut PY_DICT_SET_ITEM_STRING: unsafe extern "C" fn(
    PyObject,
    *const c_char,
    PyObject,
) -> c_int = noop_dict_setitemstring;
static mut PY_DICT_KEYS: unsafe extern "C" fn(PyObject) -> PyObject = noop_call_1;
static mut PY_OBJECT_TYPE: unsafe extern "C" fn(PyObject) -> PyObject = noop_call_1;
pub static mut PY_OBJECT_STR: unsafe extern "C" fn(PyObject) -> PyObject = noop_call_1;
static mut PY_ERR_OCCURRED: unsafe extern "C" fn() -> PyObject = noop_dict_new;
static mut PY_ERR_FETCH: unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) =
    noop_err_fetch;
static mut PY_ERR_NORMALIZE_EXCEPTION: unsafe extern "C" fn(
    *mut PyObject,
    *mut PyObject,
    *mut PyObject,
) = noop_err_fetch;
static mut PY_ERR_CLEAR: unsafe extern "C" fn() = noop_initialize;

static mut PY_SET_NEW: unsafe extern "C" fn(PyObject) -> PyObject = noop_call_1;
static mut PY_SET_ADD: unsafe extern "C" fn(PyObject, PyObject) -> c_int = noop_set_add;
static mut PY_BYTES_FROM_STRING_AND_SIZE: unsafe extern "C" fn(*const u8, isize) -> PyObject =
    noop_bytes_from_string;
static mut PY_SEQUENCE_LIST: unsafe extern "C" fn(PyObject) -> PyObject = noop_call_1;
static mut PY_BYTES_AS_STRING: unsafe extern "C" fn(PyObject) -> *const c_char =
    noop_bytes_as_string;
static mut PY_BYTES_SIZE: unsafe extern "C" fn(PyObject) -> isize = noop_bytes_size;

pub static mut PY_BOOL_TYPE: PyObject = std::ptr::null_mut();
pub static mut PY_LONG_TYPE: PyObject = std::ptr::null_mut();
pub static mut PY_FLOAT_TYPE: PyObject = std::ptr::null_mut();
pub static mut PY_UNICODE_TYPE: PyObject = std::ptr::null_mut();
pub static mut PY_LIST_TYPE: PyObject = std::ptr::null_mut();
pub static mut PY_DICT_TYPE: PyObject = std::ptr::null_mut();
pub static mut PY_SET_TYPE: PyObject = std::ptr::null_mut();
pub static mut PY_BYTES_TYPE: PyObject = std::ptr::null_mut();

pub static mut _PY_NONE_STRUCT: *mut c_void = std::ptr::null_mut();
pub static mut PY_ERR_PRINT: unsafe extern "C" fn() = noop_err_print;

pub static mut PY_TYPE_IS_SUBTYPE: unsafe extern "C" fn(PyObject, PyObject) -> c_int =
    noop_is_subtype;
pub static mut PY_EVAL_SAVE_THREAD: unsafe extern "C" fn() -> *mut c_void = noop_save_thread;
pub static mut PY_EVAL_RESTORE_THREAD: unsafe extern "C" fn(*mut c_void) = noop_restore_thread;
pub static mut PY_EVAL_INIT_THREADS: unsafe extern "C" fn() = noop_initialize;

pub static mut MAIN_THREAD_STATE: *mut c_void = std::ptr::null_mut();

pub static mut PY_SYS_GET_OBJECT: unsafe extern "C" fn(*const c_char) -> PyObject = noop_import;

unsafe fn compat_dlsym<T>(handle: *mut c_void, name: &str) -> T {
    unsafe {
        let cname = CString::new(name).unwrap();
        #[cfg(target_os = "windows")]
        let sym = { GetProcAddress(handle, cname.as_ptr() as *const u8) };
        #[cfg(not(target_os = "windows"))]
        let sym = { libc::dlsym(handle, cname.as_ptr()) };
        std::mem::transmute_copy(&sym)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_is_valid_proxy(ptr: i64) -> i64 {
    if crate::is_active_object(ptr) { 1 } else { 0 }
}

/// Shared traceback extraction. Fetches the current Python exception (if any),
/// normalises it, formats it via `traceback.format_exception`, and returns the
/// resulting string. Clears the Python error state on exit.
/// SAFETY: must be called with the GIL held.
fn is_python_available() -> bool {
    if !INITIALIZED.load(Ordering::Relaxed) {
        olive_py_initialize();
    }
    INITIALIZED.load(Ordering::SeqCst)
}

fn check_python_loaded() {
    if !is_python_available() {
        let msg =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        crate::olive_panic(msg);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_import(name: i64) -> PyObject {
    check_python_loaded();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let m = PY_IMPORT_IMPORT_MODULE((name & !1) as *const c_char);
        if m.is_null() {
            handle_py_error();
        }
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(m)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getattr(obj: PyObject, attr: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let a = PY_OBJECT_GET_ATTR_STRING(unwrapped_obj, (attr & !1) as *const c_char);
        if a.is_null() {
            handle_py_error();
        }
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(a)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setattr(obj: PyObject, attr: i64, val: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return obj;
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let py_val = olive_to_py(val);
        let res = PY_OBJECT_SET_ATTR_STRING(unwrapped_obj, (attr & !1) as *const c_char, py_val);
        if res == -1 {
            handle_py_error();
        }
        PY_DEC_REF(py_val);
        PY_GILSTATE_RELEASE(gil);
        obj
    }
}

fn olive_to_py(val: i64) -> PyObject {
    if val & 1 != 0 {
        let s = crate::olive_str_from_ptr(val);
        let c = CString::new(s).unwrap();
        unsafe { PY_UNICODE_FROM_STRING(c.as_ptr()) }
    } else if val == 0 {
        unsafe { _PY_NONE_STRUCT }
    } else {
        let ptr = val as *const c_void;
        if is_readable_ptr(ptr) {
            unsafe {
                let kind = *(ptr as *const i64);
                match kind {
                    crate::KIND_LIST => {
                        let raw = ptr as i64;
                        olive_py_create_list_proxy(raw)
                    }
                    crate::KIND_OBJ => {
                        let raw = ptr as i64;
                        olive_py_create_dict_proxy(raw)
                    }
                    crate::KIND_SET => {
                        let hs = &*(ptr as *const crate::OliveHashSet);
                        let pys = PY_SET_NEW(std::ptr::null_mut());
                        let vec_len = hs.len;
                        for i in 0..vec_len {
                            let v = *hs.ptr.add(i);
                            let py_v = olive_to_py(v);
                            PY_SET_ADD(pys, py_v);
                            PY_DEC_REF(py_v);
                        }
                        pys
                    }
                    crate::KIND_BYTES => {
                        let b = &*(ptr as *const crate::bytes::OliveBytes);
                        PY_BYTES_FROM_STRING_AND_SIZE(b.data.as_ptr(), b.data.len() as isize)
                    }
                    crate::KIND_PYOBJECT => {
                        let py_obj = &*(ptr as *const OlivePyObject);
                        let raw = py_obj.py_ptr;
                        PY_INC_REF(raw);
                        raw
                    }
                    _ => PY_LONG_FROM_LONG(val as c_long),
                }
            }
        } else {
            // Non-heap, non-string i64: treat as integer.
            unsafe { PY_LONG_FROM_LONG(val as c_long) }
        }
    }
}

/// Type-safe float conversion: interprets `val` as the bit-pattern of an f64
/// and returns a Python float object. Use this instead of `olive_to_py` whenever
/// the Olive type is statically known to be `float`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_float_bits(val: i64) -> PyObject {
    check_python_loaded();
    unsafe {
        let f = f64::from_bits(val as u64);
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_FLOAT_FROM_DOUBLE(f as c_double);
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(r)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_decref(obj: PyObject) {
    if !obj.is_null() {
        crate::unregister_object(obj as i64);
        unsafe {
            let ptr = obj as *const c_void;
            if is_readable_ptr(ptr) && *(ptr as *const i64) == crate::KIND_PYOBJECT {
                let boxed = Box::from_raw(obj as *mut OlivePyObject);
                let gil = PY_GILSTATE_ENSURE();
                PY_DEC_REF(boxed.py_ptr);
                PY_GILSTATE_RELEASE(gil);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_int(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let v = PY_LONG_AS_LONG(unwrapped_obj) as i64;
        PY_GILSTATE_RELEASE(gil);
        v
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_float(obj: PyObject) -> f64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0.0;
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let v = PY_FLOAT_AS_DOUBLE(unwrapped_obj) as f64;
        PY_GILSTATE_RELEASE(gil);
        v
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_str(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let str_obj = PY_OBJECT_STR(unwrapped_obj);
        let res = if !str_obj.is_null() {
            let s = PY_UNICODE_AS_UTF8(str_obj);
            let r = if !s.is_null() {
                let r_str = CStr::from_ptr(s).to_string_lossy();
                crate::olive_str_internal(&r_str)
            } else {
                0
            };
            PY_DEC_REF(str_obj);
            r
        } else {
            0
        };
        PY_GILSTATE_RELEASE(gil);
        res
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_list(s: i64) -> PyObject {
    check_python_loaded();
    if s == 0 {
        return std::ptr::null_mut();
    }
    unsafe {
        let sv = &*(s as *const crate::StableVec);
        let gil = PY_GILSTATE_ENSURE();
        let pyl = PY_LIST_NEW(sv.len as isize);
        for i in 0..sv.len {
            let v = *sv.ptr.add(i);
            let py_v = olive_to_py(v);
            PY_LIST_SET_ITEM(pyl, i as isize, py_v);
        }
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(pyl)
    }
}

unsafe fn py_to_olive_internal(py_val: PyObject) -> i64 {
    unsafe {
        if py_val.is_null() || py_val == _PY_NONE_STRUCT {
            return 0;
        }

        let ty = PY_OBJECT_TYPE(py_val);
        if ty.is_null() {
            return 0;
        }

        // Fast type-pointer comparison for Olive proxies — no attribute lookup or string allocation.
        let list_type = crate::python_proxy::OLIVE_LIST_PROXY_TYPE;
        let dict_type = crate::python_proxy::OLIVE_DICT_PROXY_TYPE;
        if (!list_type.is_null() && ty == list_type) || (!dict_type.is_null() && ty == dict_type) {
            let proxy = &*(py_val as *const crate::python_proxy::NativeProxy);
            PY_DEC_REF(ty);
            return proxy.ptr;
        }

        let is_subtype = |expected: PyObject| {
            if expected.is_null() {
                false
            } else {
                PY_TYPE_IS_SUBTYPE(ty, expected) != 0
            }
        };

        let result = if is_subtype(PY_BOOL_TYPE) {
            if PY_LONG_AS_LONG(py_val) != 0 { 1 } else { 0 }
        } else if is_subtype(PY_LONG_TYPE) {
            PY_LONG_AS_LONG(py_val) as i64
        } else if is_subtype(PY_FLOAT_TYPE) {
            let f = PY_FLOAT_AS_DOUBLE(py_val) as f64;
            f.to_bits() as i64
        } else if is_subtype(PY_UNICODE_TYPE) {
            let s = PY_UNICODE_AS_UTF8(py_val);
            if !s.is_null() {
                let r_str = CStr::from_ptr(s).to_string_lossy();
                crate::olive_str_internal(&r_str)
            } else {
                0
            }
        } else if is_subtype(PY_LIST_TYPE) {
            olive_py_to_list_internal(py_val)
        } else if is_subtype(PY_DICT_TYPE) {
            olive_py_to_dict_internal(py_val)
        } else if is_subtype(PY_SET_TYPE) {
            olive_py_to_set_internal(py_val)
        } else if is_subtype(PY_BYTES_TYPE) {
            olive_py_to_bytes_internal(py_val)
        } else {
            // Fallback: if the object has a finite length and supports __getitem__,
            // treat it as a sequence (e.g. numpy.ndarray row) and coerce to Olive list.
            let seq_len = PY_OBJECT_LENGTH(py_val);
            if seq_len >= 0 {
                olive_py_to_list_internal(py_val)
            } else {
                // Clear any TypeError raised by PyObject_Length on non-sequences
                PY_ERR_CLEAR();
                olive_py_wrap(py_val) as i64
            }
        };

        PY_DEC_REF(ty);
        result
    }
}

unsafe fn olive_py_to_list_internal(obj: PyObject) -> i64 {
    unsafe {
        let len = PY_OBJECT_LENGTH(obj);

        let len = len as usize;
        let list_ptr = crate::olive_list_new(len as i64);
        if len > 0 {
            let sv = &mut *(list_ptr as *mut crate::StableVec);
            for i in 0..len {
                let index_obj = PY_LONG_FROM_LONG(i as c_long);
                let py_item = PY_OBJECT_GET_ITEM(obj, index_obj);
                let olive_val = py_to_olive_internal(py_item);

                *sv.ptr.add(i) = olive_val;
                if !py_item.is_null() {
                    PY_DEC_REF(py_item);
                }
                if !index_obj.is_null() {
                    PY_DEC_REF(index_obj);
                }
            }
        }
        list_ptr
    }
}

unsafe fn olive_py_to_dict_internal(obj: PyObject) -> i64 {
    unsafe {
        let keys_list = PY_DICT_KEYS(obj);
        let olive_obj = crate::olive_obj_new();
        if !keys_list.is_null() {
            let len = PY_OBJECT_LENGTH(keys_list) as usize;
            for i in 0..len {
                let index_obj = PY_LONG_FROM_LONG(i as c_long);
                let key_obj = PY_OBJECT_GET_ITEM(keys_list, index_obj);
                if !key_obj.is_null() {
                    let val_obj = PY_OBJECT_GET_ITEM(obj, key_obj);

                    let key_str_obj = PY_OBJECT_STR(key_obj);
                    let key_utf8 = PY_UNICODE_AS_UTF8(key_str_obj);
                    if !key_utf8.is_null() {
                        let key_str = CStr::from_ptr(key_utf8).to_string_lossy();
                        let key_ptr = crate::olive_str_internal(&key_str);

                        let olive_val = py_to_olive_internal(val_obj);

                        crate::olive_obj_set(olive_obj, key_ptr, olive_val);
                    }

                    if !key_str_obj.is_null() {
                        PY_DEC_REF(key_str_obj);
                    }
                    if !val_obj.is_null() {
                        PY_DEC_REF(val_obj);
                    }
                    PY_DEC_REF(key_obj);
                }
                if !index_obj.is_null() {
                    PY_DEC_REF(index_obj);
                }
            }
            PY_DEC_REF(keys_list);
        }
        olive_obj
    }
}

unsafe fn olive_py_to_set_internal(obj: PyObject) -> i64 {
    unsafe {
        let py_list = PY_SEQUENCE_LIST(obj);
        if py_list.is_null() {
            return crate::olive_set_new(0);
        }
        let len = PY_OBJECT_LENGTH(py_list) as usize;
        let set_ptr = crate::olive_set_new(len as i64);
        for i in 0..len {
            let index_obj = PY_LONG_FROM_LONG(i as c_long);
            let py_item = PY_OBJECT_GET_ITEM(py_list, index_obj);
            let olive_val = py_to_olive_internal(py_item);
            crate::olive_set_add(set_ptr, olive_val);
            if !py_item.is_null() {
                PY_DEC_REF(py_item);
            }
            if !index_obj.is_null() {
                PY_DEC_REF(index_obj);
            }
        }
        PY_DEC_REF(py_list);
        set_ptr
    }
}

unsafe fn olive_py_to_bytes_internal(obj: PyObject) -> i64 {
    unsafe {
        let size = PY_BYTES_SIZE(obj) as usize;
        let buf_ptr = PY_BYTES_AS_STRING(obj);
        let bytes_ptr = crate::bytes::olive_buf_new(size as i64);
        if size > 0 && !buf_ptr.is_null() {
            let b = &mut *(bytes_ptr as *mut crate::bytes::OliveBytes);
            b.data.reserve(size);
            std::ptr::copy_nonoverlapping(buf_ptr as *const u8, b.data.as_mut_ptr(), size);
            b.data.set_len(size);
        }
        bytes_ptr
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_list(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = olive_py_to_list_internal(unwrapped_obj);
        PY_GILSTATE_RELEASE(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_dict(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = olive_py_to_dict_internal(unwrapped_obj);
        PY_GILSTATE_RELEASE(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getitem(obj: PyObject, key: PyObject) -> PyObject {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    let unwrapped_key = unsafe { olive_py_unwrap(key) };
    if unwrapped_obj.is_null() || unwrapped_key.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_OBJECT_GET_ITEM(unwrapped_obj, unwrapped_key);
        if r.is_null() {
            handle_py_error();
        }
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(r)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setitem(obj: PyObject, key: PyObject, val: PyObject) {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    let unwrapped_key = unsafe { olive_py_unwrap(key) };
    let unwrapped_val = unsafe { olive_py_unwrap(val) };
    if unwrapped_obj.is_null() || unwrapped_key.is_null() || unwrapped_val.is_null() {
        return;
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let res = PY_OBJECT_SET_ITEM(unwrapped_obj, unwrapped_key, unwrapped_val);
        if res == -1 {
            handle_py_error();
        }
        PY_GILSTATE_RELEASE(gil);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_len(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_OBJECT_LENGTH(unwrapped_obj) as i64;
        PY_GILSTATE_RELEASE(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_none() -> PyObject {
    check_python_loaded();
    unsafe { olive_py_wrap_borrowed(_PY_NONE_STRUCT) }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_is_none(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj == unsafe { _PY_NONE_STRUCT } {
        1
    } else {
        0
    }
}

#[repr(C)]
pub struct OlivePyObject {
    pub kind: i64,
    pub py_ptr: PyObject,
}

unsafe fn olive_py_wrap_owned(py_ptr: PyObject) -> PyObject {
    if py_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let boxed = Box::new(OlivePyObject {
        kind: crate::KIND_PYOBJECT,
        py_ptr,
    });
    let ptr = Box::into_raw(boxed) as PyObject;
    crate::register_object(ptr as i64);
    ptr
}

unsafe fn olive_py_wrap_borrowed(py_ptr: PyObject) -> PyObject {
    unsafe {
        if py_ptr.is_null() {
            return std::ptr::null_mut();
        }
        PY_INC_REF(py_ptr);
        olive_py_wrap_owned(py_ptr)
    }
}

unsafe fn olive_py_wrap(py_ptr: PyObject) -> PyObject {
    unsafe { olive_py_wrap_borrowed(py_ptr) }
}

unsafe fn olive_py_unwrap(val: PyObject) -> PyObject {
    unsafe {
        if val.is_null() {
            return std::ptr::null_mut();
        }
        let ptr = val as *const c_void;
        if is_readable_ptr(ptr) && *(ptr as *const i64) == crate::KIND_PYOBJECT {
            let py_obj = &*(ptr as *const OlivePyObject);
            py_obj.py_ptr
        } else {
            val
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_dict_keys_ffi(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        return 0;
    }
    unsafe {
        let obj = &*(obj_ptr as *const crate::OliveObj);
        let list_ptr = crate::olive_list_new(obj.fields.len() as i64);
        let sv = &mut *(list_ptr as *mut crate::StableVec);
        for (i, k) in obj.fields.keys().enumerate() {
            *sv.ptr.add(i) = crate::olive_str_internal(k);
        }
        list_ptr
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_import_safe(name: i64) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let m = PY_IMPORT_IMPORT_MODULE((name & !1) as *const c_char);
        if m.is_null() {
            if let Some(err_msg) = catch_py_exception_msg() {
                PY_GILSTATE_RELEASE(gil);
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }
        PY_GILSTATE_RELEASE(gil);
        let wrapped = olive_py_wrap_owned(m);
        crate::result::olive_result_ok(wrapped as i64)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_safe(func: PyObject, args_list: i64) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null function pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();

        let mut py_args = std::ptr::null_mut();
        if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            py_args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let v = *sv.ptr.add(i);
                let py_v = olive_to_py(v);
                PY_TUPLE_SET_ITEM(py_args, i as isize, py_v);
            }
        }

        let res = PY_OBJECT_CALL_OBJECT(unwrapped_func, py_args);
        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }

        if res.is_null() {
            if let Some(err_msg) = catch_py_exception_msg() {
                PY_GILSTATE_RELEASE(gil);
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }

        PY_GILSTATE_RELEASE(gil);
        let wrapped = olive_py_wrap_owned(res);
        crate::result::olive_result_ok(wrapped as i64)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_kw_safe(func: PyObject, args_list: i64, kwargs_dict: i64) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null function pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();

        let mut py_args = std::ptr::null_mut();
        if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            py_args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let v = *sv.ptr.add(i);
                let py_v = olive_to_py(v);
                PY_TUPLE_SET_ITEM(py_args, i as isize, py_v);
            }
        }

        let mut py_kwargs = std::ptr::null_mut();
        if kwargs_dict != 0 {
            let obj = &*(kwargs_dict as *const crate::OliveObj);
            py_kwargs = PY_DICT_NEW();
            for (k, &v) in &obj.fields {
                let k_cstr = CString::new(k.clone()).unwrap();
                let py_v = olive_to_py(v);
                PY_DICT_SET_ITEM_STRING(py_kwargs, k_cstr.as_ptr(), py_v);
                PY_DEC_REF(py_v);
            }
        }

        let res = PY_OBJECT_CALL(unwrapped_func, py_args, py_kwargs);
        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }
        if !py_kwargs.is_null() {
            PY_DEC_REF(py_kwargs);
        }

        if res.is_null() {
            if let Some(err_msg) = catch_py_exception_msg() {
                PY_GILSTATE_RELEASE(gil);
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }

        PY_GILSTATE_RELEASE(gil);
        let wrapped = olive_py_wrap_owned(res);
        crate::result::olive_result_ok(wrapped as i64)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getattr_safe(obj: PyObject, attr: i64) -> i64 {
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
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_OBJECT_GET_ATTR_STRING(unwrapped_obj, (attr & !1) as *const c_char);
        if r.is_null() {
            if let Some(err_msg) = catch_py_exception_msg() {
                PY_GILSTATE_RELEASE(gil);
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }
        PY_GILSTATE_RELEASE(gil);
        let wrapped = olive_py_wrap_owned(r);
        crate::result::olive_result_ok(wrapped as i64)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setattr_safe(obj: PyObject, attr: i64, val: i64) -> i64 {
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
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let py_val = olive_to_py(val);
        let res = PY_OBJECT_SET_ATTR_STRING(unwrapped_obj, (attr & !1) as *const c_char, py_val);
        PY_DEC_REF(py_val);
        if res == -1 {
            if let Some(err_msg) = catch_py_exception_msg() {
                PY_GILSTATE_RELEASE(gil);
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }
        PY_GILSTATE_RELEASE(gil);
        crate::result::olive_result_ok(obj as i64)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getitem_safe(obj: PyObject, key: PyObject) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    let unwrapped_key = unsafe { olive_py_unwrap(key) };
    if unwrapped_obj.is_null() || unwrapped_key.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null object or key pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_OBJECT_GET_ITEM(unwrapped_obj, unwrapped_key);
        if r.is_null() {
            if let Some(err_msg) = catch_py_exception_msg() {
                PY_GILSTATE_RELEASE(gil);
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }
        PY_GILSTATE_RELEASE(gil);
        let wrapped = olive_py_wrap_owned(r);
        crate::result::olive_result_ok(wrapped as i64)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setitem_safe(obj: PyObject, key: PyObject, val: PyObject) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    let unwrapped_key = unsafe { olive_py_unwrap(key) };
    let unwrapped_val = unsafe { olive_py_unwrap(val) };
    if unwrapped_obj.is_null() || unwrapped_key.is_null() || unwrapped_val.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null object, key, or value pointer");
        return crate::result::olive_result_err(err_str_ptr);
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let res = PY_OBJECT_SET_ITEM(unwrapped_obj, unwrapped_key, unwrapped_val);
        if res == -1 {
            if let Some(err_msg) = catch_py_exception_msg() {
                PY_GILSTATE_RELEASE(gil);
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
        }
        PY_GILSTATE_RELEASE(gil);
        crate::result::olive_result_ok(obj as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_copy_proxy_and_safe_boundaries() {
        olive_py_initialize();

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
            println!("test: list_ptr={}", list_ptr);
            let sv = &mut *(list_ptr as *mut crate::StableVec);
            *sv.ptr.add(0) = crate::olive_str_internal("hello");
            *sv.ptr.add(1) = crate::olive_str_internal("world");

            let gil = PY_GILSTATE_ENSURE();
            let py_proxy = olive_to_py(list_ptr);
            assert!(!py_proxy.is_null());

            let sys_mod = PY_IMPORT_IMPORT_MODULE(b"sys\0".as_ptr() as *const c_char);
            assert!(!sys_mod.is_null());

            let py_len = PY_OBJECT_LENGTH(py_proxy) as i64;
            if py_len == -1 {
                let err = crate::python::PY_ERR_OCCURRED();
                if !err.is_null() {
                    let err_str = catch_py_exception_msg();
                    println!("Python error in len(): {:?}", err_str);
                }
            }
            assert_eq!(py_len, 2);

            let hello_ptr = crate::olive_list_get(list_ptr, 0);

            let idx_0 = PY_LONG_FROM_LONG(0);
            let val_0 = PY_UNICODE_FROM_STRING(b"world\0".as_ptr() as *const c_char);
            let setitem_res = PY_OBJECT_SET_ITEM(py_proxy, idx_0, val_0);
            assert_ne!(setitem_res, -1);
            PY_DEC_REF(idx_0);
            PY_DEC_REF(val_0);

            let olive_val_0 = crate::olive_list_get(list_ptr, 0);
            assert_eq!(crate::olive_str_from_ptr(olive_val_0), "world");

            let val_insert_olive = crate::olive_str_internal("inserted");
            // We use C-API to insert by calling olive list functions directly for now
            crate::olive_list_insert(list_ptr, 1, val_insert_olive);

            assert_eq!(crate::olive_list_len(list_ptr), 3);
            let val_at_1 = crate::olive_list_get(list_ptr, 1);
            assert_eq!(crate::olive_str_from_ptr(val_at_1), "inserted");

            let idx_to_del = PY_LONG_FROM_LONG(1);
            let del_res = PY_OBJECT_DEL_ITEM(py_proxy, idx_to_del);
            if del_res == -1 {
                let err = crate::python::PY_ERR_OCCURRED();
                if !err.is_null() {
                    let err_str = catch_py_exception_msg();
                    println!("Python error in delitem: {:?}", err_str);
                }
            }
            assert_ne!(del_res, -1);
            PY_DEC_REF(idx_to_del);

            assert_eq!(crate::olive_list_len(list_ptr), 2);
            assert_ne!(
                crate::olive_str_from_ptr(crate::olive_list_get(list_ptr, 1)),
                "inserted"
            );

            let dict_ptr = crate::olive_obj_new();
            let py_dict = olive_to_py(dict_ptr);

            let dict_key = PY_UNICODE_FROM_STRING(b"testkey\0".as_ptr() as *const c_char);
            let dict_val = PY_LONG_FROM_LONG(9876);
            assert_ne!(PY_OBJECT_SET_ITEM(py_dict, dict_key, dict_val), -1);
            PY_DEC_REF(dict_key);
            PY_DEC_REF(dict_val);

            assert_eq!(
                crate::olive_obj_get(dict_ptr, crate::olive_str_internal("testkey")),
                9876
            );

            let dict_del_key = PY_UNICODE_FROM_STRING(b"testkey\0".as_ptr() as *const c_char);
            let dict_del_res = PY_OBJECT_DEL_ITEM(py_dict, dict_del_key);
            assert_ne!(dict_del_res, -1);
            PY_DEC_REF(dict_del_key);

            assert_eq!(
                crate::olive_obj_get(dict_ptr, crate::olive_str_internal("testkey")),
                0
            );

            PY_DEC_REF(py_dict);
            crate::olive_free_obj(dict_ptr);

            PY_DEC_REF(sys_mod);
            PY_DEC_REF(py_proxy);
            PY_GILSTATE_RELEASE(gil);

            crate::olive_free_any(hello_ptr);
            crate::olive_free_list(list_ptr);
        }
    }

    #[test]
    fn test_interop_leak_prevention() {
        olive_py_initialize();
        let mut allocated_ptrs = Vec::new();

        for i in 0..100 {
            let py_num = olive_py_from_int(i);
            assert_eq!(olive_py_to_int(py_num), i);
            allocated_ptrs.push(py_num as i64);

            let py_dict = unsafe {
                let gil = PY_GILSTATE_ENSURE();
                let d = PY_DICT_NEW();
                PY_GILSTATE_RELEASE(gil);
                olive_py_wrap_owned(d)
            };
            allocated_ptrs.push(py_dict as i64);

            olive_py_decref(py_num);
            olive_py_decref(py_dict);
        }

        // Verify that none of our allocated pointers remain in the active object registry
        for ptr in allocated_ptrs {
            assert!(
                olive_py_is_valid_proxy(ptr) == 0,
                "Active object (ptr={:#x}) leaked in interop wrapping!",
                ptr
            );
        }
    }
}
