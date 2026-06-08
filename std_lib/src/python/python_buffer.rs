//! Zero-copy ingest of Python buffer-protocol objects (R14). A numpy array,
//! `bytes`/`bytearray`, or `memoryview` exposes its raw storage through
//! `PyObject_GetBuffer`; when the layout matches what the compiler asked
//! for (1-D, C-contiguous, a known itemsize/format), this copies the whole
//! backing store in one `memcpy` instead of converting element by element
//! through the C-API. Anything that doesn't match returns the ineligible
//! sentinel (`0`, never a real list/bytes handle since both allocate a
//! nonnull header even when empty) and the caller falls back to the
//! existing per-element path.

use crate::python::*;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::Ordering;

pub(crate) const BUF_ELEM_INT: i64 = 1;
pub(crate) const BUF_ELEM_FLOAT: i64 = 2;

const PYBUF_FORMAT: c_int = 0x0004;
const PYBUF_ND: c_int = 0x0008;

/// Mirrors CPython's `Py_buffer` (`Include/cpython/object.h`), stable across
/// every 3.x release this targets.
#[repr(C)]
struct PyBuffer {
    buf: *mut c_void,
    obj: PyObject,
    len: isize,
    itemsize: isize,
    readonly: c_int,
    ndim: c_int,
    format: *mut c_char,
    shape: *mut isize,
    strides: *mut isize,
    suboffsets: *mut isize,
    internal: *mut c_void,
}

/// Reads `view`'s format code, ignoring an exporter's optional leading
/// byte-order/alignment prefix (`@`, `=`, `<`, `>`, `!`) -- the buffer
/// protocol doesn't require one, but nothing forbids it either.
unsafe fn format_code(view: &PyBuffer) -> u8 {
    if view.format.is_null() {
        return b'B';
    }
    unsafe {
        let mut p = view.format as *const u8;
        while matches!(*p, b'@' | b'=' | b'<' | b'>' | b'!') {
            p = p.add(1);
        }
        *p
    }
}

/// One-dimensional, matches the requested element tag, and its declared
/// length agrees with `len * itemsize`. Anything else isn't eligible.
unsafe fn eligible_len(view: &PyBuffer) -> Option<usize> {
    if view.ndim != 1 || view.buf.is_null() || view.itemsize <= 0 {
        return None;
    }
    let len = if view.shape.is_null() {
        (view.len / view.itemsize) as usize
    } else {
        unsafe { *view.shape as usize }
    };
    if (len as isize) * view.itemsize != view.len {
        return None;
    }
    Some(len)
}

/// Signed formats only: an unsigned source (`Q`/`L`/`I`) reinterpreted as
/// Olive's signed `int` would silently flip large values negative.
unsafe fn convert_int(view: &PyBuffer, len: usize) -> i64 {
    unsafe {
        match (view.itemsize, format_code(view)) {
            (8, b'q' | b'l') => {
                let list_ptr = crate::olive_list_new(len as i64);
                let sv = &mut *(list_ptr as *mut crate::StableVec);
                std::ptr::copy_nonoverlapping(view.buf as *const i64, sv.ptr, len);
                list_ptr
            }
            (4, b'i') => {
                let list_ptr = crate::olive_list_new(len as i64);
                let sv = &mut *(list_ptr as *mut crate::StableVec);
                let src = view.buf as *const i32;
                for i in 0..len {
                    *sv.ptr.add(i) = *src.add(i) as i64;
                }
                list_ptr
            }
            _ => 0,
        }
    }
}

unsafe fn convert_float(view: &PyBuffer, len: usize) -> i64 {
    unsafe {
        match (view.itemsize, format_code(view)) {
            (8, b'd') => {
                let list_ptr = crate::olive_list_new(len as i64);
                let sv = &mut *(list_ptr as *mut crate::StableVec);
                std::ptr::copy_nonoverlapping(view.buf as *const i64, sv.ptr, len);
                list_ptr
            }
            (4, b'f') => {
                let list_ptr = crate::olive_list_new(len as i64);
                let sv = &mut *(list_ptr as *mut crate::StableVec);
                let src = view.buf as *const f32;
                for i in 0..len {
                    *sv.ptr.add(i) = ((*src.add(i)) as f64).to_bits() as i64;
                }
                list_ptr
            }
            _ => 0,
        }
    }
}

/// Requests a 1-D, C-contiguous view (CPython raises `BufferError` itself
/// when the exporter can't honor that shape without also handing back
/// strides, so a successful `PyObject_GetBuffer` here already proves
/// contiguity) and runs `f` over it, releasing the view on every path.
unsafe fn with_buffer(obj: PyObject, f: impl FnOnce(&PyBuffer) -> i64) -> i64 {
    unsafe {
        if PY_OBJECT_CHECK_BUFFER(obj) == 0 {
            return 0;
        }
        let mut view: PyBuffer = std::mem::zeroed();
        let rc = PY_OBJECT_GET_BUFFER(
            obj,
            &mut view as *mut PyBuffer as *mut c_void,
            PYBUF_ND | PYBUF_FORMAT,
        );
        if rc != 0 {
            PY_ERR_CLEAR();
            return 0;
        }
        let result = f(&view);
        PY_BUFFER_RELEASE(&mut view as *mut PyBuffer as *mut c_void);
        result
    }
}

/// `PyObject -> [int]` / `[float]` zero-copy ingest. `elem_tag` is the
/// compiler's statically-known target element type (`BUF_ELEM_INT` /
/// `BUF_ELEM_FLOAT`); a buffer whose own format doesn't match that target
/// is rejected exactly like any other ineligible source; the caller must
/// not fall through and reinterpret a mismatched buffer.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_buffer_to_list(obj: PyObject, elem_tag: i64) -> i64 {
    check_python_loaded();
    if !HAS_BUFFER.load(Ordering::Relaxed) {
        return 0;
    }
    let unwrapped = unsafe { olive_py_unwrap(obj) };
    if unwrapped.is_null() {
        return 0;
    }
    with_gil(|| unsafe {
        with_buffer(unwrapped, |view| {
            let Some(len) = eligible_len(view) else {
                return 0;
            };
            match elem_tag {
                BUF_ELEM_INT => convert_int(view, len),
                BUF_ELEM_FLOAT => convert_float(view, len),
                _ => 0,
            }
        })
    })
}

/// `PyObject -> bytes` zero-copy ingest: any single-byte-itemsize buffer
/// (`bytes`, `bytearray`, `memoryview`, a `numpy.uint8` array, ...) copies
/// in one pass instead of `PyBytes_FromObject`'s own allocation followed by
/// a second copy into the Olive buffer.
pub(crate) fn buffer_to_bytes(obj: PyObject) -> i64 {
    if !HAS_BUFFER.load(Ordering::Relaxed) {
        return 0;
    }
    unsafe {
        with_buffer(obj, |view| {
            let Some(len) = eligible_len(view) else {
                return 0;
            };
            if view.itemsize != 1 {
                return 0;
            }
            let data = std::slice::from_raw_parts(view.buf as *const u8, len).to_vec();
            crate::bytes::new_buf(data)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::python_coerce::pyobject_slab_test_lock;
    use std::os::raw::c_char;

    fn with_forced_buffer<R>(want: bool, f: impl FnOnce() -> R) -> R {
        let prev = HAS_BUFFER.load(Ordering::SeqCst);
        HAS_BUFFER.store(want, Ordering::SeqCst);
        let r = f();
        HAS_BUFFER.store(prev, Ordering::SeqCst);
        r
    }

    fn numpy_array(dtype: &str, values: &str) -> Option<PyObject> {
        unsafe {
            let np = PY_IMPORT_IMPORT_MODULE(b"numpy\0".as_ptr() as *const c_char);
            if np.is_null() {
                PY_ERR_CLEAR();
                return None;
            }
            let code = format!("__import__('numpy').array([{values}], dtype='{dtype}')");
            let ccode = std::ffi::CString::new(code).unwrap();
            let builtins = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr() as *const c_char);
            let globals = PY_OBJECT_GET_ATTR_STRING(np, b"__dict__\0".as_ptr() as *const c_char);
            let eval_fn = PY_OBJECT_GET_ATTR_STRING(builtins, b"eval\0".as_ptr() as *const c_char);
            let code_obj = PY_UNICODE_FROM_STRING(ccode.as_ptr());
            let args = PY_TUPLE_NEW(2);
            PY_TUPLE_SET_ITEM(args, 0, code_obj);
            PY_INC_REF(globals);
            PY_TUPLE_SET_ITEM(args, 1, globals);
            let result = PY_OBJECT_CALL_OBJECT(eval_fn, args);
            PY_DEC_REF(args);
            PY_DEC_REF(eval_fn);
            PY_DEC_REF(np);
            if result.is_null() {
                PY_ERR_CLEAR();
                return None;
            }
            Some(result)
        }
    }

    #[test]
    fn ingests_int64_numpy_array_via_buffer() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_gil(|| unsafe {
            let Some(arr) = numpy_array("int64", "1,2,3,4,5") else {
                eprintln!("numpy not available, skipping test");
                return;
            };
            let list_ptr = olive_py_buffer_to_list(olive_py_wrap_owned(arr), BUF_ELEM_INT);
            assert_ne!(list_ptr, 0, "a contiguous int64 array must be eligible");
            let sv = &*(list_ptr as *const crate::StableVec);
            assert_eq!(sv.len, 5);
            assert_eq!(std::slice::from_raw_parts(sv.ptr, 5), &[1i64, 2, 3, 4, 5]);
            PY_DEC_REF(arr);
        });
    }

    #[test]
    fn ingests_float64_numpy_array_via_buffer() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_gil(|| unsafe {
            let Some(arr) = numpy_array("float64", "1.5,2.5,3.5") else {
                eprintln!("numpy not available, skipping test");
                return;
            };
            let list_ptr = olive_py_buffer_to_list(olive_py_wrap_owned(arr), BUF_ELEM_FLOAT);
            assert_ne!(list_ptr, 0, "a contiguous float64 array must be eligible");
            let sv = &*(list_ptr as *const crate::StableVec);
            assert_eq!(sv.len, 3);
            let vals: Vec<f64> = std::slice::from_raw_parts(sv.ptr, 3)
                .iter()
                .map(|&w| f64::from_bits(w as u64))
                .collect();
            assert_eq!(vals, vec![1.5, 2.5, 3.5]);
            PY_DEC_REF(arr);
        });
    }

    #[test]
    fn ingests_int32_numpy_array_by_widening() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_gil(|| unsafe {
            let Some(arr) = numpy_array("int32", "10,20,30") else {
                eprintln!("numpy not available, skipping test");
                return;
            };
            let list_ptr = olive_py_buffer_to_list(olive_py_wrap_owned(arr), BUF_ELEM_INT);
            assert_ne!(list_ptr, 0, "a contiguous int32 array must be eligible");
            let sv = &*(list_ptr as *const crate::StableVec);
            assert_eq!(std::slice::from_raw_parts(sv.ptr, 3), &[10i64, 20, 30]);
            PY_DEC_REF(arr);
        });
    }

    #[test]
    fn ingests_float32_numpy_array_by_widening() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_gil(|| unsafe {
            let Some(arr) = numpy_array("float32", "1.0,2.0") else {
                eprintln!("numpy not available, skipping test");
                return;
            };
            let list_ptr = olive_py_buffer_to_list(olive_py_wrap_owned(arr), BUF_ELEM_FLOAT);
            assert_ne!(list_ptr, 0, "a contiguous float32 array must be eligible");
            let sv = &*(list_ptr as *const crate::StableVec);
            let vals: Vec<f64> = std::slice::from_raw_parts(sv.ptr, 2)
                .iter()
                .map(|&w| f64::from_bits(w as u64))
                .collect();
            assert_eq!(vals, vec![1.0, 2.0]);
            PY_DEC_REF(arr);
        });
    }

    #[test]
    fn non_contiguous_slice_is_not_eligible() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_gil(|| unsafe {
            let Some(arr) = numpy_array("int64", "1,2,3,4,5,6") else {
                eprintln!("numpy not available, skipping test");
                return;
            };
            let handle = olive_py_wrap_owned(arr);
            let step = PY_SLICE_NEW(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                PY_LONG_FROM_LONG(2),
            );
            let sliced = PY_OBJECT_GET_ITEM(arr, step);
            PY_DEC_REF(step);
            assert!(!sliced.is_null(), "numpy slicing must succeed");
            let list_ptr = olive_py_buffer_to_list(olive_py_wrap_owned(sliced), BUF_ELEM_INT);
            assert_eq!(
                list_ptr, 0,
                "a non-contiguous view must fall through, never misread strided memory"
            );
            PY_DEC_REF(sliced);
            olive_py_decref(handle);
        });
    }

    #[test]
    fn wrong_elem_tag_is_rejected_not_reinterpreted() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_gil(|| unsafe {
            let Some(arr) = numpy_array("float64", "1.0,2.0") else {
                eprintln!("numpy not available, skipping test");
                return;
            };
            let handle = olive_py_wrap_owned(arr);
            let list_ptr = olive_py_buffer_to_list(handle, BUF_ELEM_INT);
            assert_eq!(
                list_ptr, 0,
                "a float64 buffer must never be read back as int"
            );
            olive_py_decref(handle);
        });
    }

    #[test]
    fn plain_python_list_is_not_buffer_eligible() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let list_obj = with_gil(|| {
                let l = PY_LIST_NEW(3);
                PY_LIST_SET_ITEM(l, 0, PY_LONG_FROM_LONG(1));
                PY_LIST_SET_ITEM(l, 1, PY_LONG_FROM_LONG(2));
                PY_LIST_SET_ITEM(l, 2, PY_LONG_FROM_LONG(3));
                olive_py_wrap_owned(l)
            });
            let list_ptr = olive_py_buffer_to_list(list_obj, BUF_ELEM_INT);
            assert_eq!(
                list_ptr, 0,
                "a plain python list doesn't support the buffer protocol"
            );
            olive_py_decref(list_obj);
        }
    }

    #[test]
    fn bytes_ingest_matches_bytes_from_object() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let data = b"hello world";
            let bytes_obj =
                with_gil(|| PY_BYTES_FROM_STRING_AND_SIZE(data.as_ptr(), data.len() as isize));
            let fast = with_gil(|| buffer_to_bytes(bytes_obj));
            assert_ne!(fast, 0, "a bytes object must be buffer-eligible");
            let ob = &*(fast as *const crate::bytes::OliveBytes);
            assert_eq!(ob.as_slice(), data);
            with_gil(|| PY_DEC_REF(bytes_obj));
        }
    }

    #[test]
    fn disabled_flag_falls_through_cleanly() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_forced_buffer(false, || unsafe {
            let data = b"xyz";
            let bytes_obj =
                with_gil(|| PY_BYTES_FROM_STRING_AND_SIZE(data.as_ptr(), data.len() as isize));
            assert_eq!(
                buffer_to_bytes(bytes_obj),
                0,
                "with HAS_BUFFER off the fast path must never fire"
            );
            with_gil(|| PY_DEC_REF(bytes_obj));
        });
    }
}
