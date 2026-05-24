use crate::python::*;
use std::os::raw::{c_char, c_double, c_int, c_long, c_void};

pub unsafe extern "C" fn noop_set_add(_: PyObject, _: PyObject) -> c_int {
    -1
}
pub unsafe extern "C" fn noop_bytes_from_string(_: *const u8, _: isize) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_bytes_as_string(_: PyObject) -> *const c_char {
    std::ptr::null()
}
pub unsafe extern "C" fn noop_bytes_size(_: PyObject) -> isize {
    0
}

pub unsafe extern "C" fn noop_is_subtype(_: PyObject, _: PyObject) -> c_int {
    0
}
pub unsafe extern "C" fn noop_dict_setitem_del(_: PyObject, _: PyObject) -> c_int {
    -1
}
pub unsafe extern "C" fn noop_setitem(_: PyObject, _: PyObject, _: PyObject) -> c_int {
    -1
}
pub unsafe extern "C" fn noop_save_thread() -> *mut c_void {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_restore_thread(_: *mut c_void) {}
pub unsafe extern "C" fn noop_err_print() {}
pub unsafe extern "C" fn noop_initialize() {}
pub unsafe extern "C" fn noop_finalize() {}
pub unsafe extern "C" fn noop_import(_: *const c_char) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_getattr(_: PyObject, _: *const c_char) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_setattr(_: PyObject, _: *const c_char, _: PyObject) -> c_int {
    -1
}
pub unsafe extern "C" fn noop_run_simple_string(_: *const c_char) -> c_int {
    -1
}
pub unsafe extern "C" fn noop_call(_: PyObject, _: PyObject) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_call_1(_: PyObject) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_call_kw(_: PyObject, _: PyObject, _: PyObject) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_decref(_: PyObject) {}
pub unsafe extern "C" fn noop_incref(_: PyObject) {}
pub unsafe extern "C" fn noop_as_long(_: PyObject) -> c_long {
    0
}
pub unsafe extern "C" fn noop_as_double(_: PyObject) -> c_double {
    0.0
}
pub unsafe extern "C" fn noop_as_utf8(_: PyObject) -> *const c_char {
    b"\0".as_ptr() as _
}
pub unsafe extern "C" fn noop_from_long(_: c_long) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_from_double(_: c_double) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_from_string(_: *const c_char) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_list_new(_: isize) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_list_setitem(_: PyObject, _: isize, _: PyObject) -> c_int {
    -1
}
pub unsafe extern "C" fn noop_getitem(_: PyObject, _: PyObject) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_length(_: PyObject) -> isize {
    0
}
pub unsafe extern "C" fn noop_gil_ensure() -> c_int {
    0
}
pub unsafe extern "C" fn noop_gil_release(_: c_int) {}
pub unsafe extern "C" fn noop_tuple_new(_: isize) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_tuple_setitem(_: PyObject, _: isize, _: PyObject) -> c_int {
    -1
}
pub unsafe extern "C" fn noop_dict_new() -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_dict_setitemstring(
    _: PyObject,
    _: *const c_char,
    _: PyObject,
) -> c_int {
    -1
}
pub unsafe extern "C" fn noop_err_fetch(_: *mut PyObject, _: *mut PyObject, _: *mut PyObject) {}
pub unsafe extern "C" fn noop_dict_next(
    _: PyObject,
    _: *mut isize,
    _: *mut PyObject,
    _: *mut PyObject,
) -> c_int {
    0
}
pub unsafe extern "C" fn noop_getitem_idx(_: PyObject, _: isize) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_iter_next(_: PyObject) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_get_iter(_: PyObject) -> PyObject {
    std::ptr::null_mut()
}
pub unsafe extern "C" fn noop_check_int(_: PyObject) -> c_int {
    0
}
pub unsafe extern "C" fn noop_richcomparebool(_: PyObject, _: PyObject, _: c_int) -> c_int {
    -1
}

#[cfg(target_os = "windows")]
unsafe extern "system" {
    pub fn LoadLibraryA(lpLibFileName: *const u8) -> *mut c_void;
    pub fn GetProcAddress(hModule: *mut c_void, lpProcName: *const u8) -> *mut c_void;
}
