//! Caches a ready, interned-name `PyTuple` per keyword-argument call site
//! (R15): the compiler packs a call's keyword names into one comma-joined
//! constant (kwarg names are always valid Python identifiers, so a comma
//! can never appear inside one), and every call sharing that constant
//! reuses the same `kwnames` tuple a vectorcall needs, instead of building
//! a fresh dict (or even a fresh tuple) on every call.

use crate::python::*;
use rustc_hash::FxHashMap;
use std::os::raw::c_char;

// Same invariant as `python_intern.rs`'s `ATTR_CACHE`: a packed name-list
// constant is a `Constant::Str` the compiler deduplicates into one static
// rodata blob, so every call site sharing a name list shares one stable
// address for the process's life -- safe to key by address alone.
// GIL-guarded, not locked: every caller sits inside `with_gil` on a path
// already gated by `use_interned_names()`, which is false in
// subinterpreter-pool mode.
static KWNAMES_CACHE: GilCell<Option<FxHashMap<usize, usize>>> = GilCell::new(None);
static KWNAMES_MRU: GilCell<(usize, usize)> = GilCell::new((0, 0));

/// Builds (or reuses) the interned-name tuple for `packed`, a comma-joined
/// C string of keyword names (`"training,verbose"`, or an empty string for
/// zero keyword arguments). Returns null if any step fails (an interning
/// call, a tuple allocation); the caller treats that exactly like a missing
/// vectorcall symbol and falls back to the dict-building path. Caller must
/// hold the process-wide GIL.
pub(crate) unsafe fn kwnames_tuple(packed: *const c_char) -> PyObject {
    let key = packed as usize;
    unsafe {
        let (mru_key, mru_val) = *KWNAMES_MRU.get();
        if mru_key == key {
            return mru_val as PyObject;
        }
        let cache = (*KWNAMES_CACHE.get()).get_or_insert_with(FxHashMap::default);
        if let Some(&cached) = cache.get(&key) {
            *KWNAMES_MRU.get() = (key, cached);
            return cached as PyObject;
        }
        let tuple = build_kwnames_tuple(packed);
        if !tuple.is_null() {
            cache.insert(key, tuple as usize);
            *KWNAMES_MRU.get() = (key, tuple as usize);
        }
        tuple
    }
}

unsafe fn build_kwnames_tuple(packed: *const c_char) -> PyObject {
    unsafe {
        let joined = std::ffi::CStr::from_ptr(packed).to_string_lossy();
        let names: Vec<&str> = if joined.is_empty() {
            Vec::new()
        } else {
            joined.split(',').collect()
        };
        let tuple = PY_TUPLE_NEW(names.len() as isize);
        if tuple.is_null() {
            return std::ptr::null_mut();
        }
        for (i, name) in names.iter().enumerate() {
            let cname = match std::ffi::CString::new(*name) {
                Ok(c) => c,
                Err(_) => {
                    PY_DEC_REF(tuple);
                    return std::ptr::null_mut();
                }
            };
            // Interns directly rather than through `interned_attr`'s
            // pointer-keyed cache: `cname` is a transient per-call heap
            // allocation, not a stable address, and the tuple this builds
            // is itself cached by `kwnames_tuple` -- a second cache layer
            // here would key off an address libc is free to reuse for the
            // next split name, aliasing two different interned strings.
            let interned = PY_UNICODE_INTERN_FROM_STRING(cname.as_ptr());
            if interned.is_null() {
                PY_DEC_REF(tuple);
                return std::ptr::null_mut();
            }
            PY_INC_REF(interned);
            PY_TUPLE_SET_ITEM(tuple, i as isize, interned);
        }
        tuple
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::python_coerce::pyobject_slab_test_lock;

    #[test]
    fn same_packed_pointer_returns_the_same_tuple() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_gil(|| unsafe {
            let packed = c"kw_stable_a,kw_stable_b";
            let a = kwnames_tuple(packed.as_ptr());
            let b = kwnames_tuple(packed.as_ptr());
            assert!(!a.is_null());
            assert_eq!(a, b, "repeated calls with the same key must alias");
            assert_eq!(PY_TUPLE_SIZE(a), 2);
        });
    }

    #[test]
    fn cache_reuses_the_tuple_across_many_calls() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_gil(|| unsafe {
            let packed = c"kw_repeat_x,kw_repeat_y,kw_repeat_z";
            let first = kwnames_tuple(packed.as_ptr());
            assert!(!first.is_null());
            for _ in 0..1_000 {
                assert_eq!(kwnames_tuple(packed.as_ptr()), first);
            }
        });
    }

    #[test]
    fn empty_names_produces_an_empty_tuple() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_gil(|| unsafe {
            let packed = c"";
            let t = kwnames_tuple(packed.as_ptr());
            assert!(!t.is_null());
            assert_eq!(PY_TUPLE_SIZE(t), 0);
        });
    }

    #[test]
    fn distinct_keys_cache_independently() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        with_gil(|| unsafe {
            let a = kwnames_tuple(c"kw_distinct_one".as_ptr());
            let b = kwnames_tuple(c"kw_distinct_two".as_ptr());
            assert!(!a.is_null() && !b.is_null());
            assert_ne!(a, b);
        });
    }
}
