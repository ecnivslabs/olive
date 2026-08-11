use crate::*;

fn index_type_error(loc: i64) -> ! {
    let location = (loc != 0).then(|| olive_str_from_ptr(loc));
    panic::abort("value does not support indexing", location.as_deref());
}

fn slice_type_error() -> ! {
    // Slices carry no source location in the call convention (matching the
    // typed getslice entry points), so the fault names the problem without
    // a caret. Mirrors `index_type_error` for the slice shape.
    panic::abort("value does not support slicing", None);
}

/// Runtime-dispatch slice for a statically-`Any` object. The builder used
/// to route every `Any` slice to the Python slicer, segfaulting on native
/// values (a string in an `Any` slot, an enum payload read). Dispatches on
/// the value's own representation like `olive_get_index_any` does; anything
/// without a slice shape faults cleanly instead of dereferencing.
#[unsafe(no_mangle)]
pub extern "C" fn olive_getslice_any(
    obj: i64,
    start: i64,
    stop: i64,
    step: i64,
    flags: i64,
) -> i64 {
    if obj == 0 || obj & boxed::TAG_MASK == boxed::TAG_NULL {
        slice_type_error();
    }
    if matches!(obj & boxed::TAG_MASK, boxed::TAG_INT | boxed::TAG_BOOL) {
        slice_type_error();
    }
    if obj & 1 != 0 {
        return string::olive_str_getslice(obj, start, stop, step, flags);
    }
    if !is_active_object(obj) {
        // Untagged interned chars (`s[i]`) are static one-character
        // strings, not heap objects: slice their single byte directly.
        if crate::string::is_interned_char(obj) {
            return string::olive_str_getslice(obj, start, stop, step, flags);
        }
        slice_type_error();
    }
    let kind = unsafe { *(obj as *const i64) };
    // Tuples share the list's storage layout, so they slice the same way;
    // sets share its `(kind, ptr, len)` prefix, which is all the slicer
    // reads.
    match kind {
        KIND_LIST | KIND_ANY_LIST | KIND_SET => {
            list::olive_list_getslice(obj, start, stop, step, flags)
        }
        KIND_BYTES => bytes::olive_buf_getslice(obj, start, stop, step, flags),
        KIND_PYOBJECT => python::olive_py_getslice(obj, start, stop, step, flags),
        _ => slice_type_error(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_len_any(obj: i64) -> i64 {
    if obj == 0 {
        return 0;
    }
    if obj & 1 != 0 {
        return string::olive_str_len(obj);
    }
    if !is_active_object(obj) {
        crate::panic::abort("len() argument has no length", None);
    }
    let kind = unsafe { *(obj as *const i64) };
    match kind {
        KIND_LIST | KIND_ANY_LIST => olive_list_len(obj),
        KIND_OBJ => obj::olive_obj_len(obj),
        KIND_SET => unsafe { (*(obj as *const OliveHashSet)).len as i64 },
        KIND_BYTES => bytes::olive_buf_len(obj),
        KIND_PYOBJECT => python::olive_py_len(obj as python::PyObject),
        _ => crate::panic::abort("len() argument has no length", None),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_get_index_any(obj: i64, index: i64, loc: i64) -> i64 {
    if obj == 0 || obj & boxed::TAG_MASK == boxed::TAG_NULL {
        panic::olive_nil_index_fail(loc);
    }
    if matches!(obj & boxed::TAG_MASK, boxed::TAG_INT | boxed::TAG_BOOL) {
        index_type_error(loc);
    }
    if obj & 1 != 0 {
        return string::olive_str_get_checked(obj, index, loc);
    }
    if !is_active_object(obj) {
        index_type_error(loc);
    }
    let kind = unsafe { *(obj as *const i64) };
    match kind {
        KIND_LIST | KIND_ANY_LIST => {
            let len = olive_list_len(obj);
            let effective = if index < 0 { index + len } else { index };
            if effective < 0 || effective >= len {
                panic::olive_bounds_fail(index, len, loc);
            }
            olive_list_get(obj, index)
        }
        KIND_OBJ => olive_obj_get_checked(obj, index, loc),
        KIND_ENUM => olive_enum_get(obj, index),
        KIND_BYTES => {
            let len = bytes::olive_buf_len(obj);
            let effective = if index < 0 { index + len } else { index };
            if effective < 0 || effective >= len {
                panic::olive_bounds_fail(index, len, loc);
            }
            boxed::olive_box_int(bytes::olive_buf_get(obj, effective))
        }
        KIND_PYOBJECT => {
            let key_obj = if index > 0x10000 && index & 1 != 0 {
                python::olive_py_from_str(index)
            } else {
                python::olive_py_from_int(index)
            };
            let py_res = python::olive_py_getitem(obj as *mut std::ffi::c_void, key_obj);
            python::olive_py_decref(key_obj);
            // getitem returns a wrapped arena handle; unwrap before converting (py_to_olive reads ob_type).
            let raw_res = unsafe { python::olive_py_unwrap(py_res) };
            let olive_res = python::olive_py_conv_to_olive(raw_res);
            python::olive_py_decref(py_res);
            olive_res
        }
        _ => index_type_error(loc),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_index_any(obj: i64, index: i64, val: i64, loc: i64) {
    if obj == 0 || obj & boxed::TAG_MASK == boxed::TAG_NULL {
        panic::olive_nil_index_fail(loc);
    }
    if matches!(obj & boxed::TAG_MASK, boxed::TAG_INT | boxed::TAG_BOOL) {
        index_type_error(loc);
    }
    if obj & 1 != 0 {
        index_type_error(loc);
    }
    if !is_active_object(obj) {
        index_type_error(loc);
    }
    let kind = unsafe { *(obj as *const i64) };
    match kind {
        KIND_LIST | KIND_ANY_LIST => {
            let len = olive_list_len(obj);
            let effective = if index < 0 { index + len } else { index };
            if effective < 0 || effective >= len {
                panic::olive_bounds_fail(index, len, loc);
            }
            let old = list::olive_list_get(obj, effective);
            if old != val {
                olive_free_any(old);
            }
            olive_list_set(obj, effective, val)
        }
        KIND_BYTES => {
            let len = bytes::olive_buf_len(obj);
            let effective = if index < 0 { index + len } else { index };
            if effective < 0 || effective >= len {
                panic::olive_bounds_fail(index, len, loc);
            }
            bytes::olive_buf_set(obj, effective, boxed::olive_unbox_int(val))
        }
        KIND_OBJ => {
            let old = obj::olive_obj_get(obj, index);
            if old != val {
                olive_free_any(old);
            }
            olive_obj_set(obj, index, val);
        }
        KIND_PYOBJECT => {
            let key_obj = if index > 0x10000 && index & 1 != 0 {
                python::olive_py_from_str(index)
            } else {
                python::olive_py_from_int(index)
            };
            let py_val = python::olive_py_conv_to_py(val);
            python::olive_py_setitem(obj as *mut std::ffi::c_void, key_obj, py_val);
            python::olive_py_decref(key_obj);
            python::olive_py_decref(py_val);
        }
        _ => index_type_error(loc),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_indexing_uses_boxed_values_and_negative_indices() {
        let bytes = bytes::new_buf(vec![42, 255]);
        assert_eq!(olive_len_any(bytes), 2);
        assert_eq!(olive_get_index_any(bytes, 0, 0), boxed::olive_box_int(42));
        assert_eq!(olive_get_index_any(bytes, -1, 0), boxed::olive_box_int(255));
        olive_set_index_any(bytes, -1, boxed::olive_box_int(7), 0);
        assert_eq!(bytes::olive_buf_get(bytes, 1), 7);
        olive_free_any(bytes);
    }

    #[test]
    fn list_replacement_releases_strings_but_preserves_self_assignment() {
        let old = olive_str_internal("old value");
        let generation = string_slab::olive_str_gen_of(old);
        let list = list::list_from_vec(vec![old]);
        olive_set_index_any(list, -1, old, 0);
        assert_eq!(string_slab::olive_str_gen_stale(old, generation), 0);
        let replacement = olive_str_internal("new value");
        olive_set_index_any(list, -1, replacement, 0);
        assert_eq!(string_slab::olive_str_gen_stale(old, generation), 1);
        assert_eq!(olive_get_index_any(list, -1, 0), replacement);
        olive_free_any(list);
    }

    #[test]
    fn dictionary_replacement_releases_strings() {
        let old = olive_str_internal("old dictionary value");
        let generation = string_slab::olive_str_gen_of(old);
        let dict = olive_obj_new();
        olive_obj_set(dict, 42, old);
        olive_set_index_any(dict, 42, old, 0);
        assert_eq!(string_slab::olive_str_gen_stale(old, generation), 0);
        olive_set_index_any(dict, 42, boxed::olive_box_int(7), 0);
        assert_eq!(string_slab::olive_str_gen_stale(old, generation), 1);
        assert_eq!(olive_get_index_any(dict, 42, 0), boxed::olive_box_int(7));
        olive_free_any(dict);
    }

    #[test]
    fn dynamic_length_supports_sets() {
        let set = set::olive_set_new(0);
        assert_eq!(olive_len_any(set), 0);
        set::olive_set_add(set, boxed::olive_box_int(42));
        assert_eq!(olive_len_any(set), 1);
        olive_free_any(set);
    }
}
