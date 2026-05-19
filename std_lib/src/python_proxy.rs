#![allow(unsafe_op_in_unsafe_fn)]
use crate::python::{
    PyObject, olive_py_conv_to_olive, olive_py_conv_to_py, olive_py_is_valid_proxy,
};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_void};

#[repr(C)]
pub struct PyType_Slot {
    pub slot: c_int,
    pub pfunc: *mut c_void,
}

#[repr(C)]
pub struct PyType_Spec {
    pub name: *const c_char,
    pub basicsize: c_int,
    pub itemsize: c_int,
    pub flags: c_uint,
    pub slots: *mut PyType_Slot,
}

#[repr(C)]
pub struct NativeProxy {
    pub ob_refcnt: isize,
    pub ob_type: *mut c_void,
    pub ptr: i64,
}

pub static mut OLIVE_LIST_PROXY_TYPE: PyObject = std::ptr::null_mut();
pub static mut OLIVE_DICT_PROXY_TYPE: PyObject = std::ptr::null_mut();

pub static mut PY_TYPE_FROM_SPEC: unsafe extern "C" fn(*mut PyType_Spec) -> PyObject =
    noop_type_from_spec;
pub static mut PY_TYPE_GENERIC_ALLOC: unsafe extern "C" fn(PyObject, isize) -> PyObject =
    noop_alloc;

pub static mut PY_ERR_SET_STRING: unsafe extern "C" fn(PyObject, *const c_char) =
    noop_err_set_string;
pub static mut PY_EXC_RUNTIMEERROR: PyObject = std::ptr::null_mut();
pub static mut PY_EXC_INDEXERROR: PyObject = std::ptr::null_mut();
pub static mut PY_EXC_KEYERROR: PyObject = std::ptr::null_mut();

unsafe extern "C" fn noop_type_from_spec(_: *mut PyType_Spec) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_err_set_string(_: PyObject, _: *const c_char) {}
unsafe extern "C" fn noop_alloc(_: PyObject, _: isize) -> PyObject {
    std::ptr::null_mut()
}

pub unsafe fn setup_native_proxies(
    handle: *mut c_void,
    dlsym_fn: unsafe fn(*mut c_void, &str) -> *mut c_void,
) {
    PY_TYPE_FROM_SPEC = std::mem::transmute(dlsym_fn(handle, "PyType_FromSpec"));
    PY_ERR_SET_STRING = std::mem::transmute(dlsym_fn(handle, "PyErr_SetString"));
    PY_TYPE_GENERIC_ALLOC = std::mem::transmute(dlsym_fn(handle, "PyType_GenericAlloc"));

    let exc_re = dlsym_fn(handle, "PyExc_RuntimeError");
    if !exc_re.is_null() {
        PY_EXC_RUNTIMEERROR = *(exc_re as *mut PyObject);
    }

    let exc_ie = dlsym_fn(handle, "PyExc_IndexError");
    if !exc_ie.is_null() {
        PY_EXC_INDEXERROR = *(exc_ie as *mut PyObject);
    }

    let exc_ke = dlsym_fn(handle, "PyExc_KeyError");
    if !exc_ke.is_null() {
        PY_EXC_KEYERROR = *(exc_ke as *mut PyObject);
    }

    // Setup List Proxy
    let mut list_slots = vec![
        PyType_Slot {
            slot: 45,
            pfunc: list_proxy_len as *mut c_void,
        }, // Py_sq_length
        PyType_Slot {
            slot: 44,
            pfunc: list_proxy_getitem as *mut c_void,
        }, // Py_sq_item
        PyType_Slot {
            slot: 39,
            pfunc: list_proxy_setitem as *mut c_void,
        }, // Py_sq_ass_item
        PyType_Slot {
            slot: 0,
            pfunc: std::ptr::null_mut(),
        },
    ];
    let list_name = CString::new("olive_proxies.OliveListProxy").unwrap();
    let mut list_spec = PyType_Spec {
        name: list_name.as_ptr(),
        basicsize: std::mem::size_of::<NativeProxy>() as c_int,
        itemsize: 0,
        // Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE
        flags: (1 << 0) | (1 << 10),
        slots: list_slots.as_mut_ptr(),
    };
    OLIVE_LIST_PROXY_TYPE = PY_TYPE_FROM_SPEC(&mut list_spec);

    // Setup Dict Proxy
    let mut dict_slots = vec![
        PyType_Slot {
            slot: 4,
            pfunc: dict_proxy_len as *mut c_void,
        }, // Py_mp_length
        PyType_Slot {
            slot: 5,
            pfunc: dict_proxy_getitem as *mut c_void,
        }, // Py_mp_subscript
        PyType_Slot {
            slot: 3,
            pfunc: dict_proxy_setitem as *mut c_void,
        }, // Py_mp_ass_subscript
        PyType_Slot {
            slot: 0,
            pfunc: std::ptr::null_mut(),
        },
    ];
    let dict_name = CString::new("olive_proxies.OliveDictProxy").unwrap();
    let mut dict_spec = PyType_Spec {
        name: dict_name.as_ptr(),
        basicsize: std::mem::size_of::<NativeProxy>() as c_int,
        itemsize: 0,
        // Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE
        flags: (1 << 0) | (1 << 10),
        slots: dict_slots.as_mut_ptr(),
    };
    OLIVE_DICT_PROXY_TYPE = PY_TYPE_FROM_SPEC(&mut dict_spec);
}

unsafe fn check_alive(ptr: i64) -> bool {
    if olive_py_is_valid_proxy(ptr) == 0 {
        let msg = CString::new("Olive proxy: backing value was freed").unwrap();
        PY_ERR_SET_STRING(PY_EXC_RUNTIMEERROR, msg.as_ptr());
        false
    } else {
        true
    }
}

unsafe extern "C" fn list_proxy_len(self_ptr: PyObject) -> isize {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return -1;
    }
    let len = crate::olive_list_len(proxy.ptr) as isize;
    len
}

unsafe extern "C" fn list_proxy_getitem(self_ptr: PyObject, index: isize) -> PyObject {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return std::ptr::null_mut();
    }
    let length = crate::olive_list_len(proxy.ptr);
    let mut idx = index as i64;
    if idx < 0 {
        idx += length;
    }
    if idx < 0 || idx >= length {
        let msg = CString::new("list index out of range").unwrap();
        PY_ERR_SET_STRING(PY_EXC_INDEXERROR, msg.as_ptr());
        return std::ptr::null_mut();
    }
    let val = crate::olive_list_get(proxy.ptr, idx);
    let py_val = olive_py_conv_to_py(val);
    py_val
}

unsafe extern "C" fn list_proxy_setitem(
    self_ptr: PyObject,
    index: isize,
    value: PyObject,
) -> c_int {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return -1;
    }
    let length = crate::olive_list_len(proxy.ptr);
    let mut idx = index as i64;
    if idx < 0 {
        idx += length;
    }
    if idx < 0 || idx >= length {
        let msg = CString::new("list index out of range").unwrap();
        PY_ERR_SET_STRING(PY_EXC_INDEXERROR, msg.as_ptr());
        return -1;
    }
    if value.is_null() {
        crate::olive_list_remove(proxy.ptr, idx);
    } else {
        let val = olive_py_conv_to_olive(value);
        crate::olive_list_set(proxy.ptr, idx, val);
    }
    0
}

unsafe extern "C" fn dict_proxy_len(self_ptr: PyObject) -> isize {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return -1;
    }
    crate::olive_obj_len(proxy.ptr) as isize
}

unsafe extern "C" fn dict_proxy_getitem(self_ptr: PyObject, key: PyObject) -> PyObject {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return std::ptr::null_mut();
    }
    let key_str_obj = crate::python::PY_OBJECT_STR(key);
    if key_str_obj.is_null() {
        return std::ptr::null_mut();
    }
    let key_utf8 = crate::python::PY_UNICODE_AS_UTF8(key_str_obj);
    if key_utf8.is_null() {
        crate::python::PY_DEC_REF(key_str_obj);
        return std::ptr::null_mut();
    }
    let key_str = CStr::from_ptr(key_utf8).to_string_lossy();
    let key_ptr = crate::olive_str_internal(&key_str);
    crate::python::PY_DEC_REF(key_str_obj);

    let val = crate::olive_obj_get(proxy.ptr, key_ptr | 1);
    if val == 0 {
        let msg = CString::new("KeyError").unwrap();
        PY_ERR_SET_STRING(PY_EXC_KEYERROR, msg.as_ptr());
        return std::ptr::null_mut();
    }
    let py_val = olive_py_conv_to_py(val);
    py_val
}

unsafe extern "C" fn dict_proxy_setitem(
    self_ptr: PyObject,
    key: PyObject,
    value: PyObject,
) -> c_int {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return -1;
    }

    let key_str_obj = crate::python::PY_OBJECT_STR(key);
    if key_str_obj.is_null() {
        return -1;
    }
    let key_utf8 = crate::python::PY_UNICODE_AS_UTF8(key_str_obj);
    if key_utf8.is_null() {
        crate::python::PY_DEC_REF(key_str_obj);
        return -1;
    }
    let key_str = CStr::from_ptr(key_utf8).to_string_lossy();
    let key_ptr = crate::olive_str_internal(&key_str);
    crate::python::PY_DEC_REF(key_str_obj);

    if value.is_null() {
        let res = crate::olive_obj_remove(proxy.ptr, key_ptr | 1);
        if res == 0 {
            let msg = CString::new("KeyError").unwrap();
            PY_ERR_SET_STRING(PY_EXC_KEYERROR, msg.as_ptr());
            return -1;
        }
    } else {
        let val = olive_py_conv_to_olive(value);
        crate::olive_obj_set(proxy.ptr, key_ptr | 1, val);
    }
    0
}
