#![allow(unsafe_op_in_unsafe_fn)]
use crate::python::{PyObject, olive_any_to_py, olive_py_conv_to_olive, olive_py_is_valid_proxy};
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

#[repr(C)]
pub struct PyMethodDef {
    pub ml_name: *const c_char,
    pub ml_meth: *mut c_void,
    pub ml_flags: c_int,
    pub ml_doc: *const c_char,
}

const METH_VARARGS: c_int = 0x0001;
const METH_NOARGS: c_int = 0x0004;

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

    let mut list_slots = vec![
        PyType_Slot {
            slot: 45,
            pfunc: list_proxy_len as *mut c_void,
        },
        PyType_Slot {
            slot: 44,
            pfunc: list_proxy_getitem as *mut c_void,
        },
        PyType_Slot {
            slot: 39,
            pfunc: list_proxy_setitem as *mut c_void,
        },
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
        flags: (1 << 0) | (1 << 10),
        slots: list_slots.as_mut_ptr(),
    };
    OLIVE_LIST_PROXY_TYPE = PY_TYPE_FROM_SPEC(&mut list_spec);

    // Leaked: the type holds a pointer to this for the process lifetime.
    let methods: &'static mut [PyMethodDef] = Box::leak(Box::new([
        PyMethodDef {
            ml_name: c"get".as_ptr(),
            ml_meth: dict_proxy_get as *mut c_void,
            ml_flags: METH_VARARGS,
            ml_doc: std::ptr::null(),
        },
        PyMethodDef {
            ml_name: c"setdefault".as_ptr(),
            ml_meth: dict_proxy_setdefault as *mut c_void,
            ml_flags: METH_VARARGS,
            ml_doc: std::ptr::null(),
        },
        PyMethodDef {
            ml_name: c"pop".as_ptr(),
            ml_meth: dict_proxy_pop as *mut c_void,
            ml_flags: METH_VARARGS,
            ml_doc: std::ptr::null(),
        },
        PyMethodDef {
            ml_name: c"keys".as_ptr(),
            ml_meth: dict_proxy_keys as *mut c_void,
            ml_flags: METH_NOARGS,
            ml_doc: std::ptr::null(),
        },
        PyMethodDef {
            ml_name: c"values".as_ptr(),
            ml_meth: dict_proxy_values as *mut c_void,
            ml_flags: METH_NOARGS,
            ml_doc: std::ptr::null(),
        },
        PyMethodDef {
            ml_name: c"items".as_ptr(),
            ml_meth: dict_proxy_items as *mut c_void,
            ml_flags: METH_NOARGS,
            ml_doc: std::ptr::null(),
        },
        PyMethodDef {
            ml_name: std::ptr::null(),
            ml_meth: std::ptr::null_mut(),
            ml_flags: 0,
            ml_doc: std::ptr::null(),
        },
    ]));

    let mut dict_slots = vec![
        PyType_Slot {
            slot: 4,
            pfunc: dict_proxy_len as *mut c_void,
        },
        PyType_Slot {
            slot: 5,
            pfunc: dict_proxy_getitem as *mut c_void,
        },
        PyType_Slot {
            slot: 3,
            pfunc: dict_proxy_setitem as *mut c_void,
        },
        // Py_sq_contains: `key in d`.
        PyType_Slot {
            slot: 41,
            pfunc: dict_proxy_contains as *mut c_void,
        },
        // Py_tp_methods: named methods (get/setdefault/pop/keys/values/items).
        PyType_Slot {
            slot: 64,
            pfunc: methods.as_mut_ptr() as *mut c_void,
        },
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

    crate::olive_list_len(proxy.ptr) as isize
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
    let kind = unsafe { *(proxy.ptr as *const i64) };
    if kind == crate::KIND_ANY_LIST {
        olive_any_to_py(val)
    } else {
        crate::python::python_coerce::olive_to_py(val)
    }
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

    olive_any_to_py(val)
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

/// Converts a Python key object to a tagged Olive string-key pointer, matching
/// how the proxy stores keys (`key_ptr | 1`).
unsafe fn proxy_key(key: PyObject) -> Option<i64> {
    let key_str_obj = crate::python::PY_OBJECT_STR(key);
    if key_str_obj.is_null() {
        return None;
    }
    let key_utf8 = crate::python::PY_UNICODE_AS_UTF8(key_str_obj);
    if key_utf8.is_null() {
        crate::python::PY_DEC_REF(key_str_obj);
        return None;
    }
    let key_str = CStr::from_ptr(key_utf8).to_string_lossy();
    let key_ptr = crate::olive_str_internal(&key_str);
    crate::python::PY_DEC_REF(key_str_obj);
    Some(key_ptr | 1)
}

unsafe fn none_ref() -> PyObject {
    let none = crate::python::_PY_NONE_STRUCT as PyObject;
    crate::python::PY_INC_REF(none);
    none
}

/// `d.get(key, default=None)`.
unsafe extern "C" fn dict_proxy_get(self_ptr: PyObject, args: PyObject) -> PyObject {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return std::ptr::null_mut();
    }
    let argc = crate::python::PY_TUPLE_SIZE(args);
    if argc < 1 {
        let msg = CString::new("get expected at least 1 argument").unwrap();
        PY_ERR_SET_STRING(PY_EXC_RUNTIMEERROR, msg.as_ptr());
        return std::ptr::null_mut();
    }
    let key = crate::python::PY_TUPLE_GET_ITEM(args, 0);
    let Some(key_tagged) = proxy_key(key) else {
        return std::ptr::null_mut();
    };
    let val = crate::olive_obj_get(proxy.ptr, key_tagged);
    if val == 0 {
        if argc >= 2 {
            let default = crate::python::PY_TUPLE_GET_ITEM(args, 1);
            crate::python::PY_INC_REF(default);
            return default;
        }
        return none_ref();
    }
    olive_any_to_py(val)
}

/// `d.setdefault(key, default=None)`.
unsafe extern "C" fn dict_proxy_setdefault(self_ptr: PyObject, args: PyObject) -> PyObject {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return std::ptr::null_mut();
    }
    let argc = crate::python::PY_TUPLE_SIZE(args);
    if argc < 1 {
        let msg = CString::new("setdefault expected at least 1 argument").unwrap();
        PY_ERR_SET_STRING(PY_EXC_RUNTIMEERROR, msg.as_ptr());
        return std::ptr::null_mut();
    }
    let key = crate::python::PY_TUPLE_GET_ITEM(args, 0);
    let Some(key_tagged) = proxy_key(key) else {
        return std::ptr::null_mut();
    };
    let existing = crate::olive_obj_get(proxy.ptr, key_tagged);
    if existing != 0 {
        return olive_any_to_py(existing);
    }
    let default = if argc >= 2 {
        crate::python::PY_TUPLE_GET_ITEM(args, 1)
    } else {
        crate::python::_PY_NONE_STRUCT as PyObject
    };
    let val = olive_py_conv_to_olive(default);
    crate::olive_obj_set(proxy.ptr, key_tagged, val);
    crate::python::PY_INC_REF(default);
    default
}

/// `d.pop(key, default=...)`. Without a default, a missing key raises `KeyError`.
unsafe extern "C" fn dict_proxy_pop(self_ptr: PyObject, args: PyObject) -> PyObject {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return std::ptr::null_mut();
    }
    let argc = crate::python::PY_TUPLE_SIZE(args);
    if argc < 1 {
        let msg = CString::new("pop expected at least 1 argument").unwrap();
        PY_ERR_SET_STRING(PY_EXC_RUNTIMEERROR, msg.as_ptr());
        return std::ptr::null_mut();
    }
    let key = crate::python::PY_TUPLE_GET_ITEM(args, 0);
    let Some(key_tagged) = proxy_key(key) else {
        return std::ptr::null_mut();
    };
    let removed = crate::olive_obj_remove(proxy.ptr, key_tagged);
    if removed != 0 {
        return olive_any_to_py(removed);
    }
    if argc >= 2 {
        let default = crate::python::PY_TUPLE_GET_ITEM(args, 1);
        crate::python::PY_INC_REF(default);
        return default;
    }
    let msg = CString::new("KeyError").unwrap();
    PY_ERR_SET_STRING(PY_EXC_KEYERROR, msg.as_ptr());
    std::ptr::null_mut()
}

unsafe fn build_py_list(items: &[PyObject]) -> PyObject {
    let list = crate::python::PY_LIST_NEW(items.len() as isize);
    for (i, &item) in items.iter().enumerate() {
        crate::python::PY_LIST_SET_ITEM(list, i as isize, item);
    }
    list
}

/// `d.keys()` as a list.
unsafe extern "C" fn dict_proxy_keys(self_ptr: PyObject, _args: PyObject) -> PyObject {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return std::ptr::null_mut();
    }
    let keys = crate::olive_obj_keys(proxy.ptr);
    let n = crate::olive_list_len(keys);
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        out.push(olive_any_to_py(crate::olive_list_get(keys, i)));
    }
    build_py_list(&out)
}

/// `d.values()` as a list.
unsafe extern "C" fn dict_proxy_values(self_ptr: PyObject, _args: PyObject) -> PyObject {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return std::ptr::null_mut();
    }
    let keys = crate::olive_obj_keys(proxy.ptr);
    let n = crate::olive_list_len(keys);
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let val = crate::olive_obj_get(proxy.ptr, crate::olive_list_get(keys, i));
        out.push(olive_any_to_py(val));
    }
    build_py_list(&out)
}

/// `d.items()` as a list of `(key, value)` tuples.
unsafe extern "C" fn dict_proxy_items(self_ptr: PyObject, _args: PyObject) -> PyObject {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return std::ptr::null_mut();
    }
    let keys = crate::olive_obj_keys(proxy.ptr);
    let n = crate::olive_list_len(keys);
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let key = crate::olive_list_get(keys, i);
        let val = crate::olive_obj_get(proxy.ptr, key);
        let pair = crate::python::PY_TUPLE_NEW(2);
        crate::python::PY_TUPLE_SET_ITEM(pair, 0, olive_any_to_py(key));
        crate::python::PY_TUPLE_SET_ITEM(pair, 1, olive_any_to_py(val));
        out.push(pair);
    }
    build_py_list(&out)
}

/// `key in d`.
unsafe extern "C" fn dict_proxy_contains(self_ptr: PyObject, key: PyObject) -> c_int {
    let proxy = &*(self_ptr as *const NativeProxy);
    if !check_alive(proxy.ptr) {
        return -1;
    }
    let Some(key_tagged) = proxy_key(key) else {
        return -1;
    };
    (crate::olive_obj_get(proxy.ptr, key_tagged) != 0) as c_int
}
