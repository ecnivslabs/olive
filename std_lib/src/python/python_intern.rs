//! Caches one interned Python `str` per attribute-name literal, so repeated
//! `obj.attr` access reuses the same name object instead of rebuilding it
//! from the C string on every call. Keyed by the literal's own pointer:
//! `Constant::Str` operands are deduplicated per unique string into one
//! static data blob at compile time (`codegen/cranelift/setup/strings.rs`),
//! so every use of the same source-level attribute name shares one stable
//! address for the life of the process -- safe to use as a cache key without
//! ever hashing or comparing the bytes.

use crate::python::*;
use rustc_hash::FxHashMap;
use std::os::raw::c_char;
use std::sync::RwLock;

// Values are `PyObject` cast to `usize`: a raw pointer isn't `Send`/`Sync`,
// so it can't sit in a shared static directly. Cast back at the single read
// site below.
static ATTR_CACHE: std::sync::LazyLock<RwLock<FxHashMap<usize, usize>>> =
    std::sync::LazyLock::new(|| RwLock::new(FxHashMap::default()));

/// Interns `attr` into a persistent Python `str`, reusing it across every
/// access that shares the same literal pointer. Returns null if
/// `PyUnicode_InternFromString` itself fails (caller treats it like any
/// other null from a C-API call). The interned object is never decref'd --
/// like a Python module's own compiled string constants, it lives for the
/// process, not for one call.
pub(crate) unsafe fn interned_attr(attr: *const c_char) -> PyObject {
    let key = attr as usize;
    if let Some(&cached) = ATTR_CACHE.read().unwrap().get(&key) {
        return cached as PyObject;
    }
    let mut cache = ATTR_CACHE.write().unwrap();
    if let Some(&cached) = cache.get(&key) {
        return cached as PyObject;
    }
    let interned = unsafe { PY_UNICODE_INTERN_FROM_STRING(attr) };
    if !interned.is_null() {
        cache.insert(key, interned as usize);
    }
    interned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::python_coerce::pyobject_slab_test_lock;
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
            let attr_ptr = crate::olive_str_internal("clear") | 1;

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
