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

/// Validates a statically-`Any` method receiver against the method's kinds
/// before the statically-dispatched implementation runs: first-match
/// dispatch sends every `Any` receiver down one arm, so a list word reaches
/// dict code (hanging in hash probing) and vice versa. Bitmask: 1 = list,
/// 2 = dict, 4 = set, 8 = string. Unknown methods and `PyObject` pass
/// through with legacy behavior; anything else faults naming the method.
#[unsafe(no_mangle)]
pub extern "C" fn olive_any_check_method(obj: i64, mask: i64, method: i64) -> i64 {
    const LIST: i64 = 1;
    const DICT: i64 = 2;
    const SET: i64 = 4;
    const STR: i64 = 8;
    let name = olive_str_from_ptr(method);
    let is_str_word =
        crate::string::is_interned_char(obj) || (obj & 1 == 1 && (obj & !1) > 0x10000);
    let ok = if obj != 0 && is_active_object(obj) {
        match unsafe { *(obj as *const i64) } {
            KIND_LIST | KIND_ANY_LIST => mask & LIST != 0,
            KIND_OBJ => mask & DICT != 0,
            KIND_SET => mask & SET != 0,
            KIND_PYOBJECT => true,
            _ => false,
        }
    } else {
        mask & STR != 0 && is_str_word
    };
    if !ok {
        let kind_name = olive_str_from_ptr(olive_typeof_str(obj));
        panic::abort(&format!("no method `{name}` on `{kind_name}`"), None);
    }
    obj
}

/// `count` on a statically-`Any` receiver: `str` and `list` both define it,
/// so first-match dispatch misroutes one of them (a string word read as a
/// list header aborts). Dispatches on the value's own representation like
/// `olive_getslice_any` does; anything else faults.
#[unsafe(no_mangle)]
pub extern "C" fn olive_any_count(obj: i64, needle: i64, desc: i64) -> i64 {
    if crate::string::is_interned_char(obj) || (obj & 1 == 1 && (obj & !1) > 0x10000) {
        return string::olive_str_count(obj, needle);
    }
    if obj != 0 && is_active_object(obj) {
        match unsafe { *(obj as *const i64) } {
            KIND_LIST | KIND_ANY_LIST => {
                return list::olive_list_count_typed(obj, needle, desc);
            }
            KIND_PYOBJECT => {
                panic::abort("no method `count` on `PyObject`", None);
            }
            _ => {}
        }
    }
    let kind_name = olive_str_from_ptr(olive_typeof_str(obj));
    panic::abort(&format!("no method `count` on `{kind_name}`"), None);
}

/// `clear` on a statically-`Any` receiver: lists, dicts, and sets all
/// define it, so first-match dispatch misroutes two of the three (clearing
/// a list through the dict implementation segfaults). Dispatches on the
/// value's own representation; anything else faults.
#[unsafe(no_mangle)]
pub extern "C" fn olive_any_clear(obj: i64) -> i64 {
    if obj != 0 && is_active_object(obj) {
        match unsafe { *(obj as *const i64) } {
            KIND_LIST | KIND_ANY_LIST => return list::olive_list_clear(obj),
            KIND_OBJ => return obj::olive_obj_clear(obj),
            KIND_SET => return set::olive_set_clear(obj),
            KIND_PYOBJECT => {
                panic::abort("no method `clear` on `PyObject`", None);
            }
            _ => {}
        }
    }
    let kind_name = olive_str_from_ptr(olive_typeof_str(obj));
    panic::abort(&format!("no method `clear` on `{kind_name}`"), None);
}

/// `pop` on a statically-`Any` receiver: lists pop the last element with
/// no arguments, dicts pop a key with one (faulting when absent) or return
/// a default with two. First-match dispatch hangs list words in dict hash
/// probing, so this dispatches on the value's own representation instead.
#[unsafe(no_mangle)]
pub extern "C" fn olive_any_pop(obj: i64, argc: i64, a0: i64, a1: i64, loc: i64) -> i64 {
    if obj != 0 && is_active_object(obj) {
        match unsafe { *(obj as *const i64) } {
            KIND_LIST | KIND_ANY_LIST => {
                if argc != 0 {
                    panic::abort("wrong number of arguments to `pop`", None);
                }
                return list::olive_list_pop(obj);
            }
            KIND_OBJ => {
                if argc == 1 {
                    return obj::olive_obj_pop_checked(obj, a0, loc);
                }
                if argc == 2 {
                    return obj::olive_obj_pop_default(obj, a0, a1);
                }
                panic::abort("wrong number of arguments to `pop`", None);
            }
            _ => {}
        }
    }
    let kind_name = olive_str_from_ptr(olive_typeof_str(obj));
    panic::abort(&format!("no method `pop` on `{kind_name}`"), None);
}

/// `remove` on a statically-`Any` receiver: lists remove by index, dicts
/// by key, sets by member (faulting when absent). First-match dispatch
/// hangs list words in dict hash probing, so this dispatches on the
/// value's own representation instead. Set membership compares through the
/// descriptor (like `count` does): erased members are boxed while the
/// needle arrives raw, so both forms travel (`arg` raw for list indices
/// and dict keys, `arg_boxed` for set members) and each branch takes the
/// one matching its representation. An `Any` dict holds aggregate keys in
/// `Any` form, so a boxed or sequence needle meets the stored words; every
/// other dict key stays on the raw form that matches bare stored keys.
#[unsafe(no_mangle)]
pub extern "C" fn olive_any_remove(obj: i64, arg: i64, arg_boxed: i64, loc: i64, desc: i64) -> i64 {
    if obj != 0 && is_active_object(obj) {
        match unsafe { *(obj as *const i64) } {
            KIND_LIST | KIND_ANY_LIST => return list::olive_list_remove(obj, arg),
            KIND_OBJ => {
                let key = if crate::hash_typed::is_struct_box_key(arg_boxed)
                    || crate::hash_typed::is_seq_key(arg_boxed)
                    || crate::hash_typed::is_set_key(arg_boxed)
                    || crate::hash_typed::is_dict_key(arg_boxed)
                {
                    arg_boxed
                } else {
                    arg
                };
                return obj::olive_obj_remove(obj, key);
            }
            KIND_SET => {
                return crate::hash_typed::olive_set_remove_checked_typed(
                    obj, arg_boxed, loc, desc,
                );
            }
            _ => {}
        }
    }
    let kind_name = olive_str_from_ptr(olive_typeof_str(obj));
    panic::abort(&format!("no method `remove` on `{kind_name}`"), None);
}

/// Member read on a dynamically-typed object: a struct erased into `Any`
/// carries its descriptor in a box, so member access peels the box and
/// reads the field by name; dicts read by key exactly as before. Anything
/// else faults instead of misreading the word as a map (a list word hashed
/// as a dict hangs in probing; an int word faults on a wild key).
#[unsafe(no_mangle)]
pub extern "C" fn olive_any_getattr(obj: i64, attr: i64, loc: i64) -> i64 {
    if obj != 0 && is_active_object(obj) {
        let kind = unsafe { *(obj as *const i64) };
        if kind == crate::struct_box::KIND_STRUCT_BOX {
            return struct_box_member(obj, attr, loc);
        }
        if kind == KIND_OBJ {
            return obj::olive_obj_get_checked(obj, attr, loc);
        }
    }
    let kind_name = olive_str_from_ptr(olive_typeof_str(obj));
    let attr_name = olive_str_from_ptr(attr);
    let location = (loc != 0).then(|| olive_str_from_ptr(loc));
    panic::abort(
        &format!("no field or method `{attr_name}` on `{kind_name}`"),
        location.as_deref(),
    );
}

/// Member write on a dynamically-typed object: a struct erased into `Any`
/// stores through the descriptor walk, releasing the displaced word the
/// same way; dicts store by key exactly as before. Anything else faults
/// instead of writing a map-shaped record into the wrong layout (which
/// segfaulted).
#[unsafe(no_mangle)]
pub extern "C" fn olive_any_setattr(obj: i64, attr: i64, val: i64, loc: i64) -> i64 {
    if obj != 0 && is_active_object(obj) {
        let kind = unsafe { *(obj as *const i64) };
        if kind == crate::struct_box::KIND_STRUCT_BOX {
            struct_box_store(obj, attr, val, loc);
            return 0;
        }
        if kind == KIND_OBJ {
            return obj::olive_obj_set(obj, attr, val);
        }
    }
    let kind_name = olive_str_from_ptr(olive_typeof_str(obj));
    let attr_name = olive_str_from_ptr(attr);
    let location = (loc != 0).then(|| olive_str_from_ptr(loc));
    panic::abort(
        &format!("no field or method `{attr_name}` on `{kind_name}`"),
        location.as_deref(),
    );
}

/// Stores `val` into a struct-box field by name, releasing the displaced
/// word through the field's own descriptor first (like the concrete path
/// releases through the static field type). The new word stores as-is:
/// the caller coerces it to the field's representation, mirroring the
/// concrete lowering.
fn struct_box_store(obj: i64, attr: i64, val: i64, loc: i64) {
    let want = olive_str_to_bytes(attr);
    let b = unsafe { &*(obj as *const crate::struct_box::OliveStructBox) };
    let desc = b.desc as *const u8;
    if unsafe { *desc } != crate::format::D_STRUCT
        && unsafe { *desc } != crate::format::D_STRUCT_SHARED
    {
        struct_member_miss(attr, obj, loc);
    }
    let mut pos = 1usize;
    let name_len = unsafe { *desc.add(pos) } as usize - 13;
    pos += 1 + name_len;
    let n = unsafe { *desc.add(pos) } as usize - 13;
    pos += 1;
    for i in 0..n {
        let field_len = unsafe { *desc.add(pos) } as usize - 13;
        let field_name = unsafe { std::slice::from_raw_parts(desc.add(pos + 1), field_len) };
        pos += 1 + field_len;
        let field_type_pos = pos;
        if field_name == want {
            let inner = b.ptr;
            if inner == 0 {
                struct_member_miss(attr, obj, loc);
            }
            let n_fields = unsafe { *(inner as *const i64) };
            if (i as i64) >= n_fields {
                struct_member_miss(attr, obj, loc);
            }
            let slot = (inner + 8 + 8 * i as i64) as *mut i64;
            let old = unsafe { *slot };
            if old != 0 {
                let mut free_pos = field_type_pos;
                crate::free_typed::free_val(old, desc, &mut free_pos);
            }
            unsafe { *slot = val };
            return;
        }
        crate::format::skip(desc, &mut pos);
    }
    struct_member_miss(attr, obj, loc);
}

/// Reads a struct-box member by name, walking the box's descriptor for the
/// field index (same layout `copy_struct` mirrors: name, field count biased
/// by 13, then length-prefixed names with types). Returns the raw word, a
/// borrow the box keeps owning, exactly like a concrete member read.
fn struct_box_member(obj: i64, attr: i64, loc: i64) -> i64 {
    let want = olive_str_to_bytes(attr);
    let b = unsafe { &*(obj as *const crate::struct_box::OliveStructBox) };
    let desc = b.desc as *const u8;
    if unsafe { *desc } != crate::format::D_STRUCT
        && unsafe { *desc } != crate::format::D_STRUCT_SHARED
    {
        struct_member_miss(attr, obj, loc);
    }
    let mut pos = 1usize;
    let name_len = unsafe { *desc.add(pos) } as usize - 13;
    pos += 1 + name_len;
    let n = unsafe { *desc.add(pos) } as usize - 13;
    pos += 1;
    for i in 0..n {
        let field_len = unsafe { *desc.add(pos) } as usize - 13;
        let field_name = unsafe { std::slice::from_raw_parts(desc.add(pos + 1), field_len) };
        pos += 1 + field_len;
        crate::format::skip(desc, &mut pos);
        if field_name == want {
            let inner = b.ptr;
            if inner == 0 {
                struct_member_miss(attr, obj, loc);
            }
            let n_fields = unsafe { *(inner as *const i64) };
            if (i as i64) < n_fields {
                return unsafe { *((inner + 8 + 8 * i as i64) as *const i64) };
            }
            struct_member_miss(attr, obj, loc);
        }
    }
    struct_member_miss(attr, obj, loc);
}

fn struct_member_miss(attr: i64, obj: i64, loc: i64) -> ! {
    let kind_name = olive_str_from_ptr(olive_typeof_str(obj));
    let attr_name = olive_str_from_ptr(attr);
    let location = (loc != 0).then(|| olive_str_from_ptr(loc));
    panic::abort(
        &format!("no field or method `{attr_name}` on `{kind_name}`"),
        location.as_deref(),
    );
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
