use crate::python::*;
use std::os::raw::{c_char, c_double, c_int, c_long, c_void};
use std::sync::atomic::AtomicBool;

pub static INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static mut LIBPYTHON: *mut c_void = std::ptr::null_mut();

pub static mut PY_INITIALIZE: unsafe extern "C" fn() = noop_initialize;
pub static mut PY_FINALIZE: unsafe extern "C" fn() = noop_finalize;
pub static mut PY_NUMBER_OR: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
    noop_pynumber;
pub static mut PY_NUMBER_ADD: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
    noop_pynumber;
pub static mut PY_NUMBER_SUBTRACT: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
    noop_pynumber;
pub static mut PY_NUMBER_MULTIPLY: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
    noop_pynumber;
pub static mut PY_NUMBER_TRUEDIVIDE: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
    noop_pynumber;
pub static mut PY_NUMBER_REMAINDER: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
    noop_pynumber;
pub static mut PY_NUMBER_POWER: unsafe extern "C" fn(
    *mut c_void,
    *mut c_void,
    *mut c_void,
) -> *mut c_void = noop_pynumber_power;
pub static mut PY_IMPORT_IMPORT_MODULE: unsafe extern "C" fn(*const c_char) -> PyObject =
    noop_import;
pub static mut PY_OBJECT_GET_ATTR_STRING: unsafe extern "C" fn(
    PyObject,
    *const c_char,
) -> PyObject = noop_getattr;
pub static mut PY_OBJECT_SET_ATTR_STRING: unsafe extern "C" fn(
    PyObject,
    *const c_char,
    PyObject,
) -> c_int = noop_setattr;
pub static mut PY_RUN_SIMPLE_STRING: unsafe extern "C" fn(*const c_char) -> c_int =
    noop_run_simple_string;
pub static mut PY_OBJECT_CALL_OBJECT: unsafe extern "C" fn(PyObject, PyObject) -> PyObject =
    noop_call;
pub static mut PY_OBJECT_CALL: unsafe extern "C" fn(PyObject, PyObject, PyObject) -> PyObject =
    noop_call_kw;
pub static mut PY_DEC_REF: unsafe extern "C" fn(PyObject) = noop_decref;
pub static mut PY_INC_REF: unsafe extern "C" fn(PyObject) = noop_incref;
pub static mut PY_LONG_AS_LONG: unsafe extern "C" fn(PyObject) -> c_long = noop_as_long;
pub static mut PY_NUMBER_LONG: unsafe extern "C" fn(PyObject) -> PyObject = noop_number_long;
pub static mut PY_FLOAT_AS_DOUBLE: unsafe extern "C" fn(PyObject) -> c_double = noop_as_double;
pub static mut PY_UNICODE_AS_UTF8: unsafe extern "C" fn(PyObject) -> *const c_char = noop_as_utf8;
pub static mut PY_LONG_FROM_LONG: unsafe extern "C" fn(c_long) -> PyObject = noop_from_long;
pub static mut PY_BOOL_FROM_LONG: unsafe extern "C" fn(c_long) -> PyObject = noop_from_long;
pub static mut PY_FLOAT_FROM_DOUBLE: unsafe extern "C" fn(c_double) -> PyObject = noop_from_double;
pub static mut PY_UNICODE_FROM_STRING: unsafe extern "C" fn(*const c_char) -> PyObject =
    noop_from_string;
pub static mut PY_LIST_NEW: unsafe extern "C" fn(isize) -> PyObject = noop_list_new;
pub static mut PY_LIST_SET_ITEM: unsafe extern "C" fn(PyObject, isize, PyObject) -> c_int =
    noop_list_setitem;
pub static mut PY_OBJECT_GET_ITEM: unsafe extern "C" fn(PyObject, PyObject) -> PyObject =
    noop_getitem;
pub static mut PY_OBJECT_SET_ITEM: unsafe extern "C" fn(PyObject, PyObject, PyObject) -> c_int =
    noop_setitem;
pub static mut PY_OBJECT_DEL_ITEM: unsafe extern "C" fn(PyObject, PyObject) -> c_int =
    noop_dict_setitem_del;
pub static mut PY_OBJECT_LENGTH: unsafe extern "C" fn(PyObject) -> isize = noop_length;
pub static mut PY_GILSTATE_ENSURE: unsafe extern "C" fn() -> c_int = noop_gil_ensure;
pub static mut PY_GILSTATE_RELEASE: unsafe extern "C" fn(c_int) = noop_gil_release;
pub static mut PY_TUPLE_NEW: unsafe extern "C" fn(isize) -> PyObject = noop_tuple_new;
pub static mut PY_TUPLE_SET_ITEM: unsafe extern "C" fn(PyObject, isize, PyObject) -> c_int =
    noop_tuple_setitem;
pub static mut PY_DICT_NEW: unsafe extern "C" fn() -> PyObject = noop_dict_new;
pub static mut PY_DICT_SET_ITEM_STRING: unsafe extern "C" fn(
    PyObject,
    *const c_char,
    PyObject,
) -> c_int = noop_dict_setitemstring;
pub static mut PY_DICT_KEYS: unsafe extern "C" fn(PyObject) -> PyObject = noop_call_1;
pub static mut PY_OBJECT_TYPE: unsafe extern "C" fn(PyObject) -> PyObject = noop_call_1;
pub static mut PY_OBJECT_STR: unsafe extern "C" fn(PyObject) -> PyObject = noop_call_1;
pub static mut PY_ERR_OCCURRED: unsafe extern "C" fn() -> PyObject = noop_dict_new;
pub static mut PY_ERR_FETCH: unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) =
    noop_err_fetch;
pub static mut PY_ERR_NORMALIZE_EXCEPTION: unsafe extern "C" fn(
    *mut PyObject,
    *mut PyObject,
    *mut PyObject,
) = noop_err_fetch;
pub static mut PY_ERR_CLEAR: unsafe extern "C" fn() = noop_initialize;

pub static mut PY_SET_NEW: unsafe extern "C" fn(PyObject) -> PyObject = noop_call_1;
pub static mut PY_SET_ADD: unsafe extern "C" fn(PyObject, PyObject) -> c_int = noop_set_add;
pub static mut PY_BYTES_FROM_STRING_AND_SIZE: unsafe extern "C" fn(*const u8, isize) -> PyObject =
    noop_bytes_from_string;
pub static mut PY_SEQUENCE_LIST: unsafe extern "C" fn(PyObject) -> PyObject = noop_call_1;
pub static mut PY_BYTES_AS_STRING: unsafe extern "C" fn(PyObject) -> *const c_char =
    noop_bytes_as_string;
pub static mut PY_BYTES_SIZE: unsafe extern "C" fn(PyObject) -> isize = noop_bytes_size;
pub static mut PY_BYTES_FROM_OBJECT: unsafe extern "C" fn(PyObject) -> PyObject = noop_call_1;

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

pub static mut PY_OBJECT_GET_ITER: unsafe extern "C" fn(PyObject) -> PyObject = noop_get_iter;
pub static mut PY_ITER_NEXT: unsafe extern "C" fn(PyObject) -> PyObject = noop_iter_next;
pub static mut PY_CORO_CHECK_EXACT: unsafe extern "C" fn(PyObject) -> c_int = noop_check_int;
pub static mut PY_ITER_CHECK: unsafe extern "C" fn(PyObject) -> c_int = noop_check_int;

pub static mut PY_TRACEBACK_FORMAT_EXCEPTION: PyObject = std::ptr::null_mut();

pub static mut PY_DICT_NEXT: unsafe extern "C" fn(
    PyObject,
    *mut isize,
    *mut PyObject,
    *mut PyObject,
) -> c_int = noop_dict_next;
pub static mut PY_LIST_GET_ITEM: unsafe extern "C" fn(PyObject, isize) -> PyObject =
    noop_getitem_idx;
pub static mut PY_TUPLE_GET_ITEM: unsafe extern "C" fn(PyObject, isize) -> PyObject =
    noop_getitem_idx;
pub static mut PY_TUPLE_SIZE: unsafe extern "C" fn(PyObject) -> isize = noop_tuple_size;
pub static mut PY_TUPLE_TYPE: PyObject = std::ptr::null_mut();
pub static mut PY_OBJECT_RICHCOMPAREBOOL: unsafe extern "C" fn(PyObject, PyObject, c_int) -> c_int =
    crate::python::python_noop::noop_richcomparebool;
pub static mut PY_SLICE_NEW: unsafe extern "C" fn(PyObject, PyObject, PyObject) -> PyObject =
    crate::python::python_noop::noop_slice_new;

/// `PyObject_Vectorcall`/`PyObject_VectorcallMethod`, dlsym'd when the
/// loaded libpython has them (CPython 3.9+). `HAS_VECTORCALL` gates every
/// call site -- a missing symbol is never invoked, it just leaves the flag
/// false and callers keep using the tuple-call path.
pub static mut PY_VECTORCALL: unsafe extern "C" fn(
    PyObject,
    *const PyObject,
    usize,
    PyObject,
) -> PyObject = noop_vectorcall;
pub static mut PY_VECTORCALL_METHOD: unsafe extern "C" fn(
    PyObject,
    *const PyObject,
    usize,
    PyObject,
) -> PyObject = noop_vectorcall;
pub static HAS_VECTORCALL: AtomicBool = AtomicBool::new(false);

/// `PyObject_GetBuffer`/`PyBuffer_Release`/`PyObject_CheckBuffer`, dlsym'd
/// when present (R14). `HAS_BUFFER` gates the zero-copy buffer ingest path;
/// a missing symbol leaves it false and callers keep using the per-element
/// conversion loop.
pub static mut PY_OBJECT_GET_BUFFER: unsafe extern "C" fn(PyObject, *mut c_void, c_int) -> c_int =
    noop_get_buffer;
pub static mut PY_BUFFER_RELEASE: unsafe extern "C" fn(*mut c_void) = noop_buffer_release;
pub static mut PY_OBJECT_CHECK_BUFFER: unsafe extern "C" fn(PyObject) -> c_int = noop_check_int;
pub static HAS_BUFFER: AtomicBool = AtomicBool::new(false);

/// `PyUnicode_InternFromString`/`PyObject_GetAttr`/`PyObject_SetAttr`, dlsym'd
/// when present. `HAS_INTERN` gates the interned-name attribute path; a
/// missing symbol leaves it false and callers keep using
/// `PyObject_GetAttrString`/`SetAttrString`, which rebuild the name string
/// every call.
pub static mut PY_UNICODE_INTERN_FROM_STRING: unsafe extern "C" fn(*const c_char) -> PyObject =
    noop_from_string;
pub static mut PY_OBJECT_GET_ATTR: unsafe extern "C" fn(PyObject, PyObject) -> PyObject =
    noop_getitem;
pub static mut PY_OBJECT_SET_ATTR: unsafe extern "C" fn(PyObject, PyObject, PyObject) -> c_int =
    noop_setitem;
pub static HAS_INTERN: AtomicBool = AtomicBool::new(false);
