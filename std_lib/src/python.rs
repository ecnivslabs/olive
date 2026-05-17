use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_long, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

type PyObject = *mut c_void;

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
static mut PY_DEC_REF: unsafe extern "C" fn(PyObject) = noop_decref;
static mut PY_INC_REF: unsafe extern "C" fn(PyObject) = noop_decref;
static mut PY_LONG_AS_LONG: unsafe extern "C" fn(PyObject) -> c_long = noop_as_long;
static mut PY_FLOAT_AS_DOUBLE: unsafe extern "C" fn(PyObject) -> c_double = noop_as_double;
static mut PY_UNICODE_AS_UTF8: unsafe extern "C" fn(PyObject) -> *const c_char = noop_as_utf8;
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
static mut PY_OBJECT_STR: unsafe extern "C" fn(PyObject) -> PyObject = noop_call_1;
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

unsafe extern "C" fn noop_set_add(_: PyObject, _: PyObject) -> c_int {
    -1
}
unsafe extern "C" fn noop_bytes_from_string(_: *const u8, _: isize) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_bytes_as_string(_: PyObject) -> *const c_char {
    std::ptr::null()
}
unsafe extern "C" fn noop_bytes_size(_: PyObject) -> isize {
    0
}

static mut PY_BOOL_TYPE: PyObject = std::ptr::null_mut();
static mut PY_LONG_TYPE: PyObject = std::ptr::null_mut();
static mut PY_FLOAT_TYPE: PyObject = std::ptr::null_mut();
static mut PY_UNICODE_TYPE: PyObject = std::ptr::null_mut();
static mut PY_LIST_TYPE: PyObject = std::ptr::null_mut();
static mut PY_DICT_TYPE: PyObject = std::ptr::null_mut();
static mut PY_SET_TYPE: PyObject = std::ptr::null_mut();
static mut PY_BYTES_TYPE: PyObject = std::ptr::null_mut();

static mut _PY_NONE_STRUCT: *mut c_void = std::ptr::null_mut();
static mut PY_ERR_PRINT: unsafe extern "C" fn() = noop_err_print;

static mut PY_TYPE_IS_SUBTYPE: unsafe extern "C" fn(PyObject, PyObject) -> c_int = noop_is_subtype;
static mut PY_EVAL_SAVE_THREAD: unsafe extern "C" fn() -> *mut c_void = noop_save_thread;
static mut PY_EVAL_RESTORE_THREAD: unsafe extern "C" fn(*mut c_void) = noop_restore_thread;
static mut PY_EVAL_INIT_THREADS: unsafe extern "C" fn() = noop_initialize;

static mut MAIN_THREAD_STATE: *mut c_void = std::ptr::null_mut();

unsafe extern "C" fn noop_is_subtype(_: PyObject, _: PyObject) -> c_int {
    0
}
unsafe extern "C" fn noop_save_thread() -> *mut c_void {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_restore_thread(_: *mut c_void) {}
unsafe extern "C" fn noop_err_print() {}
unsafe extern "C" fn noop_initialize() {}
unsafe extern "C" fn noop_finalize() {}
unsafe extern "C" fn noop_import(_: *const c_char) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_getattr(_: PyObject, _: *const c_char) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_setattr(_: PyObject, _: *const c_char, _: PyObject) -> c_int {
    -1
}
unsafe extern "C" fn noop_run_simple_string(_: *const c_char) -> c_int {
    -1
}
unsafe extern "C" fn noop_call(_: PyObject, _: PyObject) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_call_1(_: PyObject) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_call_kw(_: PyObject, _: PyObject, _: PyObject) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_decref(_: PyObject) {}
unsafe extern "C" fn noop_as_long(_: PyObject) -> c_long {
    0
}
unsafe extern "C" fn noop_as_double(_: PyObject) -> c_double {
    0.0
}
unsafe extern "C" fn noop_as_utf8(_: PyObject) -> *const c_char {
    b"\0".as_ptr() as _
}
unsafe extern "C" fn noop_from_long(_: c_long) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_from_double(_: c_double) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_from_string(_: *const c_char) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_list_new(_: isize) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_list_setitem(_: PyObject, _: isize, _: PyObject) -> c_int {
    -1
}
unsafe extern "C" fn noop_getitem(_: PyObject, _: PyObject) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_setitem(_: PyObject, _: PyObject, _: PyObject) -> c_int {
    -1
}
unsafe extern "C" fn noop_length(_: PyObject) -> isize {
    0
}
unsafe extern "C" fn noop_gil_ensure() -> c_int {
    0
}
unsafe extern "C" fn noop_gil_release(_: c_int) {}
unsafe extern "C" fn noop_tuple_new(_: isize) -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_tuple_setitem(_: PyObject, _: isize, _: PyObject) -> c_int {
    -1
}
unsafe extern "C" fn noop_dict_new() -> PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn noop_dict_setitemstring(_: PyObject, _: *const c_char, _: PyObject) -> c_int {
    -1
}
unsafe extern "C" fn noop_err_fetch(_: *mut PyObject, _: *mut PyObject, _: *mut PyObject) {}

#[cfg(target_os = "windows")]
extern "system" {
    fn LoadLibraryA(lpLibFileName: *const u8) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const u8) -> *mut c_void;
}

unsafe fn compat_dlopen(name: &str) -> *mut c_void { unsafe {
    let cname = CString::new(name).unwrap();
    #[cfg(target_os = "windows")]
    {
        LoadLibraryA(cname.as_ptr() as *const u8)
    }
    #[cfg(not(target_os = "windows"))]
    {
        libc::dlopen(cname.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL)
    }
}}

unsafe fn compat_dlsym<T>(handle: *mut c_void, name: &str) -> T { unsafe {
    let cname = CString::new(name).unwrap();
    #[cfg(target_os = "windows")]
    let sym = { GetProcAddress(handle, cname.as_ptr() as *const u8) };
    #[cfg(not(target_os = "windows"))]
    let sym = { libc::dlsym(handle, cname.as_ptr()) };
    std::mem::transmute_copy(&sym)
}}

fn find_libpython_via_cmd(cmd: &str) -> Option<String> {
    let py_script = r#"
import sysconfig, os, sys
def find():
    ld = sysconfig.get_config_var('LDLIBRARY') or sysconfig.get_config_var('DLLLIBRARY')
    if not ld: return ''
    bases = []
    for var in ['LIBDIR', 'prefix', 'exec_prefix']:
        val = sysconfig.get_config_var(var)
        if val: bases.append(val)
    for attr in ['base_prefix', 'base_exec_prefix', 'prefix', 'exec_prefix']:
        val = getattr(sys, attr, None)
        if val: bases.append(val)
    seen = set()
    unique_bases = [b for b in bases if not (b in seen or seen.add(b))]
    for base in unique_bases:
        for sub in ['', 'lib', 'bin', 'libs']:
            path = os.path.join(base, sub, ld)
            if os.path.exists(path) and os.path.isfile(path):
                return os.path.abspath(path)
    return ld
print(find())
"#;
    let output = std::process::Command::new(cmd)
        .args(&["-c", py_script])
        .output()
        .ok()?;
    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_initialize() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let mut handle = std::ptr::null_mut();

        if let Ok(env_path) = std::env::var("PYTHON_LIBRARY") {
            handle = compat_dlopen(&env_path);
        }

        if handle.is_null() {
            if let Some(path) = find_libpython_via_cmd("python3") {
                handle = compat_dlopen(&path);
            }
        }

        if handle.is_null() {
            if let Some(path) = find_libpython_via_cmd("python") {
                handle = compat_dlopen(&path);
            }
        }

        if handle.is_null() {
            #[cfg(target_os = "windows")]
            {
                for name in &[
                    "python3.dll", "python312.dll", "python311.dll", 
                    "python310.dll", "python39.dll"
                ] {
                    handle = compat_dlopen(name);
                    if !handle.is_null() { break; }
                }
            }
            #[cfg(target_os = "macos")]
            {
                for name in &[
                    "libpython3.dylib", "libpython3.12.dylib", "libpython3.11.dylib", 
                    "libpython3.10.dylib", "libpython3.9.dylib",
                    "/opt/homebrew/lib/libpython3.11.dylib", "/opt/homebrew/lib/libpython3.12.dylib"
                ] {
                    handle = compat_dlopen(name);
                    if !handle.is_null() { break; }
                }
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                for name in &[
                    "libpython3.so", "libpython3.12.so", "libpython3.11.so", 
                    "libpython3.10.so", "libpython3.9.so"
                ] {
                    handle = compat_dlopen(name);
                    if !handle.is_null() { break; }
                }
            }
        }

        if handle.is_null() {
            eprintln!("Warning: could not load libpython3. Python interop will not work.");
            return;
        }
        LIBPYTHON = handle;

        PY_INITIALIZE = compat_dlsym(handle, "Py_Initialize");
        PY_FINALIZE = compat_dlsym(handle, "Py_Finalize");
        PY_IMPORT_IMPORT_MODULE = compat_dlsym(handle, "PyImport_ImportModule");
        PY_OBJECT_GET_ATTR_STRING = compat_dlsym(handle, "PyObject_GetAttrString");
        PY_OBJECT_SET_ATTR_STRING = compat_dlsym(handle, "PyObject_SetAttrString");
        PY_OBJECT_CALL_OBJECT = compat_dlsym(handle, "PyObject_CallObject");
        PY_OBJECT_CALL = compat_dlsym(handle, "PyObject_Call");
        PY_DEC_REF = compat_dlsym(handle, "Py_DecRef");
        PY_INC_REF = compat_dlsym(handle, "Py_IncRef");
        PY_LONG_AS_LONG = compat_dlsym(handle, "PyLong_AsLong");
        PY_FLOAT_AS_DOUBLE = compat_dlsym(handle, "PyFloat_AsDouble");
        PY_UNICODE_AS_UTF8 = compat_dlsym(handle, "PyUnicode_AsUTF8");
        PY_LONG_FROM_LONG = compat_dlsym(handle, "PyLong_FromLong");
        PY_FLOAT_FROM_DOUBLE = compat_dlsym(handle, "PyFloat_FromDouble");
        PY_UNICODE_FROM_STRING = compat_dlsym(handle, "PyUnicode_FromString");
        PY_LIST_NEW = compat_dlsym(handle, "PyList_New");
        PY_LIST_SET_ITEM = compat_dlsym(handle, "PyList_SetItem");
        PY_OBJECT_GET_ITEM = compat_dlsym(handle, "PyObject_GetItem");
        PY_OBJECT_SET_ITEM = compat_dlsym(handle, "PyObject_SetItem");
        PY_OBJECT_LENGTH = compat_dlsym(handle, "PyObject_Length");
        PY_GILSTATE_ENSURE = compat_dlsym(handle, "PyGILState_Ensure");
        PY_GILSTATE_RELEASE = compat_dlsym(handle, "PyGILState_Release");
        PY_TUPLE_NEW = compat_dlsym(handle, "PyTuple_New");
        PY_TUPLE_SET_ITEM = compat_dlsym(handle, "PyTuple_SetItem");
        PY_DICT_NEW = compat_dlsym(handle, "PyDict_New");
        PY_DICT_SET_ITEM_STRING = compat_dlsym(handle, "PyDict_SetItemString");
        PY_SET_NEW = compat_dlsym(handle, "PySet_New");
        PY_SET_ADD = compat_dlsym(handle, "PySet_Add");
        PY_BYTES_FROM_STRING_AND_SIZE = compat_dlsym(handle, "PyBytes_FromStringAndSize");
        PY_SEQUENCE_LIST = compat_dlsym(handle, "PySequence_List");
        PY_BYTES_AS_STRING = compat_dlsym(handle, "PyBytes_AsString");
        PY_BYTES_SIZE = compat_dlsym(handle, "PyBytes_Size");
        PY_DICT_KEYS = compat_dlsym(handle, "PyDict_Keys");
        PY_OBJECT_TYPE = compat_dlsym(handle, "PyObject_Type");
        PY_OBJECT_STR = compat_dlsym(handle, "PyObject_Str");
        PY_ERR_OCCURRED = compat_dlsym(handle, "PyErr_Occurred");
        PY_ERR_FETCH = compat_dlsym(handle, "PyErr_Fetch");
        PY_ERR_NORMALIZE_EXCEPTION = compat_dlsym(handle, "PyErr_NormalizeException");
        PY_ERR_CLEAR = compat_dlsym(handle, "PyErr_Clear");
        PY_ERR_PRINT = compat_dlsym(handle, "PyErr_Print");
        PY_RUN_SIMPLE_STRING = compat_dlsym(handle, "PyRun_SimpleString");

        PY_BOOL_TYPE = compat_dlsym(handle, "PyBool_Type");
        PY_LONG_TYPE = compat_dlsym(handle, "PyLong_Type");
        PY_FLOAT_TYPE = compat_dlsym(handle, "PyFloat_Type");
        PY_UNICODE_TYPE = compat_dlsym(handle, "PyUnicode_Type");
        PY_LIST_TYPE = compat_dlsym(handle, "PyList_Type");
        PY_DICT_TYPE = compat_dlsym(handle, "PyDict_Type");
        PY_SET_TYPE = compat_dlsym(handle, "PySet_Type");
        PY_BYTES_TYPE = compat_dlsym(handle, "PyBytes_Type");
        PY_TYPE_IS_SUBTYPE = compat_dlsym(handle, "PyType_IsSubtype");
        PY_EVAL_SAVE_THREAD = compat_dlsym(handle, "PyEval_SaveThread");
        PY_EVAL_RESTORE_THREAD = compat_dlsym(handle, "PyEval_RestoreThread");
        PY_EVAL_INIT_THREADS = compat_dlsym(handle, "PyEval_InitThreads");

        _PY_NONE_STRUCT = compat_dlsym(handle, "_Py_NoneStruct");

        PY_INITIALIZE();

        let init_ptr: *const () = std::mem::transmute(PY_EVAL_INIT_THREADS);
        if !init_ptr.is_null() && init_ptr != (noop_initialize as *const ()) {
            PY_EVAL_INIT_THREADS();
        }

        let py_setup_code = format!(r#"
import collections.abc
import ctypes
import sys

_conv_to_py = None
_conv_to_olive = None

class OliveListProxy(collections.abc.MutableSequence):
    def __init__(self, ptr):
        self._ptr = ptr
        self._get = ctypes.CFUNCTYPE(ctypes.c_int64, ctypes.c_int64, ctypes.c_int64)({get_fn})
        self._set = ctypes.CFUNCTYPE(None, ctypes.c_int64, ctypes.c_int64, ctypes.c_int64)({set_fn})
        self._len = ctypes.CFUNCTYPE(ctypes.c_int64, ctypes.c_int64)({len_fn})
        self._append = ctypes.CFUNCTYPE(None, ctypes.c_int64, ctypes.c_int64)({append_fn})
        self._insert = ctypes.CFUNCTYPE(None, ctypes.c_int64, ctypes.c_int64, ctypes.c_int64)({insert_fn})
        self._remove = ctypes.CFUNCTYPE(ctypes.c_int64, ctypes.c_int64, ctypes.c_int64)({remove_fn})

    def __len__(self):
        return self._len(self._ptr)

    def __getitem__(self, index):
        length = len(self)
        if isinstance(index, slice):
            start, stop, step = index.indices(length)
            return [self[i] for i in range(start, stop, step)]
        if index < 0:
            index += length
        if index < 0 or index >= length:
            raise IndexError("list index out of range")
        return _conv_to_py(self._get(self._ptr, index))

    def __setitem__(self, index, value):
        length = len(self)
        if index < 0:
            index += length
        if index < 0 or index >= length:
            raise IndexError("list index out of range")
        self._set(self._ptr, index, _conv_to_olive(value))

    def __delitem__(self, index):
        length = len(self)
        if index < 0:
            index += length
        if index < 0 or index >= length:
            raise IndexError("list index out of range")
        self._remove(self._ptr, index)

    def insert(self, index, value):
        length = len(self)
        if index < 0:
            index += length
        if index < 0:
            index = 0
        if index >= length:
            self._append(self._ptr, _conv_to_olive(value))
        else:
            self._insert(self._ptr, index, _conv_to_olive(value))

    def append(self, value):
        self._append(self._ptr, _conv_to_olive(value))

class OliveDictProxy(collections.abc.MutableMapping):
    def __init__(self, ptr):
        self._ptr = ptr
        self._get = ctypes.CFUNCTYPE(ctypes.c_int64, ctypes.c_int64, ctypes.c_int64)({obj_get_fn})
        self._set = ctypes.CFUNCTYPE(ctypes.c_int64, ctypes.c_int64, ctypes.c_int64, ctypes.c_int64)({obj_set_fn})
        self._len = ctypes.CFUNCTYPE(ctypes.c_int64, ctypes.c_int64)({obj_len_fn})
        self._keys = ctypes.CFUNCTYPE(ctypes.c_int64, ctypes.c_int64)({obj_keys_fn})
        self._remove = ctypes.CFUNCTYPE(ctypes.c_int64, ctypes.c_int64, ctypes.c_int64)({obj_remove_fn})

    def __len__(self):
        return self._len(self._ptr)

    def __iter__(self):
        keys_ptr = self._keys(self._ptr)
        py_keys = OliveListProxy(keys_ptr)
        return iter(py_keys)

    def __getitem__(self, key):
        if not isinstance(key, str):
            raise KeyError(key)
        key_bytes = key.encode('utf-8')
        key_ptr = ctypes.cast(ctypes.c_char_p(key_bytes), ctypes.c_void_p).value
        val_ptr = self._get(self._ptr, key_ptr | 1)
        if val_ptr == 0:
            raise KeyError(key)
        return _conv_to_py(val_ptr)

    def __setitem__(self, key, value):
        if not isinstance(key, str):
            raise TypeError("keys must be strings")
        key_bytes = key.encode('utf-8')
        key_ptr = ctypes.cast(ctypes.c_char_p(key_bytes), ctypes.c_void_p).value
        self._set(self._ptr, key_ptr | 1, _conv_to_olive(value))

    def __delitem__(self, key):
        if not isinstance(key, str):
            raise KeyError(key)
        key_bytes = key.encode('utf-8')
        key_ptr = ctypes.cast(ctypes.c_char_p(key_bytes), ctypes.c_void_p).value
        res = self._remove(self._ptr, key_ptr | 1)
        if res == 0:
            raise KeyError(key)

_decref = ctypes.pythonapi.Py_DecRef
_decref.argtypes = [ctypes.py_object]
_decref.restype = None

_conv_to_py_raw = ctypes.CFUNCTYPE(ctypes.py_object, ctypes.c_int64)({conv_to_py_fn})
def _conv_to_py(val):
    obj = _conv_to_py_raw(val)
    if obj is not None:
        _decref(obj)
    return obj

_conv_to_olive = ctypes.CFUNCTYPE(ctypes.c_int64, ctypes.py_object)({conv_to_olive_fn})

sys.modules['olive_proxies'] = type('Module', (), {{
    'OliveListProxy': OliveListProxy,
    'OliveDictProxy': OliveDictProxy,
}})()
"#,
            get_fn = crate::olive_list_get as *const () as usize,
            set_fn = crate::olive_list_set as *const () as usize,
            len_fn = crate::olive_list_len as *const () as usize,
            append_fn = crate::olive_list_append as *const () as usize,
            insert_fn = crate::olive_list_insert as *const () as usize,
            remove_fn = crate::olive_list_remove as *const () as usize,
            obj_get_fn = crate::olive_obj_get as *const () as usize,
            obj_set_fn = crate::olive_obj_set as *const () as usize,
            obj_len_fn = crate::olive_obj_len as *const () as usize,
            obj_keys_fn = olive_dict_keys_ffi as *const () as usize,
            obj_remove_fn = crate::olive_obj_remove as *const () as usize,
            conv_to_py_fn = olive_py_conv_to_py as *const () as usize,
            conv_to_olive_fn = olive_py_conv_to_olive as *const () as usize,
        );

        let c_setup = CString::new(py_setup_code).unwrap();
        PY_RUN_SIMPLE_STRING(c_setup.as_ptr());

        let save_ptr: *const () = std::mem::transmute(PY_EVAL_SAVE_THREAD);
        if !save_ptr.is_null() && save_ptr != (noop_save_thread as *const ()) {
            MAIN_THREAD_STATE = PY_EVAL_SAVE_THREAD();
        }
        INITIALIZED.store(true, Ordering::SeqCst);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_finalize() {
    unsafe {
        if INITIALIZED.load(Ordering::SeqCst) {
            let restore_ptr: *const () = std::mem::transmute(PY_EVAL_RESTORE_THREAD);
            if !restore_ptr.is_null()
                && restore_ptr != (noop_restore_thread as *const ())
                && !MAIN_THREAD_STATE.is_null()
            {
                PY_EVAL_RESTORE_THREAD(MAIN_THREAD_STATE);
                MAIN_THREAD_STATE = std::ptr::null_mut();
            } else {
                let _gil = PY_GILSTATE_ENSURE();
            }
            PY_FINALIZE();
            INITIALIZED.store(false, Ordering::SeqCst);
        }
    }
}



pub(crate) fn is_readable_ptr(ptr: *const c_void) -> bool {
    crate::is_active_object(ptr as i64)
}

unsafe fn handle_py_error() { unsafe {
    if PY_ERR_OCCURRED().is_null() {
        return;
    }
    let mut ptype = std::ptr::null_mut();
    let mut pvalue = std::ptr::null_mut();
    let mut ptraceback = std::ptr::null_mut();
    PY_ERR_FETCH(&mut ptype, &mut pvalue, &mut ptraceback);
    PY_ERR_NORMALIZE_EXCEPTION(&mut ptype, &mut pvalue, &mut ptraceback);

    let mut tb_msg = String::new();

    if !ptraceback.is_null() {
        let tb_mod = PY_IMPORT_IMPORT_MODULE(b"traceback\0".as_ptr() as *const c_char);
        if !tb_mod.is_null() {
            let fmt_func =
                PY_OBJECT_GET_ATTR_STRING(tb_mod, b"format_exception\0".as_ptr() as *const c_char);
            if !fmt_func.is_null() {
                let py_args = PY_TUPLE_NEW(3);
                PY_TUPLE_SET_ITEM(py_args, 0, ptype);
                PY_TUPLE_SET_ITEM(py_args, 1, pvalue);
                PY_TUPLE_SET_ITEM(py_args, 2, ptraceback);

                // References stolen by PyTuple_SetItem.
                ptype = std::ptr::null_mut();
                pvalue = std::ptr::null_mut();
                ptraceback = std::ptr::null_mut();

                PY_ERR_CLEAR();

                let py_list = PY_OBJECT_CALL_OBJECT(fmt_func, py_args);
                if !py_list.is_null() {
                    let len = PY_OBJECT_LENGTH(py_list) as usize;
                    for i in 0..len {
                        let idx_obj = PY_LONG_FROM_LONG(i as c_long);
                        let py_item = PY_OBJECT_GET_ITEM(py_list, idx_obj);
                        if !py_item.is_null() {
                            let s = PY_UNICODE_AS_UTF8(py_item);
                            if !s.is_null() {
                                tb_msg.push_str(&CStr::from_ptr(s).to_string_lossy());
                            }
                            PY_DEC_REF(py_item);
                        }
                        PY_DEC_REF(idx_obj);
                    }
                    PY_DEC_REF(py_list);
                }
                PY_DEC_REF(py_args);
                PY_DEC_REF(fmt_func);
            }
            PY_DEC_REF(tb_mod);
        }
    }

    if tb_msg.is_empty() {
        let mut err_msg = "Unknown Python Exception".to_string();
        if !pvalue.is_null() {
            let str_obj = PY_OBJECT_STR(pvalue);
            if !str_obj.is_null() {
                let utf8_ptr = PY_UNICODE_AS_UTF8(str_obj);
                if !utf8_ptr.is_null() {
                    err_msg = CStr::from_ptr(utf8_ptr).to_string_lossy().into_owned();
                }
                PY_DEC_REF(str_obj);
            }
        }
        tb_msg = format!("Python Exception: {}", err_msg);
    }

    PY_ERR_CLEAR();

    if !ptype.is_null() {
        PY_DEC_REF(ptype);
    }
    if !pvalue.is_null() {
        PY_DEC_REF(pvalue);
    }
    if !ptraceback.is_null() {
        PY_DEC_REF(ptraceback);
    }

    let ptr = crate::olive_str_internal(&tb_msg);
    crate::olive_panic(ptr);
}}

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
                    _ => {
                        let f = f64::from_bits(val as u64);
                        if f.is_finite() && !f.is_nan() && f.abs() > 1e-300 && f.abs() < 1e300 {
                            PY_FLOAT_FROM_DOUBLE(f)
                        } else {
                            PY_LONG_FROM_LONG(val as c_long)
                        }
                    }
                }
            }
        } else {
            unsafe {
                let f = f64::from_bits(val as u64);
                if f.is_finite() && !f.is_nan() && f.abs() > 1e-300 && f.abs() < 1e300 {
                    PY_FLOAT_FROM_DOUBLE(f)
                } else {
                    PY_LONG_FROM_LONG(val as c_long)
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call(func: PyObject, args_list: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
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
        if res.is_null() {
            handle_py_error();
        }
        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(res)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_call_kw(func: PyObject, args_list: i64, kwargs_dict: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_func = unsafe { olive_py_unwrap(func) };
    if unwrapped_func.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let gil = PY_GILSTATE_ENSURE();

        let py_args = if args_list != 0 {
            let sv = &*(args_list as *const crate::StableVec);
            let args = PY_TUPLE_NEW(sv.len as isize);
            for i in 0..sv.len {
                let v = *sv.ptr.add(i);
                let py_v = olive_to_py(v);
                PY_TUPLE_SET_ITEM(args, i as isize, py_v);
            }
            args
        } else {
            PY_TUPLE_NEW(0)
        };

        let mut py_kwargs = std::ptr::null_mut();
        if kwargs_dict != 0 {
            let sv = &*(kwargs_dict as *const crate::StableVec);
            py_kwargs = PY_DICT_NEW();
            for i in (0..sv.len).step_by(2) {
                let k_ptr = *sv.ptr.add(i);
                let v = *sv.ptr.add(i + 1);

                let k_str = crate::olive_str_from_ptr(k_ptr);
                let k_cstr = CString::new(k_str).unwrap();
                let py_v = olive_to_py(v);

                PY_DICT_SET_ITEM_STRING(py_kwargs, k_cstr.as_ptr(), py_v);
            }
        }

        let res = PY_OBJECT_CALL(unwrapped_func, py_args, py_kwargs);
        if res.is_null() {
            handle_py_error();
        }

        if !py_args.is_null() {
            PY_DEC_REF(py_args);
        }
        if !py_kwargs.is_null() {
            PY_DEC_REF(py_kwargs);
        }

        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(res)
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
pub extern "C" fn olive_py_from_int(v: i64) -> PyObject {
    check_python_loaded();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_LONG_FROM_LONG(v as c_long);
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(r)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_float(v: f64) -> PyObject {
    check_python_loaded();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_FLOAT_FROM_DOUBLE(v as c_double);
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(r)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_str(s: i64) -> PyObject {
    check_python_loaded();
    let r_str = crate::olive_str_from_ptr(s);
    let c = CString::new(r_str).unwrap();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = PY_UNICODE_FROM_STRING(c.as_ptr());
        PY_GILSTATE_RELEASE(gil);
        olive_py_wrap_owned(r)
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

unsafe fn py_to_olive_internal(py_val: PyObject) -> i64 { unsafe {
    if py_val.is_null() || py_val == _PY_NONE_STRUCT {
        return 0;
    }

    let ty = PY_OBJECT_TYPE(py_val);
    if ty.is_null() {
        return 0;
    }

    let name_attr = PY_OBJECT_GET_ATTR_STRING(ty, b"__name__\0".as_ptr() as *const c_char);
    if !name_attr.is_null() {
        let s = PY_UNICODE_AS_UTF8(name_attr);
        if !s.is_null() {
            let type_name = CStr::from_ptr(s).to_string_lossy();
            if type_name == "OliveListProxy" || type_name == "OliveDictProxy" {
                let ptr_attr =
                    PY_OBJECT_GET_ATTR_STRING(py_val, b"_ptr\0".as_ptr() as *const c_char);
                if !ptr_attr.is_null() {
                    let raw_ptr = PY_LONG_AS_LONG(ptr_attr) as i64;
                    PY_DEC_REF(ptr_attr);
                    PY_DEC_REF(name_attr);
                    PY_DEC_REF(ty);
                    return raw_ptr;
                }
            }
        }
        PY_DEC_REF(name_attr);
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
        olive_py_wrap(py_val) as i64
    };

    PY_DEC_REF(ty);
    result
}}

unsafe fn olive_py_to_list_internal(obj: PyObject) -> i64 { unsafe {
    let len = PY_OBJECT_LENGTH(obj) as usize;
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
}}

unsafe fn olive_py_to_dict_internal(obj: PyObject) -> i64 { unsafe {
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
}}

unsafe fn olive_py_to_set_internal(obj: PyObject) -> i64 { unsafe {
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
}}

unsafe fn olive_py_to_bytes_internal(obj: PyObject) -> i64 { unsafe {
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
}}

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

unsafe fn olive_py_wrap_borrowed(py_ptr: PyObject) -> PyObject { unsafe {
    if py_ptr.is_null() {
        return std::ptr::null_mut();
    }
    PY_INC_REF(py_ptr);
    olive_py_wrap_owned(py_ptr)
}}

unsafe fn olive_py_wrap(py_ptr: PyObject) -> PyObject { unsafe {
    olive_py_wrap_borrowed(py_ptr)
}}

unsafe fn olive_py_unwrap(val: PyObject) -> PyObject { unsafe {
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
}}

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
pub extern "C" fn olive_py_conv_to_py(val: i64) -> PyObject {
    check_python_loaded();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = olive_to_py(val);
        PY_GILSTATE_RELEASE(gil);
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_conv_to_olive(py_val: PyObject) -> i64 {
    check_python_loaded();
    unsafe {
        let gil = PY_GILSTATE_ENSURE();
        let r = py_to_olive_internal(py_val);
        PY_GILSTATE_RELEASE(gil);
        r
    }
}

unsafe fn olive_py_create_list_proxy(ptr: i64) -> PyObject { unsafe {
    let mod_name = CString::new("olive_proxies").unwrap();
    let m = PY_IMPORT_IMPORT_MODULE(mod_name.as_ptr());
    if m.is_null() {
        return std::ptr::null_mut();
    }
    let class_obj = PY_OBJECT_GET_ATTR_STRING(m, b"OliveListProxy\0".as_ptr() as *const c_char);
    PY_DEC_REF(m);
    if class_obj.is_null() {
        return std::ptr::null_mut();
    }
    let py_args = PY_TUPLE_NEW(1);
    let py_ptr_val = PY_LONG_FROM_LONG(ptr as c_long);
    PY_TUPLE_SET_ITEM(py_args, 0, py_ptr_val);
    let res = PY_OBJECT_CALL_OBJECT(class_obj, py_args);
    PY_DEC_REF(class_obj);
    PY_DEC_REF(py_args);
    res
}}

unsafe fn olive_py_create_dict_proxy(ptr: i64) -> PyObject { unsafe {
    let mod_name = CString::new("olive_proxies").unwrap();
    let m = PY_IMPORT_IMPORT_MODULE(mod_name.as_ptr());
    if m.is_null() {
        return std::ptr::null_mut();
    }
    let class_obj = PY_OBJECT_GET_ATTR_STRING(m, b"OliveDictProxy\0".as_ptr() as *const c_char);
    PY_DEC_REF(m);
    if class_obj.is_null() {
        return std::ptr::null_mut();
    }
    let py_args = PY_TUPLE_NEW(1);
    let py_ptr_val = PY_LONG_FROM_LONG(ptr as c_long);
    PY_TUPLE_SET_ITEM(py_args, 0, py_ptr_val);
    let res = PY_OBJECT_CALL_OBJECT(class_obj, py_args);
    PY_DEC_REF(class_obj);
    PY_DEC_REF(py_args);
    res
}}

unsafe fn catch_py_exception_msg() -> Option<String> { unsafe {
    if PY_ERR_OCCURRED().is_null() {
        return None;
    }
    let mut ptype = std::ptr::null_mut();
    let mut pvalue = std::ptr::null_mut();
    let mut ptraceback = std::ptr::null_mut();
    PY_ERR_FETCH(&mut ptype, &mut pvalue, &mut ptraceback);
    PY_ERR_NORMALIZE_EXCEPTION(&mut ptype, &mut pvalue, &mut ptraceback);

    let mut tb_msg = String::new();

    if !ptraceback.is_null() {
        let tb_mod = PY_IMPORT_IMPORT_MODULE(b"traceback\0".as_ptr() as *const c_char);
        if !tb_mod.is_null() {
            let fmt_func =
                PY_OBJECT_GET_ATTR_STRING(tb_mod, b"format_exception\0".as_ptr() as *const c_char);
            if !fmt_func.is_null() {
                let py_args = PY_TUPLE_NEW(3);
                PY_TUPLE_SET_ITEM(py_args, 0, ptype);
                PY_TUPLE_SET_ITEM(py_args, 1, pvalue);
                PY_TUPLE_SET_ITEM(py_args, 2, ptraceback);

                ptype = std::ptr::null_mut();
                pvalue = std::ptr::null_mut();
                ptraceback = std::ptr::null_mut();

                PY_ERR_CLEAR();

                let py_list = PY_OBJECT_CALL_OBJECT(fmt_func, py_args);
                if !py_list.is_null() {
                    let len = PY_OBJECT_LENGTH(py_list) as usize;
                    for i in 0..len {
                        let idx_obj = PY_LONG_FROM_LONG(i as c_long);
                        let py_item = PY_OBJECT_GET_ITEM(py_list, idx_obj);
                        if !py_item.is_null() {
                            let s = PY_UNICODE_AS_UTF8(py_item);
                            if !s.is_null() {
                                tb_msg.push_str(&CStr::from_ptr(s).to_string_lossy());
                            }
                            PY_DEC_REF(py_item);
                        }
                        PY_DEC_REF(idx_obj);
                    }
                    PY_DEC_REF(py_list);
                }
                PY_DEC_REF(py_args);
                PY_DEC_REF(fmt_func);
            }
            PY_DEC_REF(tb_mod);
        }
    }

    if tb_msg.is_empty() {
        let mut err_msg = "Unknown Python Exception".to_string();
        if !pvalue.is_null() {
            let str_obj = PY_OBJECT_STR(pvalue);
            if !str_obj.is_null() {
                let utf8_ptr = PY_UNICODE_AS_UTF8(str_obj);
                if !utf8_ptr.is_null() {
                    err_msg = CStr::from_ptr(utf8_ptr).to_string_lossy().into_owned();
                }
                PY_DEC_REF(str_obj);
            }
        }
        tb_msg = format!("Python Exception: {}", err_msg);
    }

    PY_ERR_CLEAR();

    if !ptype.is_null() {
        PY_DEC_REF(ptype);
    }
    if !pvalue.is_null() {
        PY_DEC_REF(pvalue);
    }
    if !ptraceback.is_null() {
        PY_DEC_REF(ptraceback);
    }

    Some(tb_msg)
}}

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
    use std::ffi::CString;

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
            let sv = &mut *(list_ptr as *mut crate::StableVec);
            *sv.ptr.add(0) = crate::olive_str_internal("hello");
            *sv.ptr.add(1) = 24680;

            let gil = PY_GILSTATE_ENSURE();
            let py_proxy = olive_to_py(list_ptr);
            assert!(!py_proxy.is_null());

            let sys_mod = PY_IMPORT_IMPORT_MODULE(b"sys\0".as_ptr() as *const c_char);
            assert!(!sys_mod.is_null());

            let len_fn =
                PY_OBJECT_GET_ATTR_STRING(py_proxy, b"__len__\0".as_ptr() as *const c_char);
            let py_len = PY_OBJECT_CALL_OBJECT(len_fn, std::ptr::null_mut());
            assert_eq!(PY_LONG_AS_LONG(py_len), 2);
            PY_DEC_REF(py_len);
            PY_DEC_REF(len_fn);

            let hello_ptr = crate::olive_list_get(list_ptr, 0);

            let idx_0 = PY_LONG_FROM_LONG(0);
            let val_0 = PY_UNICODE_FROM_STRING(b"world\0".as_ptr() as *const c_char);
            let setitem_res = PY_OBJECT_SET_ITEM(py_proxy, idx_0, val_0);
            assert_ne!(setitem_res, -1);
            PY_DEC_REF(idx_0);
            PY_DEC_REF(val_0);

            let olive_val_0 = crate::olive_list_get(list_ptr, 0);
            assert_eq!(crate::olive_str_from_ptr(olive_val_0), "world");

            let idx_1 = PY_LONG_FROM_LONG(1);
            let val_insert = PY_UNICODE_FROM_STRING(b"inserted\0".as_ptr() as *const c_char);
            
            let insert_fn = PY_OBJECT_GET_ATTR_STRING(py_proxy, b"insert\0".as_ptr() as *const c_char);
            let args_tuple = PY_TUPLE_NEW(2);
            PY_TUPLE_SET_ITEM(args_tuple, 0, idx_1);
            PY_TUPLE_SET_ITEM(args_tuple, 1, val_insert);
            let insert_res = PY_OBJECT_CALL_OBJECT(insert_fn, args_tuple);
            assert!(!insert_res.is_null());
            PY_DEC_REF(insert_res);
            PY_DEC_REF(insert_fn);
            PY_DEC_REF(args_tuple);

            assert_eq!(crate::olive_list_len(list_ptr), 3);
            let val_at_1 = crate::olive_list_get(list_ptr, 1);
            assert_eq!(crate::olive_str_from_ptr(val_at_1), "inserted");

            let del_fn = PY_OBJECT_GET_ATTR_STRING(py_proxy, b"__delitem__\0".as_ptr() as *const c_char);
            let del_args = PY_TUPLE_NEW(1);
            let idx_to_del = PY_LONG_FROM_LONG(1);
            PY_TUPLE_SET_ITEM(del_args, 0, idx_to_del);
            let del_res = PY_OBJECT_CALL_OBJECT(del_fn, del_args);
            assert!(!del_res.is_null());
            PY_DEC_REF(del_res);
            PY_DEC_REF(del_fn);
            PY_DEC_REF(del_args);

            assert_eq!(crate::olive_list_len(list_ptr), 2);
            assert_ne!(crate::olive_str_from_ptr(crate::olive_list_get(list_ptr, 1)), "inserted");

            let dict_ptr = crate::olive_obj_new();
            let py_dict = olive_to_py(dict_ptr);
            
            let dict_key = PY_UNICODE_FROM_STRING(b"testkey\0".as_ptr() as *const c_char);
            let dict_val = PY_LONG_FROM_LONG(9876);
            assert_ne!(PY_OBJECT_SET_ITEM(py_dict, dict_key, dict_val), -1);
            PY_DEC_REF(dict_key);
            PY_DEC_REF(dict_val);
            
            assert_eq!(crate::olive_obj_get(dict_ptr, crate::olive_str_internal("testkey")), 9876);

            let dict_del_fn = PY_OBJECT_GET_ATTR_STRING(py_dict, b"__delitem__\0".as_ptr() as *const c_char);
            let dict_del_args = PY_TUPLE_NEW(1);
            let dict_del_key = PY_UNICODE_FROM_STRING(b"testkey\0".as_ptr() as *const c_char);
            PY_TUPLE_SET_ITEM(dict_del_args, 0, dict_del_key);
            let dict_del_res = PY_OBJECT_CALL_OBJECT(dict_del_fn, dict_del_args);
            assert!(!dict_del_res.is_null());
            PY_DEC_REF(dict_del_res);
            PY_DEC_REF(dict_del_fn);
            PY_DEC_REF(dict_del_args);

            assert_eq!(crate::olive_obj_get(dict_ptr, crate::olive_str_internal("testkey")), 0);
            
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
        let initial_active_count = crate::active_objects_count();

        for i in 0..100 {
            let py_num = olive_py_from_int(i);
            assert_eq!(olive_py_to_int(py_num), i);

            let py_dict = unsafe {
                let gil = PY_GILSTATE_ENSURE();
                let d = PY_DICT_NEW();
                PY_GILSTATE_RELEASE(gil);
                olive_py_wrap_owned(d)
            };

            olive_py_decref(py_num);
            olive_py_decref(py_dict);
        }

        let final_active_count = crate::active_objects_count();
        assert_eq!(
            final_active_count, initial_active_count,
            "Active object leaked in interop wrapping!"
        );
    }
}
