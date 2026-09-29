//! Caches one interned Python `str` per attribute name, so repeated
//! `obj.attr` access reuses the same name object instead of rebuilding it
//! from the C string on every call. Keyed by the name's bytes: callers pass
//! pointers with no lifetime guarantee (a stack buffer works in tests and
//! would alias a later, different name at the same address), so an address
//! key can return another name's object.

use crate::python::*;
use rustc_hash::FxHashMap;
use std::ffi::CStr;
use std::os::raw::c_char;

// Values are `PyObject` cast to `usize`. GIL-guarded rather than locked:
// every caller sits inside `with_gil`, and interning is bypassed entirely
// in subinterpreter-pool mode (an interned object belongs to one
// interpreter and must not leak into another), so the process-wide GIL
// serializes all access. `MRU` short-circuits the map probe for the loop
// case where one hot site repeats the same name.
static ATTR_CACHE: GilCell<Option<FxHashMap<Vec<u8>, usize>>> = GilCell::new(None);
static ATTR_MRU: GilCell<Option<(Vec<u8>, usize)>> = GilCell::new(None);

/// Whether the interned-name fast path may run: the intern/getattr symbols
/// exist and one process-wide GIL guards the caches. Pool mode always takes
/// the string-based paths -- an interned object is per-interpreter and the
/// GIL no longer serializes the cache.
#[inline]
pub(crate) fn use_interned_names() -> bool {
    HAS_INTERN.load(std::sync::atomic::Ordering::Relaxed) && crate::python::gil_process_wide()
}

/// Interns `attr` into a persistent Python `str`, reusing it across every
/// access that shares the same literal pointer. Returns null if
/// `PyUnicode_InternFromString` itself fails -- clearing the error indicator,
/// since callers treat null here as "no fast path, take the string-based
/// fallback", not as a pending exception. The interned object is never
/// decref'd -- like a Python module's own compiled string constants, it
/// lives for the process, not for one call. Caller must hold the
/// process-wide GIL.
pub(crate) unsafe fn interned_attr(attr: *const c_char) -> PyObject {
    unsafe {
        let bytes = CStr::from_ptr(attr).to_bytes();
        if let Some((mru_key, mru_val)) = &*ATTR_MRU.get() {
            if mru_key.as_slice() == bytes {
                return *mru_val as PyObject;
            }
        }
        let cache = (*ATTR_CACHE.get()).get_or_insert_with(FxHashMap::default);
        if let Some(&cached) = cache.get(bytes) {
            *ATTR_MRU.get() = Some((bytes.to_vec(), cached));
            return cached as PyObject;
        }
        let interned = PY_UNICODE_INTERN_FROM_STRING(attr);
        if !interned.is_null() {
            cache.insert(bytes.to_vec(), interned as usize);
            *ATTR_MRU.get() = Some((bytes.to_vec(), interned as usize));
        } else if !PY_ERR_OCCURRED().is_null() {
            PY_ERR_CLEAR();
        }
        interned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::python_coerce::{pyobject_slab_test_lock, static_attr_name};
    use std::sync::atomic::Ordering;

    fn with_forced_intern<R>(want: bool, f: impl FnOnce() -> R) -> R {
        let prev = HAS_INTERN.load(Ordering::SeqCst);
        HAS_INTERN.store(want, Ordering::SeqCst);
        let r = f();
        HAS_INTERN.store(prev, Ordering::SeqCst);
        r
    }

    #[test]
    fn same_pointer_returns_the_same_interned_object() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let name = b"__t_intern_stable\0";
            let ptr = name.as_ptr() as *const c_char;
            let a = with_gil(|| interned_attr(ptr));
            let b = with_gil(|| interned_attr(ptr));
            assert!(!a.is_null());
            assert_eq!(a, b, "repeated interning of the same pointer must alias");
        }
    }

    #[test]
    fn distinct_names_cache_independently_under_concurrent_access() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        // Every C-string is `'static` (the literal outlives the whole test),
        // so sharing pointers across threads here is sound.
        static NAMES: [&[u8]; 4] = [
            b"__t_intern_a\0",
            b"__t_intern_b\0",
            b"__t_intern_c\0",
            b"__t_intern_d\0",
        ];
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| unsafe {
                    for name in NAMES {
                        let ptr = name.as_ptr() as *const c_char;
                        for _ in 0..1000 {
                            let obj = with_gil(|| interned_attr(ptr));
                            assert!(!obj.is_null());
                        }
                    }
                });
            }
        });
    }

    #[test]
    fn attr_read_via_interned_path_matches_getattrstring_result() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let obj = with_gil(|| {
                let d = PY_DICT_NEW();
                olive_py_wrap_owned(d)
            });
            let attr_ptr = static_attr_name("clear");

            let via_intern = with_forced_intern(true, || {
                crate::python::python_coerce_ffi::olive_py_getattr(obj, attr_ptr)
            });
            let via_legacy = with_forced_intern(false, || {
                crate::python::python_coerce_ffi::olive_py_getattr(obj, attr_ptr)
            });
            assert!(!via_intern.is_null());
            assert!(!via_legacy.is_null());
            let both_callable = with_gil(|| {
                let i = olive_py_unwrap(via_intern);
                let l = olive_py_unwrap(via_legacy);
                let ity = PY_OBJECT_TYPE(i);
                let lty = PY_OBJECT_TYPE(l);
                ity == lty
            });
            assert!(
                both_callable,
                "interned and legacy getattr must resolve to the same kind of object"
            );
            olive_py_decref(via_intern);
            olive_py_decref(via_legacy);
            olive_py_decref(obj);
        }
    }
}
