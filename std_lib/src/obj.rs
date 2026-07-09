use crate::slab::GenSlab;
use crate::*;
use std::cell::UnsafeCell;

thread_local! {
    static OBJ_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::new(std::mem::size_of::<OliveObj>())) };
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_new() -> i64 {
    OBJ_SLAB.with(|sl| {
        let sl = unsafe { &mut *sl.get() };
        let (body, fresh) = sl.alloc();
        let o = body as *mut OliveObj;
        unsafe {
            if fresh || cfg!(debug_assertions) {
                std::ptr::write(
                    o,
                    OliveObj {
                        kind: KIND_OBJ,
                        fields: HashMap::default(),
                    },
                );
            } else {
                // Recycled slot: the cleared field map survived the free.
                (*o).kind = KIND_OBJ;
            }
        }
        body as i64
    })
}

/// Builds a dict object around an already-populated field map.
pub(crate) fn new_obj_from_map(fields: HashMap<OliveStringKey, i64>) -> i64 {
    let ptr = olive_obj_new();
    unsafe { (*(ptr as *mut OliveObj)).fields = fields };
    ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_set(obj_ptr: i64, attr: i64, val: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        panic!("Null pointer dereference: attempted to set attribute on a null object");
    }
    let kind = unsafe { *(obj_ptr as *const i64) };
    if kind == KIND_PYOBJECT {
        return python::olive_py_setattr(obj_ptr as *mut std::ffi::c_void, attr, val) as i64;
    }
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    // A tagged string key is a caller value that will be freed at its scope
    // exit; the dict keeps a private copy so its stored key never dangles.
    // Untagged attribute names are read-only interned symbols, kept as-is.
    if attr & 1 != 0 && !m.fields.contains_key(&OliveStringKey(attr)) {
        let bytes =
            unsafe { std::ffi::CStr::from_ptr((attr & !1) as *const std::ffi::c_char).to_bytes() };
        let owned = crate::string_slab::str_alloc(bytes);
        m.fields.insert(OliveStringKey(owned), val);
    } else {
        m.fields.insert(OliveStringKey(attr), val);
    }
    obj_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get(obj_ptr: i64, attr: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        panic!(
            "Null pointer dereference: attempted to get attribute from a null object or invalid attribute string"
        );
    }
    let kind = unsafe { *(obj_ptr as *const i64) };
    if kind == KIND_PYOBJECT {
        // `d["key"]` is item lookup, not attribute access; a Python dict has no
        // matching attribute. Missing keys read back as `0`, like an Olive dict.
        return python::olive_py_dict_get_default(obj_ptr, attr, 0);
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    *m.fields.get(&OliveStringKey(attr)).unwrap_or(&0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get_default(obj_ptr: i64, attr: i64, default: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        panic!(
            "Null pointer dereference: attempted to get attribute from a null object or invalid attribute string"
        );
    }
    let kind = unsafe { *(obj_ptr as *const i64) };
    if kind == KIND_PYOBJECT {
        // `.get(key, default)` is dict-lookup semantics: index by key (not
        // attribute) and fall back to `default`, matching Python's `dict.get`.
        return python::olive_py_dict_get_default(obj_ptr, attr, default);
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    *m.fields.get(&OliveStringKey(attr)).unwrap_or(&default)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_remove(obj_ptr: i64, attr: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        panic!("Null pointer dereference: attempted to remove attribute from a null object");
    }
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    match m.fields.remove_entry(&OliveStringKey(attr)) {
        Some((k, v)) => {
            if k.0 & 1 != 0 {
                crate::olive_free_str(k.0);
            }
            v
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_in_obj(key: i64, obj_ptr: i64) -> i64 {
    if obj_ptr == 0 || key == 0 {
        panic!("Null pointer dereference: attempted to check 'in' on a null object");
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    if m.fields.contains_key(&OliveStringKey(key)) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_len(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        panic!("Null pointer dereference: attempted to get length of a null object");
    }
    unsafe { (*(obj_ptr as *const OliveObj)).fields.len() as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_obj(ptr: i64) {
    if ptr == 0 || !crate::slab::ptr_in_slab_span(ptr) {
        return;
    }
    if crate::slab::slot_is_live(ptr) {
        unsafe {
            let obj = &mut *(ptr as *mut OliveObj);
            for &val in obj.fields.values() {
                if is_active_object(val) {
                    olive_free_any(val);
                }
            }
            // Tagged keys are dict-owned string copies; free them so the map's own
            // keys do not outlive it. Untagged attribute names are interned symbols.
            for k in obj.fields.keys() {
                if k.0 & 1 != 0 {
                    crate::olive_free_str(k.0);
                }
            }
            obj.fields.clear();
        }
    }
    free_obj_slot_raw(ptr);
}

pub(crate) fn free_obj_slot_raw(ptr: i64) {
    OBJ_SLAB.with(|sl| {
        unsafe { &mut *sl.get() }.free(ptr as *mut u8);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_is_obj(val: i64) -> i64 {
    if val == 0 || (val & 1) != 0 {
        return 0;
    }
    let kind = unsafe { *(val as *const i64) };
    if kind == KIND_OBJ { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_keys(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        return crate::list::list_from_vec(Vec::new());
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    crate::list::list_from_vec(m.fields.keys().map(|k| k.0).collect())
}

/// Returns a list of `[key, value]` pairs, backing `for k, v in d.items()`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_items(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        return crate::list::olive_list_new(0);
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let pairs: Vec<(i64, i64)> = m.fields.iter().map(|(k, v)| (k.0, *v)).collect();
    let outer = crate::list::olive_list_new(pairs.len() as i64);
    for (i, (k, v)) in pairs.iter().enumerate() {
        let pair = crate::list::olive_list_new(2);
        crate::list::olive_list_set(pair, 0, *k);
        crate::list::olive_list_set(pair, 1, *v);
        crate::list::olive_list_set(outer, i as i64, pair);
    }
    outer
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_values(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        return crate::list::list_from_vec(Vec::new());
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    crate::list::list_from_vec(m.fields.values().copied().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;

    fn s(text: &str) -> i64 {
        olive_str_internal(text)
    }

    fn make_obj(pairs: &[(&str, i64)]) -> i64 {
        let obj = olive_obj_new();
        for (k, v) in pairs {
            olive_obj_set(obj, s(k), *v);
        }
        obj
    }

    #[test]
    fn new_obj_creates_empty() {
        let obj = olive_obj_new();
        assert_ne!(obj, 0);
        assert_eq!(olive_obj_len(obj), 0);
    }

    #[test]
    fn set_and_get() {
        let obj = olive_obj_new();
        olive_obj_set(obj, s("key"), 42);
        assert_eq!(olive_obj_get(obj, s("key")), 42);
    }

    #[test]
    fn get_missing_key() {
        let obj = olive_obj_new();
        assert_eq!(olive_obj_get(obj, s("nonexistent")), 0);
    }

    #[test]
    fn get_default_present() {
        let obj = make_obj(&[("x", 1)]);
        assert_eq!(olive_obj_get_default(obj, s("x"), 99), 1);
    }

    #[test]
    fn get_default_missing() {
        let obj = olive_obj_new();
        assert_eq!(olive_obj_get_default(obj, s("missing"), 99), 99);
    }

    #[test]
    fn overwrite_value() {
        let obj = make_obj(&[("x", 1)]);
        olive_obj_set(obj, s("x"), 99);
        assert_eq!(olive_obj_get(obj, s("x")), 99);
    }

    #[test]
    fn remove_key() {
        let obj = make_obj(&[("a", 1), ("b", 2)]);
        let removed = olive_obj_remove(obj, s("a"));
        assert_eq!(removed, 1);
        assert_eq!(olive_obj_get(obj, s("a")), 0);
        assert_eq!(olive_obj_len(obj), 1);
    }

    #[test]
    fn remove_nonexistent() {
        let obj = make_obj(&[("x", 1)]);
        assert_eq!(olive_obj_remove(obj, s("y")), 0);
    }

    #[test]
    fn in_obj_true() {
        let obj = make_obj(&[("key", 42)]);
        assert_eq!(olive_in_obj(s("key"), obj), 1);
    }

    #[test]
    fn in_obj_false() {
        let obj = make_obj(&[("key", 42)]);
        assert_eq!(olive_in_obj(s("nope"), obj), 0);
    }

    #[test]
    fn len_multiple_fields() {
        let obj = make_obj(&[("a", 1), ("b", 2), ("c", 3)]);
        assert_eq!(olive_obj_len(obj), 3);
    }

    #[test]
    fn keys_list() {
        let obj = make_obj(&[("x", 1), ("y", 2)]);
        let keys_ptr = olive_obj_keys(obj);
        assert_ne!(keys_ptr, 0);
        let s = unsafe { &*(keys_ptr as *const StableVec) };
        assert_eq!(s.len, 2);
    }

    #[test]
    fn values_list() {
        let obj = make_obj(&[("a", 10), ("b", 20)]);
        let vals_ptr = olive_obj_values(obj);
        assert_ne!(vals_ptr, 0);
        let s = unsafe { &*(vals_ptr as *const StableVec) };
        assert_eq!(s.len, 2);
        let v0 = unsafe { *s.ptr };
        let v1 = unsafe { *s.ptr.add(1) };
        assert!((v0 == 10 && v1 == 20) || (v0 == 20 && v1 == 10));
    }

    #[test]
    fn is_obj_true() {
        let obj = olive_obj_new();
        assert_eq!(olive_is_obj(obj), 1);
    }

    #[test]
    fn is_obj_false() {
        assert_eq!(olive_is_obj(0), 0);
        assert_eq!(olive_is_obj(1), 0);
        assert_eq!(olive_is_obj(1 | 1), 0);
    }

    #[test]
    fn free_obj_no_panic() {
        let obj = make_obj(&[("x", 1)]);
        olive_free_obj(obj);

        let obj2 = olive_obj_new();
        assert_ne!(obj2, 0);
    }
}
