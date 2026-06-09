//! Copy-out for a collection argument passed into a Python call: the Python
//! callee's mutation (`xs.sort()`, `d.update(...)`, `random.shuffle(xs)`) is
//! synced back into the same Olive allocation after the call returns, on both
//! the success and the exception path, so a Python-mutating call behaves like
//! the equivalent Python code with zero extra syntax on the Olive side.

use crate::python::python_coerce::raw_ob_type;
use crate::python::*;
use std::ffi::CStr;
use std::os::raw::c_char;

/// Not a collection: no copy-out for this argument.
pub(crate) const TAG_NONE: i64 = 0;
pub(crate) const TAG_ANY_LIST: i64 = 1;
pub(crate) const TAG_INT_LIST: i64 = 2;
pub(crate) const TAG_FLOAT_LIST: i64 = 3;
pub(crate) const TAG_BOOL_LIST: i64 = 4;
pub(crate) const TAG_STR_LIST: i64 = 5;
/// `{str: Any}`: values are already boxed the way `py_to_any_internal` boxes
/// them, so no separate scalar type ever needs distinguishing.
pub(crate) const TAG_ANY_DICT: i64 = 6;
pub(crate) const TAG_ANY_SET: i64 = 7;
/// A concretely-typed dict/set (`{str: int}`, `set[int]`, ...) stores its
/// values raw, the same convention a typed list uses -- not Any-boxed. Any
/// container this deep-realizes or syncs back needs to know which, or a raw
/// scalar whose low bits collide with the inline-Any-tag pattern (`TAG_INT`
/// etc. in `boxed.rs`) gets silently misread as a boxed value instead of a
/// plain one. Lists don't need separate tags for this because their own
/// `KIND_LIST`/`KIND_ANY_LIST` header word already says which; a dict/set has
/// one shared kind for both shapes, so the compiler's static tag is the only
/// signal available.
pub(crate) const TAG_INT_DICT: i64 = 8;
pub(crate) const TAG_FLOAT_DICT: i64 = 9;
pub(crate) const TAG_BOOL_DICT: i64 = 10;
pub(crate) const TAG_STR_DICT: i64 = 11;
pub(crate) const TAG_INT_SET: i64 = 12;
pub(crate) const TAG_FLOAT_SET: i64 = 13;
pub(crate) const TAG_BOOL_SET: i64 = 14;
pub(crate) const TAG_STR_SET: i64 = 15;

/// One collection argument realized for a Python call: the Olive allocation
/// it came from, the genuine Python object built for it, and which sync
/// routine applies. `py_obj` carries a reference distinct from any tuple
/// slot's -- the tuple's own reference is released with the tuple itself.
pub(crate) struct WritebackPair {
    olive_ptr: i64,
    py_obj: PyObject,
    tag: i64,
}

/// Reads arg `i`'s 4-bit collection tag out of a packed tag word. Calls with
/// more than 16 args pack `0` for every slot (the compiler's fallback), so an
/// out-of-range index reads as "not a collection" too.
pub(crate) fn tag_at(tags: i64, i: usize) -> i64 {
    if i >= 16 {
        return TAG_NONE;
    }
    (tags >> (i * 4)) & 0xF
}

/// The scalar-decode shape a tag implies, normalized to the list tag
/// constants: a typed dict/set's value/element decodes exactly like a typed
/// list's element of the same scalar kind.
fn scalar_kind(tag: i64) -> i64 {
    match tag {
        TAG_INT_LIST | TAG_INT_DICT | TAG_INT_SET => TAG_INT_LIST,
        TAG_FLOAT_LIST | TAG_FLOAT_DICT | TAG_FLOAT_SET => TAG_FLOAT_LIST,
        TAG_BOOL_LIST | TAG_BOOL_DICT | TAG_BOOL_SET => TAG_BOOL_LIST,
        TAG_STR_LIST | TAG_STR_DICT | TAG_STR_SET => TAG_STR_LIST,
        other => other,
    }
}

/// Converts one call argument, tracking it for copy-out when `tag` marks it
/// as a collection. Dedupes by `olive_ptr`: the same Olive list/dict passed
/// twice in one call becomes one Python object referenced twice, matching
/// what passing the same object twice in Python itself would do.
pub(crate) unsafe fn convert_arg(val: i64, tag: i64, pairs: &mut Vec<WritebackPair>) -> PyObject {
    unsafe {
        if tag == TAG_NONE || !crate::is_active_object(val) {
            return olive_to_py(val);
        }
        convert_collection_arg(val, tag, pairs)
    }
}

/// The collection-realize half of `convert_arg`, factored out so the
/// tagged fast path (`convert_arg_tagged`) can reuse it without duplicating
/// the dedupe-and-track logic.
unsafe fn convert_collection_arg(val: i64, tag: i64, pairs: &mut Vec<WritebackPair>) -> PyObject {
    unsafe {
        if let Some(existing) = pairs.iter().find(|p| p.olive_ptr == val) {
            PY_INC_REF(existing.py_obj);
            return existing.py_obj;
        }
        let py_obj = match tag {
            TAG_INT_LIST | TAG_FLOAT_LIST | TAG_BOOL_LIST | TAG_STR_LIST => {
                to_py_typed_list(val, scalar_kind(tag))
            }
            TAG_INT_DICT | TAG_FLOAT_DICT | TAG_BOOL_DICT | TAG_STR_DICT => {
                to_py_typed_dict(val, scalar_kind(tag))
            }
            TAG_INT_SET | TAG_FLOAT_SET | TAG_BOOL_SET | TAG_STR_SET => {
                to_py_typed_set(val, scalar_kind(tag))
            }
            _ => to_py_deep(val),
        };
        if py_obj.is_null() {
            return py_obj;
        }
        // One reference for the tuple/dict slot this call is building
        // (stolen by `PyTuple_SetItem`/consumed by the kwargs dict), one
        // retained here for the sync pass after the call returns.
        PY_INC_REF(py_obj);
        pairs.push(WritebackPair {
            olive_ptr: val,
            py_obj,
            tag,
        });
        py_obj
    }
}

/// Static-type encoding for a py-call argument's raw word, orthogonal to the
/// `TAG_*` collection vocabulary above (which says whether/how an arg copies
/// out, not how its bits decode). Chosen by the compiler from the argument's
/// *declared* type so a raw word that would otherwise collide -- `0` as
/// `int` vs `None`, a bit pattern as `int` vs `float`, `0`/`1` as `int` vs
/// `bool` -- decodes exactly, with no runtime guessing (the old fallback,
/// `olive_to_py`'s `looks_like_float` heuristic, is unsound on adversarial
/// bit patterns and always wrong for `bool`/`None`). A collection-tagged slot
/// ignores this word entirely; see `convert_arg_tagged`.
pub(crate) const ARG_PYOBJECT: i64 = 0;
pub(crate) const ARG_INT: i64 = 1;
pub(crate) const ARG_FLOAT: i64 = 2;
pub(crate) const ARG_STR: i64 = 3;
pub(crate) const ARG_BOOL: i64 = 4;
/// A genuinely dynamic value (`Any`, or any type this scheme doesn't name
/// individually): decode via the same inline-tag-aware path a boxed `Any`
/// slot always used, `olive_any_to_py`.
pub(crate) const ARG_ANY: i64 = 5;
pub(crate) const ARG_NONE: i64 = 6;
pub(crate) const ARG_BYTES: i64 = 7;

/// Reads arg `i`'s 4-bit encode tag out of a packed word. Mirrors `tag_at`;
/// a call with more than 16 args never reaches the tagged fast path at all
/// (the compiler keeps it on the legacy, pre-converted entry points), so an
/// out-of-range index here is unreachable in practice -- `ARG_ANY` is the
/// safe default if it's ever hit anyway, since `olive_any_to_py` is a
/// correct (if slower) decode for every representable value.
pub(crate) fn arg_tag_at(tags: i64, i: usize) -> i64 {
    if i >= 16 {
        return ARG_ANY;
    }
    (tags >> (i * 4)) & 0xF
}

/// Decodes one raw, unconverted call argument by its compiler-supplied
/// static tag. The tagged fast path: no pre-conversion, no handle
/// allocation, one C-API call per scalar, all under the call's single GIL
/// region.
unsafe fn decode_scalar_arg(val: i64, tag: i64) -> PyObject {
    unsafe {
        match tag {
            ARG_PYOBJECT => {
                let p = olive_py_unwrap(val as PyObject);
                if p.is_null() {
                    return p;
                }
                PY_INC_REF(p);
                p
            }
            ARG_INT => raw_scalar_to_py(val, TAG_INT_LIST),
            ARG_FLOAT => raw_scalar_to_py(val, TAG_FLOAT_LIST),
            ARG_STR => raw_scalar_to_py(val, TAG_STR_LIST),
            ARG_BOOL => raw_scalar_to_py(val, TAG_BOOL_LIST),
            ARG_NONE => {
                let none = _PY_NONE_STRUCT as PyObject;
                PY_INC_REF(none);
                none
            }
            ARG_BYTES => olive_to_py(val),
            _ => olive_any_to_py_checked(val),
        }
    }
}

/// The tagged-argument counterpart to `convert_arg`: `coll_tag` still
/// selects copy-out exactly as before, but a non-collection slot decodes by
/// `arg_tag` instead of falling through to `olive_to_py`'s raw-word
/// heuristic. Used only by the `_t` call entry points; the legacy entry
/// points keep calling `convert_arg` unchanged.
pub(crate) unsafe fn convert_arg_tagged(
    val: i64,
    coll_tag: i64,
    arg_tag: i64,
    pairs: &mut Vec<WritebackPair>,
) -> PyObject {
    unsafe {
        if coll_tag != TAG_NONE && crate::is_active_object(val) {
            return convert_collection_arg(val, coll_tag, pairs);
        }
        decode_scalar_arg(val, arg_tag)
    }
}

/// Releases every tracked pair's retained reference without syncing. Used
/// when argument conversion fails before the Python call itself ever runs.
pub(crate) unsafe fn abandon_pairs(pairs: &[WritebackPair]) {
    unsafe {
        for p in pairs {
            PY_DEC_REF(p.py_obj);
        }
    }
}

/// Converts one raw (unboxed) Olive scalar to a genuine Python value by its
/// static kind. Mirrors `olive_to_py`'s scalar arms but never consults the
/// inline-Any-tag bits -- the value is known concrete, not a boxed `Any`.
unsafe fn raw_scalar_to_py(val: i64, kind: i64) -> PyObject {
    unsafe {
        match kind {
            TAG_INT_LIST => PY_LONG_FROM_LONG(val as std::os::raw::c_long),
            TAG_FLOAT_LIST => PY_FLOAT_FROM_DOUBLE(f64::from_bits(val as u64)),
            TAG_BOOL_LIST => PY_BOOL_FROM_LONG(val as std::os::raw::c_long),
            TAG_STR_LIST => olive_str_to_py(val),
            _ => unreachable!("raw_scalar_to_py: {kind} is not a scalar kind"),
        }
    }
}

/// Deep-realizes a concretely-typed list (`[T]`, `T` a scalar) into a real
/// Python `list`, reading each element raw by `kind` instead of through
/// `to_py_deep`'s per-element runtime-guessed dispatch (`olive_to_py`'s
/// `is_active_object` scan plus the `looks_like_float` heuristic) -- the
/// compiler's own static element type already says which scalar every
/// element is, so there is nothing left to guess.
unsafe fn to_py_typed_list(val: i64, kind: i64) -> PyObject {
    unsafe {
        let n = crate::olive_list_len(val);
        let py_list = PY_LIST_NEW(n as isize);
        for i in 0..n {
            let elem = crate::olive_list_get(val, i);
            let py_v = raw_scalar_to_py(elem, kind);
            PY_LIST_SET_ITEM(py_list, i as isize, py_v);
        }
        py_list
    }
}

/// Deep-realizes a concretely-typed dict (`{str: T}`, `T` a scalar) into a
/// real Python `dict`, reading each value raw by `kind` instead of through
/// `to_py_deep`'s Any-boxed assumption.
unsafe fn to_py_typed_dict(val: i64, kind: i64) -> PyObject {
    unsafe {
        let py_dict = PY_DICT_NEW();
        let keys = crate::olive_obj_keys(val);
        let n = crate::olive_list_len(keys);
        for i in 0..n {
            let key = crate::olive_list_get(keys, i);
            let value = crate::olive_obj_get(val, key);
            let py_value = raw_scalar_to_py(value, kind);
            PY_DICT_SET_ITEM_STRING(py_dict, (key & !1) as *const c_char, py_value);
            PY_DEC_REF(py_value);
        }
        py_dict
    }
}

/// Deep-realizes a concretely-typed set (`set[T]`, `T` a scalar) into a real
/// Python `set`, the same raw-by-`kind` reasoning as `to_py_typed_dict`.
unsafe fn to_py_typed_set(val: i64, kind: i64) -> PyObject {
    unsafe {
        let hs = &*(val as *const crate::OliveHashSet);
        let pys = PY_SET_NEW(std::ptr::null_mut());
        for i in 0..hs.len {
            let v = *hs.ptr.add(i);
            let py_v = raw_scalar_to_py(v, kind);
            PY_SET_ADD(pys, py_v);
            PY_DEC_REF(py_v);
        }
        pys
    }
}

fn py_type_name(ty: PyObject) -> String {
    unsafe {
        if ty.is_null() {
            return "object".to_string();
        }
        let name_obj = PY_OBJECT_GET_ATTR_STRING(ty, b"__name__\0".as_ptr() as *const c_char);
        if name_obj.is_null() {
            if !PY_ERR_OCCURRED().is_null() {
                PY_ERR_CLEAR();
            }
            return "object".to_string();
        }
        let s = PY_UNICODE_AS_UTF8(name_obj);
        let name = if s.is_null() {
            "object".to_string()
        } else {
            CStr::from_ptr(s).to_string_lossy().into_owned()
        };
        PY_DEC_REF(name_obj);
        name
    }
}

fn expected_name_for_kind(kind: i64) -> &'static str {
    match kind {
        TAG_INT_LIST => "int",
        TAG_FLOAT_LIST => "float",
        TAG_BOOL_LIST => "bool",
        TAG_STR_LIST => "str",
        _ => "value",
    }
}

/// Decodes one Python scalar back into an Olive typed-container element by
/// its static kind. Bool is checked before int (bool subtypes int in
/// CPython): an int slot rejects an out-of-band `True`/`False` just like a
/// bool slot rejects a plain `5`, each kind has exactly one accepted Python
/// type.
unsafe fn decode_scalar(item: PyObject, kind: i64) -> Result<i64, String> {
    unsafe {
        let ty = raw_ob_type(item);
        let is_sub = |expected: PyObject| {
            !expected.is_null() && (ty == expected || PY_TYPE_IS_SUBTYPE(ty, expected) != 0)
        };
        match kind {
            TAG_INT_LIST => {
                if is_sub(PY_BOOL_TYPE) || is_sub(PY_LONG_TYPE) {
                    let v = PY_LONG_AS_LONG(item);
                    #[cfg(windows)]
                    let v = v as i64;
                    return Ok(v);
                }
                Err(py_type_name(ty))
            }
            TAG_FLOAT_LIST => {
                if is_sub(PY_FLOAT_TYPE) {
                    return Ok(PY_FLOAT_AS_DOUBLE(item).to_bits() as i64);
                }
                Err(py_type_name(ty))
            }
            TAG_BOOL_LIST => {
                if is_sub(PY_BOOL_TYPE) {
                    return Ok(if PY_LONG_AS_LONG(item) != 0 { 1 } else { 0 });
                }
                Err(py_type_name(ty))
            }
            TAG_STR_LIST => {
                if is_sub(PY_UNICODE_TYPE) {
                    let r = py_str_to_olive(item);
                    if r == 0 {
                        return Err("str (invalid utf-8)".to_string());
                    }
                    return Ok(r);
                }
                Err(py_type_name(ty))
            }
            _ => unreachable!("decode_scalar: {kind} is not a scalar kind"),
        }
    }
}

unsafe fn writeback_type_fail(loc_desc: &str, tag: i64, actual: &str) -> ! {
    let msg = format!(
        "writeback type mismatch: {loc_desc} synced back from Python is `{actual}`, expected `{}`",
        expected_name_for_kind(scalar_kind(tag))
    );
    let loc = py_call_loc();
    let loc = (!loc.is_empty()).then_some(loc);
    crate::panic::abort_py_writeback_type(&msg, loc.as_deref())
}

unsafe fn sync_list(pair: &WritebackPair) {
    unsafe {
        let new_len = PY_OBJECT_LENGTH(pair.py_obj).max(0) as usize;
        let old_len = crate::olive_list_len(pair.olive_ptr) as usize;
        let overlap = new_len.min(old_len);
        let kind = scalar_kind(pair.tag);

        let decode = |i: usize| -> i64 {
            let item = PY_LIST_GET_ITEM(pair.py_obj, i as isize);
            if pair.tag == TAG_ANY_LIST {
                return py_to_any_internal(item);
            }
            match decode_scalar(item, kind) {
                Ok(v) => v,
                Err(actual) => writeback_type_fail(&format!("element {i}"), pair.tag, &actual),
            }
        };

        for i in 0..overlap {
            let val = decode(i);
            crate::olive_list_set(pair.olive_ptr, i as i64, val);
        }
        if new_len > old_len {
            for i in old_len..new_len {
                let val = decode(i);
                crate::olive_list_insert(pair.olive_ptr, i as i64, val);
            }
        } else if new_len < old_len {
            for _ in new_len..old_len {
                crate::olive_list_pop(pair.olive_ptr);
            }
        }
    }
}

/// Reads a Python dict key as an Olive string, stringifying non-`str` keys
/// the same way `olive_py_to_dict_internal` does. `0` on a decode failure (a
/// non-UTF-8 key, or `str()` itself raising), which the caller skips.
unsafe fn dict_key_olive(key_obj: PyObject) -> i64 {
    unsafe {
        let key_ty = raw_ob_type(key_obj);
        let is_unicode = !key_ty.is_null()
            && !PY_UNICODE_TYPE.is_null()
            && (key_ty == PY_UNICODE_TYPE || PY_TYPE_IS_SUBTYPE(key_ty, PY_UNICODE_TYPE) != 0);
        if is_unicode {
            return py_str_to_olive(key_obj);
        }
        let str_obj = PY_OBJECT_STR(key_obj);
        if str_obj.is_null() {
            return 0;
        }
        let r = py_str_to_olive(str_obj);
        PY_DEC_REF(str_obj);
        r
    }
}

unsafe fn sync_dict(pair: &WritebackPair) {
    unsafe {
        crate::olive_obj_clear(pair.olive_ptr);
        let mut pos: isize = 0;
        let mut key_obj: PyObject = std::ptr::null_mut();
        let mut val_obj: PyObject = std::ptr::null_mut();
        while PY_DICT_NEXT(pair.py_obj, &mut pos, &mut key_obj, &mut val_obj) != 0 {
            if key_obj.is_null() {
                continue;
            }
            let key_ptr = dict_key_olive(key_obj);
            if key_ptr == 0 {
                continue;
            }
            let olive_val = py_to_any_internal(val_obj);
            crate::olive_obj_set(pair.olive_ptr, key_ptr, olive_val);
        }
    }
}

/// `sync_dict`'s counterpart for a concretely-typed dict: values decode raw
/// by `kind` (an exact-type check, faulting on mismatch) instead of boxing
/// through `py_to_any_internal`, matching the raw storage a `{str: T}` dict
/// actually uses.
unsafe fn sync_dict_typed(pair: &WritebackPair) {
    unsafe {
        crate::olive_obj_clear(pair.olive_ptr);
        let kind = scalar_kind(pair.tag);
        let mut pos: isize = 0;
        let mut key_obj: PyObject = std::ptr::null_mut();
        let mut val_obj: PyObject = std::ptr::null_mut();
        while PY_DICT_NEXT(pair.py_obj, &mut pos, &mut key_obj, &mut val_obj) != 0 {
            if key_obj.is_null() {
                continue;
            }
            let key_ptr = dict_key_olive(key_obj);
            if key_ptr == 0 {
                continue;
            }
            let olive_val = match decode_scalar(val_obj, kind) {
                Ok(v) => v,
                Err(actual) => {
                    let key_str = crate::olive_str_from_ptr(key_ptr);
                    writeback_type_fail(&format!("value at key \"{key_str}\""), pair.tag, &actual)
                }
            };
            crate::olive_obj_set(pair.olive_ptr, key_ptr, olive_val);
        }
    }
}

unsafe fn sync_set(pair: &WritebackPair) {
    unsafe {
        crate::olive_set_clear(pair.olive_ptr);
        let iter = PY_OBJECT_GET_ITER(pair.py_obj);
        if iter.is_null() {
            PY_ERR_CLEAR();
            return;
        }
        loop {
            let item = PY_ITER_NEXT(iter);
            if item.is_null() {
                PY_ERR_CLEAR();
                break;
            }
            let olive_val = py_to_any_internal(item);
            crate::olive_set_add(pair.olive_ptr, olive_val);
            PY_DEC_REF(item);
        }
        PY_DEC_REF(iter);
    }
}

/// `sync_set`'s counterpart for a concretely-typed set: elements decode raw
/// by `kind` instead of boxing through `py_to_any_internal`.
unsafe fn sync_set_typed(pair: &WritebackPair) {
    unsafe {
        crate::olive_set_clear(pair.olive_ptr);
        let kind = scalar_kind(pair.tag);
        let iter = PY_OBJECT_GET_ITER(pair.py_obj);
        if iter.is_null() {
            PY_ERR_CLEAR();
            return;
        }
        let mut i = 0usize;
        loop {
            let item = PY_ITER_NEXT(iter);
            if item.is_null() {
                PY_ERR_CLEAR();
                break;
            }
            let olive_val = match decode_scalar(item, kind) {
                Ok(v) => v,
                Err(actual) => writeback_type_fail(&format!("element {i}"), pair.tag, &actual),
            };
            crate::olive_set_add(pair.olive_ptr, olive_val);
            PY_DEC_REF(item);
            i += 1;
        }
        PY_DEC_REF(iter);
    }
}

/// Syncs every tracked collection argument back into its Olive allocation and
/// releases the pair's retained reference. Runs after the underlying
/// `PyObject_Call`/`PyObject_CallObject`, on both the success and the
/// Python-exception path, before either is handled -- Python keeps whatever
/// partial mutation happened before a raise, and Olive must show the same
/// state.
pub(crate) unsafe fn sync_back(pairs: &[WritebackPair]) {
    unsafe {
        for pair in pairs {
            match pair.tag {
                TAG_ANY_LIST | TAG_INT_LIST | TAG_FLOAT_LIST | TAG_BOOL_LIST | TAG_STR_LIST => {
                    sync_list(pair)
                }
                TAG_ANY_DICT => sync_dict(pair),
                TAG_ANY_SET => sync_set(pair),
                TAG_INT_DICT | TAG_FLOAT_DICT | TAG_BOOL_DICT | TAG_STR_DICT => {
                    sync_dict_typed(pair)
                }
                TAG_INT_SET | TAG_FLOAT_SET | TAG_BOOL_SET | TAG_STR_SET => sync_set_typed(pair),
                _ => {}
            }
            PY_DEC_REF(pair.py_obj);
        }
    }
}
