//! Python-backed `bytes` values: an Olive `bytes` whose buffer borrows an
//! owned, immutable `PyBytes` payload instead of a native `Vec`. Ingest of
//! an exact `PyBytes` and re-export of a backed value are both zero-copy;
//! a native buffer's second outbound crossing adopts the fresh object as
//! backing so repeated exports amortize to CPython's one mandatory copy.
//! Mutation paths realize back to a native `Vec` first (`OliveBytes::realize`),
//! so Olive value semantics never leak through the shared payload.

use crate::python::PyObject;
use crate::python::python_bindings::{
    PY_BYTES_AS_STRING, PY_BYTES_FROM_STRING_AND_SIZE, PY_BYTES_SIZE, PY_BYTES_TYPE, PY_INC_REF,
};
use crate::python::python_coerce::raw_ob_type;

/// Zero-copy ingest of an exact `PyBytes`: the value borrows the object's
/// immutable buffer and owns a fresh strong reference. GIL must be held.
/// Subclasses and other bytes-likes must keep going through the copy paths.
pub(crate) unsafe fn olive_py_bytes_wrap_exact(obj: PyObject) -> Option<i64> {
    unsafe {
        if PY_BYTES_TYPE.is_null() || raw_ob_type(obj) != PY_BYTES_TYPE {
            return None;
        }
        let buf_ptr = PY_BYTES_AS_STRING(obj);
        let len = PY_BYTES_SIZE(obj);
        if buf_ptr.is_null() || len < 0 {
            return None;
        }
        PY_INC_REF(obj);
        Some(crate::bytes::new_buf_py_backed(
            obj,
            buf_ptr as *mut u8,
            len as i64,
        ))
    }
}

/// Olive `bytes` to Python, GIL held. A Python-backed value re-exports its
/// own object for free. A native buffer pays CPython's mandatory copy; on
/// its second crossing the fresh object is adopted as backing, so repeated
/// exports of one buffer amortize to a single copy. A first-crossing value
/// keeps native backing so a later mutation stays free.
pub(crate) unsafe fn bytes_to_py(b: *mut crate::bytes::OliveBytes) -> PyObject {
    unsafe {
        let b = &mut *b;
        if !b.py.is_null() {
            PY_INC_REF(b.py);
            return b.py;
        }
        let s = b.as_slice();
        let len = s.len() as i64;
        let obj = PY_BYTES_FROM_STRING_AND_SIZE(s.as_ptr(), s.len() as isize);
        if obj.is_null() {
            return obj;
        }
        if b.exported == 0 {
            b.exported = 1;
            return obj;
        }
        let buf_ptr = PY_BYTES_AS_STRING(obj);
        if !buf_ptr.is_null() {
            PY_INC_REF(obj);
            drop(b.take_vec());
            b.ptr = buf_ptr as *mut u8;
            b.len = len;
            b.cap = 0;
            b.py = obj;
        }
        obj
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::OliveBytes;
    use crate::python::python_bindings::PY_DEC_REF;
    use crate::python::python_coerce::pyobject_slab_test_lock;
    use crate::python::{is_python_available, with_gil};

    unsafe fn refcnt(obj: PyObject) -> isize {
        unsafe { *(obj as *const isize) }
    }

    unsafe fn fresh_py_bytes(payload: &[u8]) -> PyObject {
        unsafe { PY_BYTES_FROM_STRING_AND_SIZE(payload.as_ptr(), payload.len() as isize) }
    }

    #[test]
    fn exact_bytes_ingest_shares_buffer_and_holds_one_ref() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            with_gil(|| {
                let obj = fresh_py_bytes(b"shared-payload-xyz");
                let base = refcnt(obj);
                let buf = olive_py_bytes_wrap_exact(obj).expect("exact bytes must wrap");
                let b = &*(buf as *const OliveBytes);
                assert_eq!(b.py, obj);
                assert_eq!(b.ptr as *const i8, PY_BYTES_AS_STRING(obj));
                assert_eq!(b.as_slice(), b"shared-payload-xyz");
                assert_eq!(refcnt(obj), base + 1);
                crate::bytes::olive_buf_free(buf);
                assert_eq!(refcnt(obj), base);
                PY_DEC_REF(obj);
            });
        }
    }

    #[test]
    fn backed_value_reexports_the_same_object() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            with_gil(|| {
                let obj = fresh_py_bytes(b"roundtrip-payload");
                let buf = olive_py_bytes_wrap_exact(obj).unwrap();
                let out = bytes_to_py(buf as *mut OliveBytes);
                assert_eq!(out, obj, "re-export must return the backing object");
                PY_DEC_REF(out);
                crate::bytes::olive_buf_free(buf);
                PY_DEC_REF(obj);
            });
        }
    }

    #[test]
    fn native_buffer_promotes_on_second_export_only() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            with_gil(|| {
                let buf = crate::bytes::new_buf(vec![9u8; 4096]);
                let first = bytes_to_py(buf as *mut OliveBytes);
                {
                    let b = &*(buf as *const OliveBytes);
                    assert!(b.py.is_null(), "first export must not promote");
                    assert_eq!(b.exported, 1);
                }
                let second = bytes_to_py(buf as *mut OliveBytes);
                let b = &*(buf as *const OliveBytes);
                assert_eq!(b.py, second, "second export must adopt the object");
                assert_eq!(b.as_slice(), &[9u8; 4096][..]);
                let third = bytes_to_py(buf as *mut OliveBytes);
                assert_eq!(third, second, "backed export returns the same object");
                PY_DEC_REF(first);
                PY_DEC_REF(second);
                PY_DEC_REF(third);
                crate::bytes::olive_buf_free(buf);
            });
        }
    }

    #[test]
    fn mutation_realizes_and_leaves_python_object_untouched() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let (buf, obj) = with_gil(|| {
                let obj = fresh_py_bytes(b"immutable");
                (olive_py_bytes_wrap_exact(obj).unwrap(), obj)
            });
            crate::bytes::olive_buf_set(buf, 0, b'X' as i64);
            let b = &*(buf as *const OliveBytes);
            assert!(b.py.is_null(), "mutation must detach the backing");
            assert_eq!(b.as_slice(), b"Xmmutable");
            with_gil(|| {
                let py_view = std::slice::from_raw_parts(
                    PY_BYTES_AS_STRING(obj) as *const u8,
                    PY_BYTES_SIZE(obj) as usize,
                );
                assert_eq!(py_view, b"immutable", "Python payload must stay unchanged");
                PY_DEC_REF(obj);
            });
            crate::bytes::olive_buf_free(buf);
        }
    }

    #[test]
    fn non_bytes_and_null_type_table_do_not_wrap() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            with_gil(|| {
                let five = crate::python::python_bindings::PY_LONG_FROM_LONG(5);
                assert!(olive_py_bytes_wrap_exact(five).is_none());
                PY_DEC_REF(five);
            });
        }
    }

    #[test]
    fn clone_of_backed_value_shares_payload_with_own_ref() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            with_gil(|| {
                let obj = fresh_py_bytes(b"clone-me");
                let base = refcnt(obj);
                let buf = olive_py_bytes_wrap_exact(obj).unwrap();
                let copy = crate::bytes::clone_buf(buf);
                assert_eq!(refcnt(obj), base + 2);
                let cb = &*(copy as *const OliveBytes);
                assert_eq!(cb.py, obj);
                assert_eq!(cb.as_slice(), b"clone-me");
                crate::bytes::olive_buf_free(buf);
                crate::bytes::olive_buf_free(copy);
                assert_eq!(refcnt(obj), base);
                PY_DEC_REF(obj);
            });
        }
    }
}
