use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_long, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

type PyObject = *mut c_void;

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static mut LIBPYTHON: *mut c_void = std::ptr::null_mut();

// Function pointers
static mut PY_INITIALIZE: unsafe extern "C" fn() = noop_initialize;
static mut PY_FINALIZE: unsafe extern "C" fn() = noop_finalize;
static mut PY_IMPORT_IMPORT_MODULE: unsafe extern "C" fn(*const c_char) -> PyObject = noop_import;
static mut PY_OBJECT_GET_ATTR_STRING: unsafe extern "C" fn(PyObject, *const c_char) -> PyObject = noop_getattr;
static mut PY_OBJECT_CALL_OBJECT: unsafe extern "C" fn(PyObject, PyObject) -> PyObject = noop_call;
static mut PY_OBJECT_CALL: unsafe extern "C" fn(PyObject, PyObject, PyObject) -> PyObject = noop_call_kw;
static mut PY_DEC_REF: unsafe extern "C" fn(PyObject) = noop_decref;
static mut PY_LONG_AS_LONG: unsafe extern "C" fn(PyObject) -> c_long = noop_as_long;
static mut PY_FLOAT_AS_DOUBLE: unsafe extern "C" fn(PyObject) -> c_double = noop_as_double;
static mut PY_UNICODE_AS_UTF8: unsafe extern "C" fn(PyObject) -> *const c_char = noop_as_utf8;
static mut PY_LONG_FROM_LONG: unsafe extern "C" fn(c_long) -> PyObject = noop_from_long;
static mut PY_FLOAT_FROM_DOUBLE: unsafe extern "C" fn(c_double) -> PyObject = noop_from_double;
static mut PY_UNICODE_FROM_STRING: unsafe extern "C" fn(*const c_char) -> PyObject = noop_from_string;
static mut PY_LIST_NEW: unsafe extern "C" fn(isize) -> PyObject = noop_list_new;
static mut PY_LIST_SET_ITEM: unsafe extern "C" fn(PyObject, isize, PyObject) -> c_int = noop_list_setitem;
static mut PY_OBJECT_GET_ITEM: unsafe extern "C" fn(PyObject, PyObject) -> PyObject = noop_getitem;
static mut PY_OBJECT_SET_ITEM: unsafe extern "C" fn(PyObject, PyObject, PyObject) -> c_int = noop_setitem;
static mut PY_OBJECT_LENGTH: unsafe extern "C" fn(PyObject) -> isize = noop_length;
static mut PY_GILSTATE_ENSURE: unsafe extern "C" fn() -> c_int = noop_gil_ensure;
static mut PY_GILSTATE_RELEASE: unsafe extern "C" fn(c_int) = noop_gil_release;
static mut PY_TUPLE_NEW: unsafe extern "C" fn(isize) -> PyObject = noop_tuple_new;
static mut PY_TUPLE_SET_ITEM: unsafe extern "C" fn(PyObject, isize, PyObject) -> c_int = noop_tuple_setitem;

static mut _PY_NONE_STRUCT: *mut c_void = std::ptr::null_mut();
static mut PY_ERR_PRINT: unsafe extern "C" fn() = noop_err_print;

unsafe extern "C" fn noop_err_print() {}
unsafe extern "C" fn noop_initialize() {}
unsafe extern "C" fn noop_finalize() {}
unsafe extern "C" fn noop_import(_: *const c_char) -> PyObject { std::ptr::null_mut() }
unsafe extern "C" fn noop_getattr(_: PyObject, _: *const c_char) -> PyObject { std::ptr::null_mut() }
unsafe extern "C" fn noop_call(_: PyObject, _: PyObject) -> PyObject { std::ptr::null_mut() }
unsafe extern "C" fn noop_call_kw(_: PyObject, _: PyObject, _: PyObject) -> PyObject { std::ptr::null_mut() }
unsafe extern "C" fn noop_decref(_: PyObject) {}
unsafe extern "C" fn noop_as_long(_: PyObject) -> c_long { 0 }
unsafe extern "C" fn noop_as_double(_: PyObject) -> c_double { 0.0 }
unsafe extern "C" fn noop_as_utf8(_: PyObject) -> *const c_char { b"\0".as_ptr() as _ }
unsafe extern "C" fn noop_from_long(_: c_long) -> PyObject { std::ptr::null_mut() }
unsafe extern "C" fn noop_from_double(_: c_double) -> PyObject { std::ptr::null_mut() }
unsafe extern "C" fn noop_from_string(_: *const c_char) -> PyObject { std::ptr::null_mut() }
unsafe extern "C" fn noop_list_new(_: isize) -> PyObject { std::ptr::null_mut() }
unsafe extern "C" fn noop_list_setitem(_: PyObject, _: isize, _: PyObject) -> c_int { -1 }
unsafe extern "C" fn noop_getitem(_: PyObject, _: PyObject) -> PyObject { std::ptr::null_mut() }
unsafe extern "C" fn noop_setitem(_: PyObject, _: PyObject, _: PyObject) -> c_int { -1 }
unsafe extern "C" fn noop_length(_: PyObject) -> isize { 0 }
unsafe extern "C" fn noop_gil_ensure() -> c_int { 0 }
unsafe extern "C" fn noop_gil_release(_: c_int) {}
unsafe extern "C" fn noop_tuple_new(_: isize) -> PyObject { std::ptr::null_mut() }
unsafe extern "C" fn noop_tuple_setitem(_: PyObject, _: isize, _: PyObject) -> c_int { -1 }

unsafe fn load_sym<T>(handle: *mut c_void, name: &str) -> T { unsafe {
    let cname = CString::new(name).unwrap();
    let sym = libc::dlsym(handle, cname.as_ptr());
    std::mem::transmute_copy(&sym)
}}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_initialize() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        let mut handle = libc::dlopen(b"libpython3.so\0".as_ptr() as _, libc::RTLD_NOW | libc::RTLD_GLOBAL);
        if handle.is_null() {
            handle = libc::dlopen(b"libpython3.11.so\0".as_ptr() as _, libc::RTLD_NOW | libc::RTLD_GLOBAL);
        }
        if handle.is_null() {
            handle = libc::dlopen(b"libpython3.10.so\0".as_ptr() as _, libc::RTLD_NOW | libc::RTLD_GLOBAL);
        }
        if handle.is_null() {
            eprintln!("Warning: could not load libpython3. Python interop will not work.");
            return;
        }
        LIBPYTHON = handle;

        PY_INITIALIZE = load_sym(handle, "Py_Initialize");
        PY_FINALIZE = load_sym(handle, "Py_Finalize");
        PY_IMPORT_IMPORT_MODULE = load_sym(handle, "PyImport_ImportModule");
        PY_OBJECT_GET_ATTR_STRING = load_sym(handle, "PyObject_GetAttrString");
        PY_OBJECT_CALL_OBJECT = load_sym(handle, "PyObject_CallObject");
        PY_OBJECT_CALL = load_sym(handle, "PyObject_Call");
        PY_DEC_REF = load_sym(handle, "Py_DecRef");
        PY_LONG_AS_LONG = load_sym(handle, "PyLong_AsLong");
        PY_FLOAT_AS_DOUBLE = load_sym(handle, "PyFloat_AsDouble");
        PY_UNICODE_AS_UTF8 = load_sym(handle, "PyUnicode_AsUTF8");
        PY_LONG_FROM_LONG = load_sym(handle, "PyLong_FromLong");
        PY_FLOAT_FROM_DOUBLE = load_sym(handle, "PyFloat_FromDouble");
        PY_UNICODE_FROM_STRING = load_sym(handle, "PyUnicode_FromString");
        PY_LIST_NEW = load_sym(handle, "PyList_New");
        PY_LIST_SET_ITEM = load_sym(handle, "PyList_SetItem");
        PY_OBJECT_GET_ITEM = load_sym(handle, "PyObject_GetItem");
        PY_OBJECT_SET_ITEM = load_sym(handle, "PyObject_SetItem");
        PY_OBJECT_LENGTH = load_sym(handle, "PyObject_Length");
        PY_GILSTATE_ENSURE = load_sym(handle, "PyGILState_Ensure");
        PY_GILSTATE_RELEASE = load_sym(handle, "PyGILState_Release");
        PY_TUPLE_NEW = load_sym(handle, "PyTuple_New");
        PY_TUPLE_SET_ITEM = load_sym(handle, "PyTuple_SetItem");
        PY_ERR_PRINT = load_sym(handle, "PyErr_Print");

        _PY_NONE_STRUCT = libc::dlsym(handle, b"_Py_NoneStruct\0".as_ptr() as _) as *mut c_void;

        PY_INITIALIZE();
        println!("Python loaded!");
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_finalize() {
    unsafe {
        PY_FINALIZE();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_import(name: i64) -> PyObject {
    olive_py_initialize();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let m = PY_IMPORT_IMPORT_MODULE((name & !1) as *const c_char);
        if m.is_null() { PY_ERR_PRINT(); }
        PY_GILSTATE_RELEASE(gil);
        m
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getattr(obj: PyObject, attr: i64) -> PyObject {
    if obj.is_null() { return std::ptr::null_mut(); }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let a = PY_OBJECT_GET_ATTR_STRING(obj, (attr & !1) as *const c_char);
        if a.is_null() { PY_ERR_PRINT(); }
        PY_GILSTATE_RELEASE(gil);
        a
    }
}

fn olive_to_py(val: i64) -> PyObject {
    // Basic conversion logic.
    // If it's a string
    if val & 1 != 0 {
        let s = crate::olive_str_from_ptr(val);
        let c = CString::new(s).unwrap();
        unsafe { PY_UNICODE_FROM_STRING(c.as_ptr()) }
    } else if val == 0 {
        unsafe { _PY_NONE_STRUCT }
    } else {
        unsafe { PY_LONG_FROM_LONG(val as c_long) }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call(func: PyObject, args_list: i64) -> PyObject {
    if func.is_null() { return std::ptr::null_mut(); }
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
        
        let res = PY_OBJECT_CALL_OBJECT(func, py_args);
        if res.is_null() {
            PY_ERR_PRINT();
        }
        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }
        PY_GILSTATE_RELEASE(gil);
        res
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_kw(func: PyObject, args_list: i64, _kwargs: i64) -> PyObject {
    olive_py_call(func, args_list) // Simplify kw calls for now
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_decref(obj: PyObject) {
    if !obj.is_null() {
        unsafe {
            let gil = PY_GILSTATE_ENSURE();
            PY_DEC_REF(obj);
            PY_GILSTATE_RELEASE(gil);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_int(obj: PyObject) -> i64 {
    if obj.is_null() { return 0; }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let v = PY_LONG_AS_LONG(obj) as i64;
        PY_GILSTATE_RELEASE(gil);
        v
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_float(obj: PyObject) -> f64 {
    if obj.is_null() { return 0.0; }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let v = PY_FLOAT_AS_DOUBLE(obj) as f64;
        PY_GILSTATE_RELEASE(gil);
        v
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_str(obj: PyObject) -> i64 {
    if obj.is_null() { return 0; }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let s = PY_UNICODE_AS_UTF8(obj);
        let res = if !s.is_null() {
            let r_str = CStr::from_ptr(s).to_string_lossy();
            crate::olive_str_internal(&r_str)
        } else {
            0
        };
        PY_GILSTATE_RELEASE(gil);
        res
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_int(v: i64) -> PyObject {
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_LONG_FROM_LONG(v as c_long);
        PY_GILSTATE_RELEASE(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_float(v: f64) -> PyObject {
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_FLOAT_FROM_DOUBLE(v as c_double);
        PY_GILSTATE_RELEASE(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_str(s: i64) -> PyObject {
    let r_str = crate::olive_str_from_ptr(s);
    let c = CString::new(r_str).unwrap();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_UNICODE_FROM_STRING(c.as_ptr());
        PY_GILSTATE_RELEASE(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_list(s: i64) -> PyObject {
    if s == 0 { return std::ptr::null_mut(); }
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
        pyl
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getitem(obj: PyObject, key: PyObject) -> PyObject {
    if obj.is_null() || key.is_null() { return std::ptr::null_mut(); }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_OBJECT_GET_ITEM(obj, key);
        PY_GILSTATE_RELEASE(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setitem(obj: PyObject, key: PyObject, val: PyObject) {
    if obj.is_null() || key.is_null() || val.is_null() { return; }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        PY_OBJECT_SET_ITEM(obj, key, val);
        PY_GILSTATE_RELEASE(gil);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_len(obj: PyObject) -> i64 {
    if obj.is_null() { return 0; }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_OBJECT_LENGTH(obj) as i64;
        PY_GILSTATE_RELEASE(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_none() -> PyObject {
    olive_py_initialize();
    unsafe { _PY_NONE_STRUCT }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_is_none(obj: PyObject) -> i64 {
    olive_py_initialize();
    if obj == unsafe { _PY_NONE_STRUCT } { 1 } else { 0 }
}
