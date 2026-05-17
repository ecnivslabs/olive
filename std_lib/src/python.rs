use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_long, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

type PyObject = *mut c_void;

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static mut LIBPYTHON: *mut c_void = std::ptr::null_mut();

// Function pointers
static mut Py_Initialize: unsafe extern "C" fn() = noop_initialize;
static mut Py_Finalize: unsafe extern "C" fn() = noop_finalize;
static mut PyImport_ImportModule: unsafe extern "C" fn(*const c_char) -> PyObject = noop_import;
static mut PyObject_GetAttrString: unsafe extern "C" fn(PyObject, *const c_char) -> PyObject = noop_getattr;
static mut PyObject_CallObject: unsafe extern "C" fn(PyObject, PyObject) -> PyObject = noop_call;
static mut PyObject_Call: unsafe extern "C" fn(PyObject, PyObject, PyObject) -> PyObject = noop_call_kw;
static mut Py_DecRef: unsafe extern "C" fn(PyObject) = noop_decref;
static mut PyLong_AsLong: unsafe extern "C" fn(PyObject) -> c_long = noop_as_long;
static mut PyFloat_AsDouble: unsafe extern "C" fn(PyObject) -> c_double = noop_as_double;
static mut PyUnicode_AsUTF8: unsafe extern "C" fn(PyObject) -> *const c_char = noop_as_utf8;
static mut PyLong_FromLong: unsafe extern "C" fn(c_long) -> PyObject = noop_from_long;
static mut PyFloat_FromDouble: unsafe extern "C" fn(c_double) -> PyObject = noop_from_double;
static mut PyUnicode_FromString: unsafe extern "C" fn(*const c_char) -> PyObject = noop_from_string;
static mut PyList_New: unsafe extern "C" fn(isize) -> PyObject = noop_list_new;
static mut PyList_SetItem: unsafe extern "C" fn(PyObject, isize, PyObject) -> c_int = noop_list_setitem;
static mut PyObject_GetItem: unsafe extern "C" fn(PyObject, PyObject) -> PyObject = noop_getitem;
static mut PyObject_SetItem: unsafe extern "C" fn(PyObject, PyObject, PyObject) -> c_int = noop_setitem;
static mut PyObject_Length: unsafe extern "C" fn(PyObject) -> isize = noop_length;
static mut PyGILState_Ensure: unsafe extern "C" fn() -> c_int = noop_gil_ensure;
static mut PyGILState_Release: unsafe extern "C" fn(c_int) = noop_gil_release;
static mut PyTuple_New: unsafe extern "C" fn(isize) -> PyObject = noop_tuple_new;
static mut PyTuple_SetItem: unsafe extern "C" fn(PyObject, isize, PyObject) -> c_int = noop_tuple_setitem;

static mut _Py_NoneStruct: *mut c_void = std::ptr::null_mut();
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

unsafe fn load_sym<T>(handle: *mut c_void, name: &str) -> T {
    let cname = CString::new(name).unwrap();
    let sym = libc::dlsym(handle, cname.as_ptr());
    std::mem::transmute_copy(&sym)
}

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

        Py_Initialize = load_sym(handle, "Py_Initialize");
        Py_Finalize = load_sym(handle, "Py_Finalize");
        PyImport_ImportModule = load_sym(handle, "PyImport_ImportModule");
        PyObject_GetAttrString = load_sym(handle, "PyObject_GetAttrString");
        PyObject_CallObject = load_sym(handle, "PyObject_CallObject");
        PyObject_Call = load_sym(handle, "PyObject_Call");
        Py_DecRef = load_sym(handle, "Py_DecRef");
        PyLong_AsLong = load_sym(handle, "PyLong_AsLong");
        PyFloat_AsDouble = load_sym(handle, "PyFloat_AsDouble");
        PyUnicode_AsUTF8 = load_sym(handle, "PyUnicode_AsUTF8");
        PyLong_FromLong = load_sym(handle, "PyLong_FromLong");
        PyFloat_FromDouble = load_sym(handle, "PyFloat_FromDouble");
        PyUnicode_FromString = load_sym(handle, "PyUnicode_FromString");
        PyList_New = load_sym(handle, "PyList_New");
        PyList_SetItem = load_sym(handle, "PyList_SetItem");
        PyObject_GetItem = load_sym(handle, "PyObject_GetItem");
        PyObject_SetItem = load_sym(handle, "PyObject_SetItem");
        PyObject_Length = load_sym(handle, "PyObject_Length");
        PyGILState_Ensure = load_sym(handle, "PyGILState_Ensure");
        PyGILState_Release = load_sym(handle, "PyGILState_Release");
        PyTuple_New = load_sym(handle, "PyTuple_New");
        PyTuple_SetItem = load_sym(handle, "PyTuple_SetItem");
        PY_ERR_PRINT = load_sym(handle, "PyErr_Print");

        _Py_NoneStruct = libc::dlsym(handle, b"_Py_NoneStruct\0".as_ptr() as _) as *mut c_void;

        Py_Initialize();
        println!("Python loaded!");
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_finalize() {
    unsafe {
        Py_Finalize();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_import(name: i64) -> PyObject {
    olive_py_initialize();
    unsafe {
        let gil = PyGILState_Ensure();
        let m = PyImport_ImportModule((name & !1) as *const c_char);
        if m.is_null() { PY_ERR_PRINT(); }
        PyGILState_Release(gil);
        m
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getattr(obj: PyObject, attr: i64) -> PyObject {
    if obj.is_null() { return std::ptr::null_mut(); }
    unsafe {
        let gil = PyGILState_Ensure();
        let a = PyObject_GetAttrString(obj, (attr & !1) as *const c_char);
        if a.is_null() { PY_ERR_PRINT(); }
        PyGILState_Release(gil);
        a
    }
}

fn olive_to_py(val: i64) -> PyObject {
    // Basic conversion logic.
    // If it's a string
    if val & 1 != 0 {
        let s = crate::olive_str_from_ptr(val);
        let c = CString::new(s).unwrap();
        unsafe { PyUnicode_FromString(c.as_ptr()) }
    } else if val == 0 {
        unsafe { _Py_NoneStruct }
    } else {
        unsafe { PyLong_FromLong(val as c_long) }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call(func: PyObject, args_list: i64) -> PyObject {
    if func.is_null() { return std::ptr::null_mut(); }
    unsafe {
        let gil = PyGILState_Ensure();
        
        let mut py_args = std::ptr::null_mut();
        if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            py_args = PyTuple_New(sv.len as isize);
            for i in 0..sv.len {
                let v = *sv.ptr.add(i);
                let py_v = olive_to_py(v);
                PyTuple_SetItem(py_args, i as isize, py_v);
            }
        }
        
        let res = PyObject_CallObject(func, py_args);
        if res.is_null() {
            PY_ERR_PRINT();
        }
        if !py_args.is_null() {
            Py_DecRef(py_args);
        }
        PyGILState_Release(gil);
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
            let gil = PyGILState_Ensure();
            Py_DecRef(obj);
            PyGILState_Release(gil);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_int(obj: PyObject) -> i64 {
    if obj.is_null() { return 0; }
    unsafe {
        let gil = PyGILState_Ensure();
        let v = PyLong_AsLong(obj) as i64;
        PyGILState_Release(gil);
        v
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_float(obj: PyObject) -> f64 {
    if obj.is_null() { return 0.0; }
    unsafe {
        let gil = PyGILState_Ensure();
        let v = PyFloat_AsDouble(obj) as f64;
        PyGILState_Release(gil);
        v
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_str(obj: PyObject) -> i64 {
    if obj.is_null() { return 0; }
    unsafe {
        let gil = PyGILState_Ensure();
        let s = PyUnicode_AsUTF8(obj);
        let res = if !s.is_null() {
            let r_str = CStr::from_ptr(s).to_string_lossy();
            crate::olive_str_internal(&r_str)
        } else {
            0
        };
        PyGILState_Release(gil);
        res
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_int(v: i64) -> PyObject {
    unsafe {
        let gil = PyGILState_Ensure();
        let r = PyLong_FromLong(v as c_long);
        PyGILState_Release(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_float(v: f64) -> PyObject {
    unsafe {
        let gil = PyGILState_Ensure();
        let r = PyFloat_FromDouble(v as c_double);
        PyGILState_Release(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_str(s: i64) -> PyObject {
    let r_str = crate::olive_str_from_ptr(s);
    let c = CString::new(r_str).unwrap();
    unsafe {
        let gil = PyGILState_Ensure();
        let r = PyUnicode_FromString(c.as_ptr());
        PyGILState_Release(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_list(s: i64) -> PyObject {
    if s == 0 { return std::ptr::null_mut(); }
    unsafe {
        let sv = &*(s as *const crate::StableVec);
        let gil = PyGILState_Ensure();
        let pyl = PyList_New(sv.len as isize);
        for i in 0..sv.len {
            let v = *sv.ptr.add(i);
            let py_v = olive_to_py(v);
            PyList_SetItem(pyl, i as isize, py_v);
        }
        PyGILState_Release(gil);
        pyl
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getitem(obj: PyObject, key: PyObject) -> PyObject {
    if obj.is_null() || key.is_null() { return std::ptr::null_mut(); }
    unsafe {
        let gil = PyGILState_Ensure();
        let r = PyObject_GetItem(obj, key);
        PyGILState_Release(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setitem(obj: PyObject, key: PyObject, val: PyObject) {
    if obj.is_null() || key.is_null() || val.is_null() { return; }
    unsafe {
        let gil = PyGILState_Ensure();
        PyObject_SetItem(obj, key, val);
        PyGILState_Release(gil);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_len(obj: PyObject) -> i64 {
    if obj.is_null() { return 0; }
    unsafe {
        let gil = PyGILState_Ensure();
        let r = PyObject_Length(obj) as i64;
        PyGILState_Release(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_none() -> PyObject {
    olive_py_initialize();
    unsafe { _Py_NoneStruct }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_is_none(obj: PyObject) -> i64 {
    olive_py_initialize();
    if obj == unsafe { _Py_NoneStruct } { 1 } else { 0 }
}
