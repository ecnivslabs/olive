use crate::python::*;
use std::os::raw::{c_char, c_double, c_long};
use std::sync::atomic::Ordering;

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_int(v: i64) -> PyObject {
    check_python_loaded();
    with_gil(|| unsafe {
        let r = PY_LONG_FROM_LONG(v as c_long);
        olive_py_wrap_owned(r)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_float(v: f64) -> PyObject {
    check_python_loaded();
    with_gil(|| unsafe {
        let r = PY_FLOAT_FROM_DOUBLE(v as c_double);
        olive_py_wrap_owned(r)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_from_str(s: i64) -> PyObject {
    check_python_loaded();
    with_gil(|| unsafe {
        let r = olive_str_to_py(s);
        if r.is_null() {
            handle_py_error();
        }
        olive_py_wrap_owned(r)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_conv_to_py(val: i64) -> PyObject {
    check_python_loaded();
    with_gil(|| unsafe { olive_to_py_checked(val) })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_conv_to_olive(py_val: PyObject) -> i64 {
    check_python_loaded();
    with_gil(|| unsafe { py_to_olive_internal(py_val) })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_eq(l: PyObject, r: PyObject) -> i64 {
    check_python_loaded();
    let un_l = unsafe { olive_py_unwrap(l) };
    let un_r = unsafe { olive_py_unwrap(r) };
    if un_l.is_null() || un_r.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        let res = PY_OBJECT_RICHCOMPAREBOOL(un_l, un_r, 2);
        if res == -1 {
            PY_ERR_CLEAR();
            0
        } else {
            res as i64
        }
    })
}

fn py_richcmp(l: PyObject, r: PyObject, op: std::ffi::c_int) -> i64 {
    check_python_loaded();
    let un_l = unsafe { olive_py_unwrap(l) };
    let un_r = unsafe { olive_py_unwrap(r) };
    if un_l.is_null() || un_r.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        let res = PY_OBJECT_RICHCOMPAREBOOL(un_l, un_r, op);
        if res == -1 {
            PY_ERR_CLEAR();
            0
        } else {
            res as i64
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_lt(l: PyObject, r: PyObject) -> i64 {
    py_richcmp(l, r, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_le(l: PyObject, r: PyObject) -> i64 {
    py_richcmp(l, r, 1)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_gt(l: PyObject, r: PyObject) -> i64 {
    py_richcmp(l, r, 4)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_ge(l: PyObject, r: PyObject) -> i64 {
    py_richcmp(l, r, 5)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_ne(l: PyObject, r: PyObject) -> i64 {
    py_richcmp(l, r, 3)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_copy_ref(arena_ptr: PyObject) -> PyObject {
    if arena_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let raw_py = unsafe { olive_py_unwrap(arena_ptr) };
    if raw_py.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { olive_py_wrap_borrowed(raw_py) }
}

/// Converts an already-unwrapped, live Python object into an `i64`, the
/// inner logic `olive_py_to_int` wraps with the unwrap/GIL boundary. Shared
/// with the fused call-result path (`python_ret.rs`), which already holds
/// the GIL and a raw (never-wrapped) object when it needs this conversion.
pub(crate) unsafe fn raw_py_to_int(raw: PyObject) -> i64 {
    unsafe {
        let int_obj = PY_NUMBER_LONG(raw);
        if int_obj.is_null() {
            PY_ERR_CLEAR();
            crate::panic::abort_py_coerce("cannot convert this Python value to an integer");
        }
        let result = PY_LONG_AS_LONG(int_obj);
        PY_DEC_REF(int_obj);
        if !PY_ERR_OCCURRED().is_null() {
            PY_ERR_CLEAR();
            crate::panic::abort_py_coerce("cannot convert this Python value to an integer");
        }
        // c_long is 32 bits on Windows (LLP64), 64 on Unix -- this cast is a
        // no-op there but a real widen on Windows.
        #[allow(clippy::unnecessary_cast)]
        {
            result as i64
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
    with_gil(|| unsafe { raw_py_to_int(unwrapped_obj) })
}

/// See `raw_py_to_int`; the float counterpart `olive_py_to_float` wraps.
pub(crate) unsafe fn raw_py_to_float(raw: PyObject) -> f64 {
    unsafe {
        let result = PY_FLOAT_AS_DOUBLE(raw);
        if result == -1.0 && !PY_ERR_OCCURRED().is_null() {
            PY_ERR_CLEAR();
            crate::panic::abort_py_coerce("cannot convert this Python value to a float");
        }
        result
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_float(obj: PyObject) -> f64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0.0;
    }
    with_gil(|| unsafe { raw_py_to_float(unwrapped_obj) })
}

/// See `raw_py_to_int`; the string counterpart `olive_py_to_str` wraps.
pub(crate) unsafe fn raw_py_to_str(raw: PyObject) -> i64 {
    unsafe {
        let str_obj = PY_OBJECT_STR(raw);
        if str_obj.is_null() {
            handle_py_error();
        }
        let r = py_str_to_olive(str_obj);
        PY_DEC_REF(str_obj);
        if r == 0 {
            PY_ERR_CLEAR();
            crate::panic::abort_py_coerce("cannot encode this Python string as UTF-8");
        }
        r
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_str(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    with_gil(|| unsafe { raw_py_to_str(unwrapped_obj) })
}

/// Materializes a Python bytes-like object into a native buffer; non-bytes-like raises.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_bytes(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return crate::bytes::new_buf(Vec::new());
    }
    let fast = crate::python::python_buffer::buffer_to_bytes(unwrapped_obj);
    if fast != 0 {
        return fast;
    }
    with_gil(|| unsafe {
        let bytes_obj = PY_BYTES_FROM_OBJECT(unwrapped_obj);
        if bytes_obj.is_null() {
            handle_py_error();
            return crate::bytes::new_buf(Vec::new());
        }
        let ptr = PY_BYTES_AS_STRING(bytes_obj);
        let len = PY_BYTES_SIZE(bytes_obj);
        let data = if ptr.is_null() || len <= 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(ptr as *const u8, len as usize).to_vec()
        };
        PY_DEC_REF(bytes_obj);
        crate::bytes::new_buf(data)
    })
}

/// `[T]` target with a concrete native `T`: elements land as raw native words.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_list(obj: PyObject, elem_tag: i64) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    if elem_tag != 0 {
        let fast = crate::python::python_buffer::olive_py_buffer_to_list(obj, elem_tag);
        if fast != 0 {
            return fast;
        }
    }
    with_gil(|| unsafe { olive_py_to_list_internal(unwrapped_obj, false) })
}

/// `[Any]` target: elements are boxed so a nested float/int/bool/null reads
/// back correctly instead of colliding with the Any tag bits.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_any_list(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    with_gil(|| unsafe { olive_py_to_list_internal(unwrapped_obj, true) })
}

/// `{str: T}` target with a concrete native `T`: values land as raw native words.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_dict(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    with_gil(|| unsafe { olive_py_to_dict_internal(unwrapped_obj, false) })
}

/// `{str: Any}` target: values are boxed so a nested float/int/bool/null reads
/// back correctly instead of colliding with the Any tag bits.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_any_dict(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    with_gil(|| unsafe { olive_py_to_dict_internal(unwrapped_obj, true) })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getitem(obj: PyObject, key: PyObject) -> PyObject {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    with_gil(|| unsafe {
        let py_key = olive_to_py_checked(key as i64);
        let r = PY_OBJECT_GET_ITEM(unwrapped_obj, py_key);
        PY_DEC_REF(py_key);
        if r.is_null() {
            handle_py_error();
        }
        olive_py_wrap_owned(r)
    })
}

/// Converts an Olive Any value into a Python object handle. Inverse of py_to_any_internal.
#[unsafe(no_mangle)]
pub extern "C" fn olive_to_pyobject(val: i64) -> i64 {
    check_python_loaded();
    with_gil(|| unsafe { olive_py_wrap_owned(olive_any_to_py_checked(val)) as i64 })
}

/// Materializes a Python arena handle into a self-describing Any value
/// (scalars boxed, strings/lists/dicts in Olive form).
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_to_any(obj: i64) -> i64 {
    check_python_loaded();
    let raw = unsafe { olive_py_unwrap(obj as PyObject) };
    if raw.is_null() {
        return crate::boxed::olive_box_null();
    }
    with_gil(|| unsafe { py_to_any_internal(raw) })
}

/// dict.get(key, default) for Python objects. Looks up by item, returns default on miss.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_dict_get_default(obj: i64, key: i64, default: i64) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj as PyObject) };
    if unwrapped_obj.is_null() {
        return default;
    }
    with_gil(|| unsafe {
        let py_key = olive_to_py_checked(key);
        let val = PY_OBJECT_GET_ITEM(unwrapped_obj, py_key);
        PY_DEC_REF(py_key);
        if val.is_null() {
            PY_ERR_CLEAR();
            return default;
        }
        // Result lands in an Any slot, so box scalars.
        let result = py_to_any_internal(val);
        PY_DEC_REF(val);
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setitem(obj: PyObject, key: PyObject, val: PyObject) {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return;
    }
    with_gil(|| unsafe {
        let py_key = olive_to_py_checked(key as i64);
        let py_val = olive_to_py_checked(val as i64);
        let res = PY_OBJECT_SET_ITEM(unwrapped_obj, py_key, py_val);
        PY_DEC_REF(py_key);
        PY_DEC_REF(py_val);
        if res == -1 {
            handle_py_error();
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getitem_int(obj: PyObject, key: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    with_gil(|| unsafe {
        let py_key = PY_LONG_FROM_LONG(key as std::os::raw::c_long);
        if py_key.is_null() {
            return std::ptr::null_mut();
        }
        let r = PY_OBJECT_GET_ITEM(unwrapped_obj, py_key);
        PY_DEC_REF(py_key);
        if r.is_null() {
            handle_py_error();
        }
        olive_py_wrap_owned(r)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setitem_int(obj: PyObject, key: i64, val: PyObject) {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return;
    }
    with_gil(|| unsafe {
        let py_key = PY_LONG_FROM_LONG(key as std::os::raw::c_long);
        if py_key.is_null() {
            handle_py_error();
        }
        let py_val = olive_to_py_checked(val as i64);
        let res = PY_OBJECT_SET_ITEM(unwrapped_obj, py_key, py_val);
        PY_DEC_REF(py_key);
        PY_DEC_REF(py_val);
        if res == -1 {
            handle_py_error();
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_len(obj: PyObject) -> i64 {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return 0;
    }
    with_gil(|| unsafe { PY_OBJECT_LENGTH(unwrapped_obj) as i64 })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_none() -> PyObject {
    check_python_loaded();
    unsafe { olive_py_wrap_borrowed(_PY_NONE_STRUCT) }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_is_none(obj: PyObject) -> i64 {
    check_python_loaded();
    if (obj as i64) == 0 {
        return 1;
    }
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() || unwrapped_obj == unsafe { _PY_NONE_STRUCT } {
        1
    } else {
        0
    }
}

/// Whether `val` is a live Python arena handle. Lets codegen dispatch a
/// mixed py/native union to the realize path only when the value really
/// is a Python object.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_is_handle(val: i64) -> i64 {
    if val == 0 {
        return 0;
    }
    if crate::python::python_coerce::is_arena_ptr(val as usize) {
        1
    } else {
        0
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
            *sv.ptr.add(i) = k.0;
        }
        list_ptr
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_import(name: i64) -> PyObject {
    check_python_loaded();
    with_gil(|| unsafe {
        let m = PY_IMPORT_IMPORT_MODULE((name & !1) as *const c_char);
        if m.is_null() {
            handle_py_error();
        }
        olive_py_wrap_owned(m)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getattr(obj: PyObject, attr: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return std::ptr::null_mut();
    }
    let attr_ptr = (attr & !1) as *const c_char;
    with_gil(|| unsafe {
        let a = if HAS_INTERN.load(Ordering::Relaxed) {
            let name = interned_attr(attr_ptr);
            if name.is_null() {
                std::ptr::null_mut()
            } else {
                PY_OBJECT_GET_ATTR(unwrapped_obj, name)
            }
        } else {
            PY_OBJECT_GET_ATTR_STRING(unwrapped_obj, attr_ptr)
        };
        if a.is_null() {
            handle_py_error();
        }
        olive_py_wrap_owned(a)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_setattr(obj: PyObject, attr: i64, val: i64) -> PyObject {
    check_python_loaded();
    let unwrapped_obj = unsafe { olive_py_unwrap(obj) };
    if unwrapped_obj.is_null() {
        return obj;
    }
    let attr_ptr = (attr & !1) as *const c_char;
    with_gil(|| unsafe {
        let py_val = olive_to_py_checked(val);
        let res = if HAS_INTERN.load(Ordering::Relaxed) {
            let name = interned_attr(attr_ptr);
            if name.is_null() {
                -1
            } else {
                PY_OBJECT_SET_ATTR(unwrapped_obj, name, py_val)
            }
        } else {
            PY_OBJECT_SET_ATTR_STRING(unwrapped_obj, attr_ptr, py_val)
        };
        if res == -1 {
            handle_py_error();
        }
        PY_DEC_REF(py_val);
        obj
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_bitor(l: PyObject, r: PyObject) -> i64 {
    check_python_loaded();
    let un_l = unsafe { olive_py_unwrap(l) };
    let un_r = unsafe { olive_py_unwrap(r) };
    if un_l.is_null() || un_r.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        let res = crate::python::PY_NUMBER_OR(un_l, un_r);
        if res.is_null() {
            crate::python::python_error::handle_py_error();
        }
        olive_py_wrap_owned(res) as i64
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_getslice(
    obj_handle: i64,
    start_val: i64,
    stop_val: i64,
    step_val: i64,
    flags: i64,
) -> i64 {
    check_python_loaded();
    let obj = unsafe { olive_py_unwrap(obj_handle as PyObject) };
    if obj.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        let py_none = _PY_NONE_STRUCT;
        let py_start = if flags & 1 != 0 {
            PY_LONG_FROM_LONG(start_val as std::os::raw::c_long)
        } else {
            PY_INC_REF(py_none);
            py_none
        };
        let py_stop = if flags & 2 != 0 {
            PY_LONG_FROM_LONG(stop_val as std::os::raw::c_long)
        } else {
            PY_INC_REF(py_none);
            py_none
        };
        let py_step = if flags & 4 != 0 {
            PY_LONG_FROM_LONG(step_val as std::os::raw::c_long)
        } else {
            PY_INC_REF(py_none);
            py_none
        };
        let slice = PY_SLICE_NEW(py_start, py_stop, py_step);
        PY_DEC_REF(py_start);
        PY_DEC_REF(py_stop);
        PY_DEC_REF(py_step);
        if slice.is_null() {
            handle_py_error();
            return 0;
        }
        let result = PY_OBJECT_GET_ITEM(obj, slice);
        PY_DEC_REF(slice);
        if result.is_null() {
            handle_py_error();
            return 0;
        }
        olive_py_wrap_owned(result) as i64
    })
}

/// R18: `PyUnicode_FromStringAndSize`/`PyUnicode_AsUTF8AndSize` single-pass
/// string crossing, both directions, and its embedded-NUL fix.
#[cfg(test)]
mod str_and_size_tests {
    use super::*;
    use crate::python::python_coerce::{olive_str_to_py, py_str_to_olive, pyobject_slab_test_lock};

    fn with_forced_str_and_size<R>(want: bool, f: impl FnOnce() -> R) -> R {
        let prev = HAS_STR_AND_SIZE.load(Ordering::SeqCst);
        HAS_STR_AND_SIZE.store(want, Ordering::SeqCst);
        let r = f();
        HAS_STR_AND_SIZE.store(prev, Ordering::SeqCst);
        r
    }

    #[test]
    fn multibyte_cjk_and_emoji_round_trip_py_to_olive_to_py() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_forced_str_and_size(true, || unsafe {
            let text = "héllo 世界 🎉 — done";
            let c = std::ffi::CString::new(text).unwrap();
            let py_str = with_gil(|| PY_UNICODE_FROM_STRING(c.as_ptr()));
            let olive_str = with_gil(|| py_str_to_olive(py_str));
            assert_eq!(crate::olive_str_from_ptr(olive_str), text);

            let py_back = with_gil(|| olive_str_to_py(olive_str));
            let back_len = with_gil(|| PY_OBJECT_LENGTH(py_back));
            assert_eq!(back_len as usize, text.chars().count());

            let back_olive = with_gil(|| py_str_to_olive(py_back));
            assert_eq!(crate::olive_str_from_ptr(back_olive), text);

            with_gil(|| {
                PY_DEC_REF(py_str);
                PY_DEC_REF(py_back);
            });
            crate::string_slab::str_free(olive_str);
            crate::string_slab::str_free(back_olive);
        });
    }

    #[test]
    fn embedded_nul_copies_through_intact_py_to_olive_to_py() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_forced_str_and_size(true, || unsafe {
            let bytes: &[u8] = b"ab\0cd";
            let py_str = with_gil(|| {
                PY_UNICODE_FROM_STRING_AND_SIZE(
                    bytes.as_ptr() as *const c_char,
                    bytes.len() as isize,
                )
            });
            assert!(
                !py_str.is_null(),
                "PyUnicode_FromStringAndSize must be available for this test"
            );

            let olive_str = with_gil(|| py_str_to_olive(py_str));
            assert_eq!(
                crate::olive_str_to_bytes(olive_str),
                bytes,
                "embedded NUL must survive py->olive"
            );

            let py_back = with_gil(|| olive_str_to_py(olive_str));
            let back_len = with_gil(|| PY_OBJECT_LENGTH(py_back));
            assert_eq!(
                back_len, 5,
                "embedded NUL must not truncate olive->py either"
            );

            with_gil(|| {
                PY_DEC_REF(py_str);
                PY_DEC_REF(py_back);
            });
            crate::string_slab::str_free(olive_str);
        });
    }

    #[test]
    fn empty_string_round_trips_both_directions() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_forced_str_and_size(true, || unsafe {
            let py_str = with_gil(|| PY_UNICODE_FROM_STRING(b"\0".as_ptr() as *const c_char));
            let olive_str = with_gil(|| py_str_to_olive(py_str));
            assert_eq!(crate::olive_str_from_ptr(olive_str), "");

            let empty_olive = crate::olive_str_internal("");
            let py_back = with_gil(|| olive_str_to_py(empty_olive));
            let back_len = with_gil(|| PY_OBJECT_LENGTH(py_back));
            assert_eq!(back_len, 0);

            with_gil(|| {
                PY_DEC_REF(py_str);
                PY_DEC_REF(py_back);
            });
            crate::string_slab::str_free(olive_str);
            crate::string_slab::str_free(empty_olive);
        });
    }

    /// Missing symbols must keep the old strlen-based path working exactly
    /// as before (embedded NUL still truncates there -- that limitation is
    /// unchanged, only the fast path fixes it).
    #[test]
    fn fallback_path_still_works_when_str_and_size_forced_off() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_forced_str_and_size(false, || unsafe {
            let py_str =
                with_gil(|| PY_UNICODE_FROM_STRING(b"plain ascii\0".as_ptr() as *const c_char));
            let olive_str = with_gil(|| py_str_to_olive(py_str));
            assert_eq!(crate::olive_str_from_ptr(olive_str), "plain ascii");

            let py_back = with_gil(|| olive_str_to_py(olive_str));
            let back_len = with_gil(|| PY_OBJECT_LENGTH(py_back));
            assert_eq!(back_len, 11);

            with_gil(|| {
                PY_DEC_REF(py_str);
                PY_DEC_REF(py_back);
            });
            crate::string_slab::str_free(olive_str);
        });
    }

    #[test]
    fn public_ffi_round_trips_cleanly_100k_times_no_leak() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        let s = crate::olive_str_internal("tokenizer-shaped-string");
        for _ in 0..100_000 {
            let handle = olive_py_from_str(s);
            assert!(!handle.is_null());
            assert!(crate::is_active_object(handle as i64));
            let back = olive_py_to_str(handle);
            assert_eq!(crate::olive_str_from_ptr(back), "tokenizer-shaped-string");
            crate::string_slab::str_free(back);
            olive_py_decref(handle);
            assert!(!crate::is_active_object(handle as i64));
        }
        crate::string_slab::str_free(s);
    }

    #[test]
    fn py_str_to_olive_never_moves_the_source_refcount() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let shared = with_gil(|| {
                PY_UNICODE_FROM_STRING(b"shared-refcount-probe\0".as_ptr() as *const c_char)
            });
            let baseline = with_gil(|| *(shared as *const isize));
            for _ in 0..100_000 {
                let r = with_gil(|| py_str_to_olive(shared));
                crate::string_slab::str_free(r);
            }
            let after = with_gil(|| *(shared as *const isize));
            assert_eq!(
                after, baseline,
                "py_str_to_olive must be a pure borrow, no refcount drift"
            );
            with_gil(|| PY_DEC_REF(shared));
        }
    }
}
