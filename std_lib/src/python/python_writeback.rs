//! Copy-out for a collection argument passed into a Python call: the Python
//! callee's mutation (`xs.sort()`, `d.update(...)`, `random.shuffle(xs)`) is
//! synced back into the same Olive allocation after the call returns, on both
//! the success and the exception path, so a Python-mutating call behaves like
//! the equivalent Python code with zero extra syntax on the Olive side.

use crate::python::python_coerce::{raw_ob_type, to_py_deep};
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
const TAG_NONE_LIST: i64 = 16;
const TAG_NONE_DICT: i64 = 17;
const TAG_NONE_SET: i64 = 18;

#[repr(align(8))]
struct AlignedScalarDescriptor([u8; 1]);

#[repr(align(8))]
struct AlignedSetDescriptor([u8; 2]);

#[repr(align(8))]
struct AlignedDictDescriptor([u8; 3]);

/// One collection argument realized for a Python call: the Olive allocation
/// it came from, the genuine Python object built for it, and which sync
/// routine applies. `py_obj` carries a reference distinct from any tuple
/// slot's -- the tuple's own reference is released with the tuple itself.
pub(crate) struct WritebackPair {
    olive_ptr: i64,
    py_obj: PyObject,
    tag: i64,
    key_tag: i64,
    value_f32: bool,
    value_u64: bool,
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
        TAG_NONE_LIST | TAG_NONE_DICT | TAG_NONE_SET => TAG_NONE_LIST,
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
        convert_collection_arg(val, tag, DICT_KEY_ANY, pairs)
    }
}

/// The collection-realize half of `convert_arg`, factored out so the
/// tagged fast path (`convert_arg_tagged`) can reuse it without duplicating
/// the dedupe-and-track logic.
unsafe fn convert_collection_arg(
    val: i64,
    tag: i64,
    key_tag: i64,
    pairs: &mut Vec<WritebackPair>,
) -> PyObject {
    unsafe {
        if let Some(existing) = pairs.iter().find(|p| p.olive_ptr == val) {
            PY_INC_REF(existing.py_obj);
            return existing.py_obj;
        }
        let width_flag = key_tag & COLLECTION_WIDTH_FLAG != 0;
        let key_tag = key_tag & 0x7;
        let value_kind = scalar_kind(tag);
        let value_f32 = width_flag && value_kind == TAG_FLOAT_LIST;
        let value_u64 = width_flag && value_kind == TAG_INT_LIST;
        let py_obj = match tag {
            TAG_INT_LIST | TAG_FLOAT_LIST | TAG_BOOL_LIST | TAG_STR_LIST => {
                to_py_typed_list(val, value_kind, value_f32, value_u64)
            }
            TAG_ANY_DICT | TAG_INT_DICT | TAG_FLOAT_DICT | TAG_BOOL_DICT | TAG_STR_DICT => {
                to_py_typed_dict(val, value_kind, key_tag, value_f32, value_u64)
            }
            TAG_INT_SET | TAG_FLOAT_SET | TAG_BOOL_SET | TAG_STR_SET => {
                to_py_typed_set(val, value_kind, value_f32, value_u64)
            }
            TAG_NONE_LIST => to_py_typed_list(val, TAG_NONE_LIST, false, false),
            TAG_NONE_DICT => to_py_typed_dict(val, TAG_NONE_LIST, key_tag, false, false),
            TAG_NONE_SET => to_py_typed_set(val, TAG_NONE_LIST, false, false),
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
            key_tag,
            value_f32,
            value_u64,
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
pub(crate) const ARG_SCALAR_F32: i64 = 14;
pub(crate) const ARG_SCALAR_U64: i64 = 15;
pub(crate) const DICT_KEY_INT: i64 = 0;
pub(crate) const DICT_KEY_U64: i64 = 1;
pub(crate) const DICT_KEY_FLOAT: i64 = 2;
pub(crate) const DICT_KEY_F32: i64 = 3;
pub(crate) const DICT_KEY_BOOL: i64 = 4;
pub(crate) const DICT_KEY_STR: i64 = 5;
pub(crate) const DICT_KEY_ANY: i64 = 6;
pub(crate) const DICT_KEY_NONE: i64 = 7;
/// In a collection argument tag, bit 3 carries f32 or u64 width. It shares
/// the numeric value of `ARG_FLOAT_LIST` because the collection tag context
/// selects the interpretation.
pub(crate) const COLLECTION_WIDTH_FLAG: i64 = 8;

#[inline]
pub(crate) fn arg_is_collection(coll_tag: i64, arg_tag: i64) -> bool {
    coll_tag != TAG_NONE || arg_tag & COLLECTION_WIDTH_FLAG != 0
}
/// A typed list crossing (`[float]`, `[int]`, etc.) in the export
/// direction: the runtime converts the entire collection as one unit
/// instead of treating the handle as an opaque scalar. Tags 8–12 extend
/// the ARG_* vocabulary from scalar-only (0-7) to include the common
/// homogenous collection shapes the compile-time checker knows.
pub(crate) const ARG_FLOAT_LIST: i64 = 8;
pub(crate) const ARG_INT_LIST: i64 = 9;
pub(crate) const ARG_STR_LIST: i64 = 10;
pub(crate) const ARG_BOOL_LIST: i64 = 11;
pub(crate) const ARG_ANY_LIST: i64 = 12;

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
pub(crate) unsafe fn decode_scalar_arg(val: i64, tag: i64) -> PyObject {
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
            ARG_SCALAR_F32 => raw_collection_scalar_to_py(val, TAG_FLOAT_LIST, true, false),
            ARG_SCALAR_U64 => py_long_from_u64(val as u64),
            ARG_STR => raw_scalar_to_py(val, TAG_STR_LIST),
            ARG_BOOL => raw_scalar_to_py(val, TAG_BOOL_LIST),
            ARG_NONE => {
                let none = _PY_NONE_STRUCT as PyObject;
                PY_INC_REF(none);
                none
            }
            ARG_BYTES => olive_to_py(val),
            ARG_FLOAT_LIST => to_py_typed_list(val, TAG_FLOAT_LIST, false, false),
            ARG_INT_LIST => to_py_typed_list(val, TAG_INT_LIST, false, false),
            ARG_STR_LIST => to_py_typed_list(val, TAG_STR_LIST, false, false),
            ARG_BOOL_LIST => to_py_typed_list(val, TAG_BOOL_LIST, false, false),
            ARG_ANY_LIST => to_py_deep(val),
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
            return convert_collection_arg(val, coll_tag, arg_tag, pairs);
        }
        if coll_tag == TAG_NONE && arg_tag == ARG_PYOBJECT {
            if val == 0 {
                return std::ptr::null_mut();
            }
            return olive_to_py(val);
        }
        if coll_tag == TAG_NONE
            && arg_tag & COLLECTION_WIDTH_FLAG != 0
            && crate::is_active_object(val)
        {
            let kind = *(val as *const i64);
            let internal_tag = match kind {
                crate::KIND_LIST | crate::KIND_ANY_LIST => TAG_NONE_LIST,
                crate::KIND_OBJ => TAG_NONE_DICT,
                crate::KIND_SET => TAG_NONE_SET,
                _ => return decode_scalar_arg(val, arg_tag),
            };
            return convert_collection_arg(val, internal_tag, arg_tag & 0x7, pairs);
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
            TAG_INT_LIST => py_long_from_i64(val),
            TAG_FLOAT_LIST => PY_FLOAT_FROM_DOUBLE(f64::from_bits(val as u64)),
            TAG_BOOL_LIST => PY_BOOL_FROM_LONG(val as std::os::raw::c_long),
            TAG_STR_LIST => olive_str_to_py(val),
            TAG_NONE_LIST => {
                let none = _PY_NONE_STRUCT as PyObject;
                PY_INC_REF(none);
                none
            }
            _ => unreachable!("raw_scalar_to_py: {kind} is not a scalar kind"),
        }
    }
}

unsafe fn raw_collection_scalar_to_py(
    val: i64,
    kind: i64,
    value_f32: bool,
    value_u64: bool,
) -> PyObject {
    if value_f32 && kind == TAG_FLOAT_LIST {
        unsafe { PY_FLOAT_FROM_DOUBLE(f32::from_bits(val as u32) as f64) }
    } else if value_u64 && kind == TAG_INT_LIST {
        unsafe { py_long_from_u64(val as u64) }
    } else {
        unsafe { raw_scalar_to_py(val, kind) }
    }
}

/// Deep-realizes a concretely-typed list (`[T]`, `T` a scalar) into a real
/// Python `list`, reading each element raw by `kind` instead of through
/// `to_py_deep`'s per-element runtime-guessed dispatch (`olive_to_py`'s
/// `is_active_object` scan plus the `looks_like_float` heuristic) -- the
/// compiler's own static element type already says which scalar every
/// element is, so there is nothing left to guess.
unsafe fn to_py_typed_list(val: i64, kind: i64, value_f32: bool, value_u64: bool) -> PyObject {
    unsafe {
        let n = crate::olive_list_len(val);
        let py_list = PY_LIST_NEW(n as isize);
        for i in 0..n {
            let elem = crate::olive_list_get(val, i);
            let py_v = raw_collection_scalar_to_py(elem, kind, value_f32, value_u64);
            PY_LIST_SET_ITEM(py_list, i as isize, py_v);
        }
        py_list
    }
}

unsafe fn dict_key_to_py(key: i64, key_tag: i64) -> PyObject {
    unsafe {
        match key_tag {
            DICT_KEY_INT => py_long_from_i64(key),
            DICT_KEY_U64 => py_long_from_u64(key as u64),
            DICT_KEY_FLOAT => PY_FLOAT_FROM_DOUBLE(f64::from_bits(key as u64)),
            DICT_KEY_F32 => PY_FLOAT_FROM_DOUBLE(f32::from_bits(key as u32) as f64),
            DICT_KEY_BOOL => PY_BOOL_FROM_LONG(key as std::os::raw::c_long),
            DICT_KEY_STR => olive_str_to_py(key),
            DICT_KEY_NONE => {
                let none = _PY_NONE_STRUCT as PyObject;
                PY_INC_REF(none);
                none
            }
            DICT_KEY_ANY => to_py_deep(key),
            _ => to_py_deep(key),
        }
    }
}

/// Deep-realizes a dict into a real Python `dict`, using the compiler-supplied
/// key kind for keys and the collection tag for values. Typed scalar values
/// are read raw; `Any` values recurse through the dynamic converter.
unsafe fn to_py_typed_dict(
    val: i64,
    kind: i64,
    key_tag: i64,
    value_f32: bool,
    value_u64: bool,
) -> PyObject {
    unsafe {
        let py_dict = PY_DICT_NEW();
        if py_dict.is_null() {
            return py_dict;
        }
        let obj = &*(val as *const crate::OliveObj);
        for (key, &value) in &obj.fields {
            let py_key = dict_key_to_py(key.0, key_tag);
            if py_key.is_null() {
                PY_DEC_REF(py_dict);
                return std::ptr::null_mut();
            }
            let py_value = if kind == TAG_ANY_DICT {
                to_py_deep(value)
            } else {
                raw_collection_scalar_to_py(value, kind, value_f32, value_u64)
            };
            if py_value.is_null() {
                PY_DEC_REF(py_key);
                PY_DEC_REF(py_dict);
                return std::ptr::null_mut();
            }
            let res = PY_OBJECT_SET_ITEM(py_dict, py_key, py_value);
            PY_DEC_REF(py_key);
            PY_DEC_REF(py_value);
            if res == -1 {
                PY_DEC_REF(py_dict);
                return std::ptr::null_mut();
            }
        }
        py_dict
    }
}

/// Deep-realizes a concretely-typed set (`set[T]`, `T` a scalar) into a real
/// Python `set`, the same raw-by-`kind` reasoning as `to_py_typed_dict`.
unsafe fn to_py_typed_set(val: i64, kind: i64, value_f32: bool, value_u64: bool) -> PyObject {
    unsafe {
        let hs = &*(val as *const crate::OliveHashSet);
        let pys = PY_SET_NEW(std::ptr::null_mut());
        for i in 0..hs.len {
            let v = *hs.ptr.add(i);
            let py_v = raw_collection_scalar_to_py(v, kind, value_f32, value_u64);
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
        TAG_NONE_LIST => "None",
        _ => "value",
    }
}

/// Decodes one Python scalar back into an Olive typed-container element by
/// its static kind. Bool is checked before int (bool subtypes int in
/// CPython): an int slot accepts `True`/`False` as 1/0, mirroring Python's
/// own `int(True) == 1`, while a bool slot rejects a plain `5`. Each
/// direction follows the language's subtyping, not a symmetric gate.
unsafe fn decode_scalar(item: PyObject, kind: i64) -> Result<i64, String> {
    unsafe {
        let ty = raw_ob_type(item);
        let is_sub = |expected: PyObject| {
            !expected.is_null() && (ty == expected || PY_TYPE_IS_SUBTYPE(ty, expected) != 0)
        };
        match kind {
            TAG_INT_LIST => {
                if is_sub(PY_BOOL_TYPE) || is_sub(PY_LONG_TYPE) {
                    let had_error = !PY_ERR_OCCURRED().is_null();
                    let v = py_long_as_i64(item);
                    if had_error {
                        return Ok(v);
                    }
                    if !PY_ERR_OCCURRED().is_null() {
                        PY_ERR_CLEAR();
                        return Err("int (out of range)".to_string());
                    }
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
            TAG_NONE_LIST => {
                if item == _PY_NONE_STRUCT {
                    Ok(0)
                } else {
                    Err(py_type_name(ty))
                }
            }
            _ => unreachable!("decode_scalar: {kind} is not a scalar kind"),
        }
    }
}

unsafe fn decode_collection_scalar(
    item: PyObject,
    kind: i64,
    value_f32: bool,
    value_u64: bool,
) -> Result<i64, String> {
    if value_u64 && kind == TAG_INT_LIST {
        return unsafe { py_u64_from_python(item) }
            .map(|value| value as i64)
            .ok_or_else(|| "int (out of u64 range)".to_string());
    }
    let value = unsafe { decode_scalar(item, kind) }?;
    if value_f32 && kind == TAG_FLOAT_LIST {
        let narrowed = f64::from_bits(value as u64) as f32;
        return Ok(narrowed.to_bits() as i64);
    }
    Ok(value)
}

fn writeback_type_message(loc_desc: &str, tag: i64, actual: &str) -> String {
    format!(
        "writeback type mismatch: {loc_desc} synced back from Python is `{actual}`, expected `{}`",
        expected_name_for_kind(scalar_kind(tag))
    )
}

/// Releases a displaced list element by collection tag. Only `Any` and
/// `str` elements own heap values. Raw scalar lists store unboxed words.
#[inline]
fn free_writeback_elem(val: i64, tag: i64) {
    if tag == TAG_ANY_LIST {
        if crate::is_tagged_str_key(val) {
            crate::olive_free_str(val);
        } else if crate::is_active_object(val) {
            crate::olive_free_any(val);
        }
    } else if tag == TAG_STR_LIST {
        crate::olive_free_str(val);
    }
}

unsafe fn sync_list(pair: &WritebackPair) -> Result<(), String> {
    unsafe {
        let new_len = PY_OBJECT_LENGTH(pair.py_obj).max(0) as usize;
        let old_len = crate::olive_list_len(pair.olive_ptr) as usize;
        let overlap = new_len.min(old_len);
        let kind = scalar_kind(pair.tag);

        let decode = |i: usize| -> Result<i64, String> {
            let item = PY_LIST_GET_ITEM(pair.py_obj, i as isize);
            if pair.tag == TAG_ANY_LIST {
                return Ok(py_to_any_internal(item));
            }
            decode_collection_scalar(item, kind, pair.value_f32, pair.value_u64).map_err(|actual| {
                writeback_type_message(&format!("element {i}"), pair.tag, &actual)
            })
        };

        for i in 0..overlap {
            let val = decode(i)?;
            let old = crate::olive_list_get(pair.olive_ptr, i as i64);
            crate::olive_list_set(pair.olive_ptr, i as i64, val);
            free_writeback_elem(old, pair.tag);
        }
        if new_len > old_len {
            for i in old_len..new_len {
                let val = decode(i)?;
                crate::olive_list_insert(pair.olive_ptr, i as i64, val);
            }
        } else if new_len < old_len {
            for _ in new_len..old_len {
                let popped = crate::olive_list_pop(pair.olive_ptr);
                free_writeback_elem(popped, pair.tag);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum DecodedKeyOwnership {
    Raw,
    Str,
    Any,
}

#[derive(Clone, Copy)]
struct DecodedDictKey {
    word: i64,
    ownership: DecodedKeyOwnership,
}

fn dict_key_format_tag(key_tag: i64) -> u8 {
    match key_tag {
        DICT_KEY_INT | DICT_KEY_U64 => crate::format::D_INT,
        DICT_KEY_FLOAT => crate::format::D_FLOAT,
        DICT_KEY_F32 => crate::format::D_F32,
        DICT_KEY_BOOL => crate::format::D_BOOL,
        DICT_KEY_STR => crate::format::D_STR,
        DICT_KEY_NONE => crate::format::D_NULL,
        _ => crate::format::D_ANY,
    }
}

fn dict_value_format_tag(tag: i64, value_f32: bool) -> u8 {
    match scalar_kind(tag) {
        TAG_INT_LIST => crate::format::D_INT,
        TAG_FLOAT_LIST if value_f32 => crate::format::D_F32,
        TAG_FLOAT_LIST => crate::format::D_FLOAT,
        TAG_BOOL_LIST => crate::format::D_BOOL,
        TAG_STR_LIST => crate::format::D_STR,
        TAG_NONE_LIST => crate::format::D_NULL,
        _ => crate::format::D_ANY,
    }
}

fn free_decoded_dict_key(key: DecodedDictKey) {
    match key.ownership {
        DecodedKeyOwnership::Raw => {}
        DecodedKeyOwnership::Str => crate::string_slab::str_free(key.word),
        DecodedKeyOwnership::Any => crate::free_any_word(key.word),
    }
}

fn free_dict_value(value: i64, tag: i64) {
    match scalar_kind(tag) {
        TAG_INT_LIST | TAG_FLOAT_LIST | TAG_BOOL_LIST => {}
        TAG_STR_LIST => crate::olive_free_str(value),
        _ => crate::free_any_word(value),
    }
}

fn release_decoded_dict_key_after_insert(key: DecodedDictKey) {
    match key.ownership {
        DecodedKeyOwnership::Raw => {}
        DecodedKeyOwnership::Str => crate::string_slab::str_free(key.word),
        DecodedKeyOwnership::Any if crate::is_tagged_str_key(key.word) => {
            crate::olive_free_str(key.word)
        }
        DecodedKeyOwnership::Any => {}
    }
}

unsafe fn decode_dict_key(key_obj: PyObject, key_tag: i64) -> Result<DecodedDictKey, String> {
    unsafe {
        match key_tag {
            DICT_KEY_INT => decode_scalar(key_obj, TAG_INT_LIST).map(|word| DecodedDictKey {
                word,
                ownership: DecodedKeyOwnership::Raw,
            }),
            DICT_KEY_U64 => {
                let key_ty = raw_ob_type(key_obj);
                let is_int = !key_ty.is_null()
                    && ((!PY_LONG_TYPE.is_null() && key_ty == PY_LONG_TYPE)
                        || (!PY_BOOL_TYPE.is_null() && key_ty == PY_BOOL_TYPE));
                if !is_int {
                    return Err(py_type_name(key_ty));
                }
                match py_u64_from_python(key_obj) {
                    Some(value) => Ok(DecodedDictKey {
                        word: value as i64,
                        ownership: DecodedKeyOwnership::Raw,
                    }),
                    None => Err("int (out of u64 range)".to_string()),
                }
            }
            DICT_KEY_FLOAT => decode_scalar(key_obj, TAG_FLOAT_LIST).map(|word| DecodedDictKey {
                word,
                ownership: DecodedKeyOwnership::Raw,
            }),
            DICT_KEY_F32 => decode_scalar(key_obj, TAG_FLOAT_LIST).map(|word| {
                let narrowed = f64::from_bits(word as u64) as f32;
                DecodedDictKey {
                    word: narrowed.to_bits() as i64,
                    ownership: DecodedKeyOwnership::Raw,
                }
            }),
            DICT_KEY_BOOL => decode_scalar(key_obj, TAG_BOOL_LIST).map(|word| DecodedDictKey {
                word,
                ownership: DecodedKeyOwnership::Raw,
            }),
            DICT_KEY_STR => builtin_dict_key_olive(key_obj).map(|word| DecodedDictKey {
                word,
                ownership: DecodedKeyOwnership::Str,
            }),
            DICT_KEY_NONE => {
                if key_obj == _PY_NONE_STRUCT {
                    Ok(DecodedDictKey {
                        word: 0,
                        ownership: DecodedKeyOwnership::Raw,
                    })
                } else {
                    Err(py_type_name(raw_ob_type(key_obj)))
                }
            }
            DICT_KEY_ANY => Ok(DecodedDictKey {
                word: py_to_any_internal(key_obj),
                ownership: DecodedKeyOwnership::Any,
            }),
            _ => builtin_dict_key_olive(key_obj).map(|word| DecodedDictKey {
                word,
                ownership: DecodedKeyOwnership::Str,
            }),
        }
    }
}

fn dict_key_error_text(key: i64, key_tag: i64) -> String {
    if key_tag == DICT_KEY_STR {
        let bytes = crate::olive_str_to_bytes(key);
        format!("{bytes:?}")
    } else {
        key.to_string()
    }
}

fn writeback_key_type_message(loc_desc: &str, key_tag: i64, actual: &str) -> String {
    let expected = match key_tag {
        DICT_KEY_INT | DICT_KEY_U64 => "int",
        DICT_KEY_FLOAT | DICT_KEY_F32 => "float",
        DICT_KEY_BOOL => "bool",
        DICT_KEY_STR => "str",
        DICT_KEY_NONE => "None",
        _ => "Any",
    };
    format!(
        "writeback type mismatch: {loc_desc} synced back from Python is `{actual}`, expected `{expected}`"
    )
}

unsafe fn clear_dict_pair(pair: &WritebackPair) {
    let desc = AlignedDictDescriptor([
        crate::format::D_DICT,
        dict_key_format_tag(pair.key_tag),
        dict_value_format_tag(pair.tag, pair.value_f32),
    ]);
    crate::obj::olive_obj_clear_typed(pair.olive_ptr, desc.0.as_ptr() as i64);
}

unsafe fn builtin_dict_key_olive(key_obj: PyObject) -> Result<i64, String> {
    unsafe {
        let key_ty = raw_ob_type(key_obj);
        let is_unicode = !key_ty.is_null()
            && !PY_UNICODE_TYPE.is_null()
            && (key_ty == PY_UNICODE_TYPE || PY_TYPE_IS_SUBTYPE(key_ty, PY_UNICODE_TYPE) != 0);
        if is_unicode {
            let word = py_str_to_olive(key_obj);
            return if word == 0 {
                Err("str (invalid utf-8)".to_string())
            } else {
                Ok(word)
            };
        }
        let safe_builtin = key_obj == _PY_NONE_STRUCT
            || (!PY_LONG_TYPE.is_null() && key_ty == PY_LONG_TYPE)
            || (!PY_FLOAT_TYPE.is_null() && key_ty == PY_FLOAT_TYPE)
            || (!PY_BOOL_TYPE.is_null() && key_ty == PY_BOOL_TYPE)
            || (!PY_BYTES_TYPE.is_null() && key_ty == PY_BYTES_TYPE);
        if !safe_builtin {
            return Err(py_type_name(key_ty));
        }
        let str_obj = PY_OBJECT_STR(key_obj);
        if str_obj.is_null() {
            return Err("string conversion failed".to_string());
        }
        let word = py_str_to_olive(str_obj);
        PY_DEC_REF(str_obj);
        if word == 0 {
            Err("string conversion failed".to_string())
        } else {
            Ok(word)
        }
    }
}

unsafe fn snapshot_dict_entries(py_dict: PyObject) -> Vec<(PyObject, PyObject)> {
    unsafe {
        let mut entries = Vec::new();
        let mut pos: isize = 0;
        let mut key_obj: PyObject = std::ptr::null_mut();
        let mut val_obj: PyObject = std::ptr::null_mut();
        while PY_DICT_NEXT(py_dict, &mut pos, &mut key_obj, &mut val_obj) != 0 {
            if key_obj.is_null() || val_obj.is_null() {
                continue;
            }
            PY_INC_REF(key_obj);
            PY_INC_REF(val_obj);
            entries.push((key_obj, val_obj));
        }
        entries
    }
}

/// Syncs one dict-shaped pair in place: clears the Olive side, decodes every
/// entry with the static key kind, inserts each distinct key exactly once,
/// then releases any decoded key that was not retained by the dict.
unsafe fn sync_dict_entries(
    pair: &WritebackPair,
    decode_val: impl Fn(PyObject, i64) -> Result<i64, String>,
) -> Result<(), String> {
    unsafe {
        let mut py_entries = snapshot_dict_entries(pair.py_obj).into_iter();
        clear_dict_pair(pair);

        let mut raw: Vec<(DecodedDictKey, i64)> = Vec::new();
        let release_raw = |raw: &mut Vec<(DecodedDictKey, i64)>| {
            for (key, value) in raw.drain(..) {
                free_decoded_dict_key(key);
                free_dict_value(value, pair.tag);
            }
        };
        while let Some((key_obj, val_obj)) = py_entries.next() {
            let key = match decode_dict_key(key_obj, pair.key_tag) {
                Ok(key) => key,
                Err(actual) => {
                    PY_DEC_REF(key_obj);
                    PY_DEC_REF(val_obj);
                    for (key_obj, val_obj) in py_entries {
                        PY_DEC_REF(key_obj);
                        PY_DEC_REF(val_obj);
                    }
                    release_raw(&mut raw);
                    return Err(writeback_key_type_message(
                        "dict key",
                        pair.key_tag,
                        &actual,
                    ));
                }
            };
            let value = match decode_val(val_obj, key.word) {
                Ok(value) => value,
                Err(message) => {
                    free_decoded_dict_key(key);
                    PY_DEC_REF(key_obj);
                    PY_DEC_REF(val_obj);
                    for (key_obj, val_obj) in py_entries {
                        PY_DEC_REF(key_obj);
                        PY_DEC_REF(val_obj);
                    }
                    release_raw(&mut raw);
                    return Err(message);
                }
            };
            PY_DEC_REF(key_obj);
            PY_DEC_REF(val_obj);
            raw.push((key, value));
        }
        dedupe_and_insert(pair.olive_ptr, raw, pair.key_tag, pair.tag);
        Ok(())
    }
}

/// Inserts decoded entries, keeping the last occurrence of each key like
/// Python dict construction does, and releasing every displaced duplicate
/// key and value.
fn dedupe_and_insert(obj_ptr: i64, raw: Vec<(DecodedDictKey, i64)>, key_tag: i64, value_tag: i64) {
    let key_desc = match key_tag {
        DICT_KEY_INT | DICT_KEY_U64 => {
            static DESC: AlignedScalarDescriptor = AlignedScalarDescriptor([crate::format::D_INT]);
            DESC.0.as_ptr() as i64
        }
        DICT_KEY_FLOAT => {
            static DESC: AlignedScalarDescriptor =
                AlignedScalarDescriptor([crate::format::D_FLOAT]);
            DESC.0.as_ptr() as i64
        }
        DICT_KEY_F32 => {
            static DESC: AlignedScalarDescriptor = AlignedScalarDescriptor([crate::format::D_F32]);
            DESC.0.as_ptr() as i64
        }
        DICT_KEY_BOOL => {
            static DESC: AlignedScalarDescriptor = AlignedScalarDescriptor([crate::format::D_BOOL]);
            DESC.0.as_ptr() as i64
        }
        DICT_KEY_STR => {
            static DESC: AlignedScalarDescriptor = AlignedScalarDescriptor([crate::format::D_STR]);
            DESC.0.as_ptr() as i64
        }
        DICT_KEY_NONE => {
            static DESC: AlignedScalarDescriptor = AlignedScalarDescriptor([crate::format::D_NULL]);
            DESC.0.as_ptr() as i64
        }
        _ => 0,
    };

    let mut entries: Vec<(DecodedDictKey, i64)> = Vec::new();
    let mut index_of: rustc_hash::FxHashMap<crate::OliveStringKey, usize> =
        rustc_hash::FxHashMap::default();
    crate::hash_typed::with_key_descriptor(key_desc, || {
        for (key, olive_val) in raw {
            let keyed = crate::OliveStringKey(key.word);
            match index_of.entry(keyed) {
                std::collections::hash_map::Entry::Occupied(occ) => {
                    let idx = *occ.get();
                    let prev_val = entries[idx].1;
                    free_dict_value(prev_val, value_tag);
                    entries[idx].1 = olive_val;
                    free_decoded_dict_key(key);
                }
                std::collections::hash_map::Entry::Vacant(vac) => {
                    vac.insert(entries.len());
                    entries.push((key, olive_val));
                }
            }
        }

        for (key, olive_val) in &entries {
            if key_desc == 0 {
                crate::olive_obj_set(obj_ptr, key.word, *olive_val);
            } else {
                crate::hash_typed::olive_obj_set_typed(obj_ptr, key.word, *olive_val, key_desc);
            }
        }
        for (key, _) in &entries {
            release_decoded_dict_key_after_insert(*key);
        }
    });
}

unsafe fn sync_dict(pair: &WritebackPair) -> Result<(), String> {
    unsafe { sync_dict_entries(pair, |val_obj, _key| Ok(py_to_any_internal(val_obj))) }
}

/// `sync_dict`'s counterpart for a concretely-typed dict: values decode raw
/// by `kind` (an exact-type check, faulting on mismatch) instead of boxing
/// through `py_to_any_internal`, matching the raw storage a `{str: T}` dict
/// actually uses.
unsafe fn sync_dict_typed(pair: &WritebackPair) -> Result<(), String> {
    unsafe {
        let kind = scalar_kind(pair.tag);
        let value_f32 = pair.value_f32;
        sync_dict_entries(pair, |val_obj, key_ptr| {
            decode_collection_scalar(val_obj, kind, value_f32, pair.value_u64).map_err(|actual| {
                let key_text = dict_key_error_text(key_ptr, pair.key_tag);
                writeback_type_message(&format!("value at key {key_text}"), pair.tag, &actual)
            })
        })
    }
}

unsafe fn sync_set(pair: &WritebackPair) -> Result<(), String> {
    unsafe {
        crate::olive_set_clear(pair.olive_ptr);
        let iter = PY_OBJECT_GET_ITER(pair.py_obj);
        if iter.is_null() {
            PY_ERR_CLEAR();
            return Err("set iteration failed".to_string());
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
        Ok(())
    }
}

/// `sync_set`'s counterpart for a concretely-typed set: elements decode raw
/// by `kind` instead of boxing through `py_to_any_internal`.
unsafe fn sync_set_typed(pair: &WritebackPair) -> Result<(), String> {
    static INT_DESC: AlignedScalarDescriptor = AlignedScalarDescriptor([crate::format::D_INT]);
    static FLOAT_DESC: AlignedScalarDescriptor = AlignedScalarDescriptor([crate::format::D_FLOAT]);
    static F32_DESC: AlignedScalarDescriptor = AlignedScalarDescriptor([crate::format::D_F32]);
    static BOOL_DESC: AlignedScalarDescriptor = AlignedScalarDescriptor([crate::format::D_BOOL]);
    static STR_DESC: AlignedScalarDescriptor = AlignedScalarDescriptor([crate::format::D_STR]);
    static NONE_DESC: AlignedScalarDescriptor = AlignedScalarDescriptor([crate::format::D_NULL]);
    static INT_SET_DESC: AlignedSetDescriptor =
        AlignedSetDescriptor([crate::format::D_SET, crate::format::D_INT]);
    static FLOAT_SET_DESC: AlignedSetDescriptor =
        AlignedSetDescriptor([crate::format::D_SET, crate::format::D_FLOAT]);
    static F32_SET_DESC: AlignedSetDescriptor =
        AlignedSetDescriptor([crate::format::D_SET, crate::format::D_F32]);
    static BOOL_SET_DESC: AlignedSetDescriptor =
        AlignedSetDescriptor([crate::format::D_SET, crate::format::D_BOOL]);
    static STR_SET_DESC: AlignedSetDescriptor =
        AlignedSetDescriptor([crate::format::D_SET, crate::format::D_STR]);
    static NONE_SET_DESC: AlignedSetDescriptor =
        AlignedSetDescriptor([crate::format::D_SET, crate::format::D_NULL]);
    unsafe {
        let set_desc = match scalar_kind(pair.tag) {
            TAG_INT_LIST => INT_SET_DESC.0.as_ptr() as i64,
            TAG_FLOAT_LIST if pair.value_f32 => F32_SET_DESC.0.as_ptr() as i64,
            TAG_FLOAT_LIST => FLOAT_SET_DESC.0.as_ptr() as i64,
            TAG_BOOL_LIST => BOOL_SET_DESC.0.as_ptr() as i64,
            TAG_NONE_LIST => NONE_SET_DESC.0.as_ptr() as i64,
            _ => STR_SET_DESC.0.as_ptr() as i64,
        };
        crate::set::olive_set_clear_typed(pair.olive_ptr, set_desc);
        let kind = scalar_kind(pair.tag);
        // Raw scalar elements must hash by static type, not the
        // string-pointer magnitude heuristic: a big odd int (or an odd float
        // bit pattern) is bit-identical to a tagged string pointer, which the
        // untyped op would dereference as string bytes and fault on.
        let key_desc = match kind {
            TAG_INT_LIST => INT_DESC.0.as_ptr() as i64,
            TAG_FLOAT_LIST if pair.value_f32 => F32_DESC.0.as_ptr() as i64,
            TAG_FLOAT_LIST => FLOAT_DESC.0.as_ptr() as i64,
            TAG_BOOL_LIST => BOOL_DESC.0.as_ptr() as i64,
            TAG_NONE_LIST => NONE_DESC.0.as_ptr() as i64,
            _ => STR_DESC.0.as_ptr() as i64,
        };
        let iter = PY_OBJECT_GET_ITER(pair.py_obj);
        if iter.is_null() {
            PY_ERR_CLEAR();
            return Err("set iteration failed".to_string());
        }
        let mut i = 0usize;
        loop {
            let item = PY_ITER_NEXT(iter);
            if item.is_null() {
                PY_ERR_CLEAR();
                break;
            }
            let olive_val =
                match decode_collection_scalar(item, kind, pair.value_f32, pair.value_u64) {
                    Ok(v) => v,
                    Err(actual) => {
                        PY_DEC_REF(item);
                        PY_DEC_REF(iter);
                        return Err(writeback_type_message(
                            &format!("element {i}"),
                            pair.tag,
                            &actual,
                        ));
                    }
                };
            crate::hash_typed::olive_set_add_typed(pair.olive_ptr, olive_val, key_desc);
            PY_DEC_REF(item);
            i += 1;
        }
        PY_DEC_REF(iter);
        Ok(())
    }
}

/// Syncs every tracked collection argument back into its Olive allocation and
/// releases the pair's retained reference. Runs after the underlying
/// `PyObject_Call`/`PyObject_CallObject`, on both the success and the
/// Python-exception path, before either is handled -- Python keeps whatever
/// partial mutation happened before a raise, and Olive must show the same
/// state.
pub(crate) unsafe fn sync_back(pairs: &[WritebackPair]) -> Result<(), String> {
    unsafe {
        for (index, pair) in pairs.iter().enumerate() {
            let result = match pair.tag {
                TAG_ANY_LIST | TAG_INT_LIST | TAG_FLOAT_LIST | TAG_BOOL_LIST | TAG_STR_LIST
                | TAG_NONE_LIST => sync_list(pair),
                TAG_ANY_DICT => sync_dict(pair),
                TAG_NONE_DICT => sync_dict_typed(pair),
                TAG_ANY_SET => sync_set(pair),
                TAG_NONE_SET => sync_set_typed(pair),
                TAG_INT_DICT | TAG_FLOAT_DICT | TAG_BOOL_DICT | TAG_STR_DICT => {
                    sync_dict_typed(pair)
                }
                TAG_INT_SET | TAG_FLOAT_SET | TAG_BOOL_SET | TAG_STR_SET => sync_set_typed(pair),
                _ => Ok(()),
            };
            if let Err(message) = result {
                PY_DEC_REF(pair.py_obj);
                for remaining in &pairs[index + 1..] {
                    PY_DEC_REF(remaining.py_obj);
                }
                return Err(message);
            }
            PY_DEC_REF(pair.py_obj);
        }
        Ok(())
    }
}

pub(crate) unsafe fn sync_back_or_abort(pairs: &[WritebackPair]) {
    if let Err(message) = unsafe { sync_back(pairs) } {
        let loc = py_call_loc();
        let loc = (!loc.is_empty()).then_some(loc);
        crate::panic::abort_py_writeback_type(&message, loc.as_deref());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::{
        DICT_KEY_STR, DecodedDictKey, DecodedKeyOwnership, TAG_ANY_LIST, TAG_INT_LIST,
        TAG_STR_DICT, TAG_STR_LIST, dedupe_and_insert, free_writeback_elem,
    };

    #[test]
    fn str_tag_releases_displaced() {
        let s = crate::olive_str_internal("old");
        let g = crate::string_slab::olive_str_gen_of(s);
        free_writeback_elem(s, TAG_STR_LIST);
        assert_eq!(crate::string_slab::olive_str_gen_stale(s, g), 1);
    }

    #[test]
    fn any_tag_releases_displaced_string() {
        let s = crate::olive_str_internal("old-any");
        let g = crate::string_slab::olive_str_gen_of(s);
        free_writeback_elem(s, TAG_ANY_LIST);
        assert_eq!(crate::string_slab::olive_str_gen_stale(s, g), 1);
    }

    #[test]
    fn any_tag_releases_displaced_heap_object() {
        let list = crate::list::list_from_vec(vec![1]);
        free_writeback_elem(list, TAG_ANY_LIST);
        assert!(!crate::slab::slot_is_live(list));
    }

    #[test]
    fn int_tag_keeps_raw_word() {
        free_writeback_elem(42, TAG_INT_LIST);
        assert_eq!(crate::olive_list_len(0), 0);
    }

    #[test]
    fn dedupe_releases_displaced_string_key_and_value() {
        let dict = crate::obj::olive_obj_new();
        let k1 = crate::olive_str_internal("1");
        let v1 = crate::olive_str_internal("a");
        let gk1 = crate::string_slab::olive_str_gen_of(k1);
        let gv1 = crate::string_slab::olive_str_gen_of(v1);
        let k2 = crate::olive_str_internal("1");
        let v2 = crate::olive_str_internal("b");
        let gk2 = crate::string_slab::olive_str_gen_of(k2);
        let gv2 = crate::string_slab::olive_str_gen_of(v2);
        dedupe_and_insert(
            dict,
            vec![
                (
                    DecodedDictKey {
                        word: k1,
                        ownership: DecodedKeyOwnership::Str,
                    },
                    v1,
                ),
                (
                    DecodedDictKey {
                        word: k2,
                        ownership: DecodedKeyOwnership::Str,
                    },
                    v2,
                ),
            ],
            DICT_KEY_STR,
            TAG_STR_DICT,
        );
        assert_eq!(crate::string_slab::olive_str_gen_stale(k2, gk2), 1);
        assert_eq!(crate::string_slab::olive_str_gen_stale(v1, gv1), 1);
        assert_eq!(crate::string_slab::olive_str_gen_stale(k1, gk1), 1);
        assert_eq!(crate::string_slab::olive_str_gen_stale(v2, gv2), 0);
        let q = crate::olive_str_internal("1");
        let stored = crate::obj::olive_obj_get(dict, q);
        assert_eq!(crate::olive_str_from_ptr(stored), "b");
        crate::olive_free_str(q);
        crate::obj::olive_free_obj(dict);
        assert_eq!(crate::string_slab::olive_str_gen_stale(v2, gv2), 1);
    }

    #[test]
    fn dedupe_keeps_raw_scalar_values_intact() {
        let dict = crate::obj::olive_obj_new();
        let k1 = crate::olive_str_internal("1");
        let k2 = crate::olive_str_internal("1");
        let gk1 = crate::string_slab::olive_str_gen_of(k1);
        dedupe_and_insert(
            dict,
            vec![
                (
                    DecodedDictKey {
                        word: k1,
                        ownership: DecodedKeyOwnership::Str,
                    },
                    42,
                ),
                (
                    DecodedDictKey {
                        word: k2,
                        ownership: DecodedKeyOwnership::Str,
                    },
                    43,
                ),
            ],
            DICT_KEY_STR,
            TAG_STR_DICT,
        );
        assert_eq!(crate::string_slab::olive_str_gen_stale(k1, gk1), 1);
        let q = crate::olive_str_internal("1");
        assert_eq!(crate::obj::olive_obj_get(dict, q), 43);
        crate::olive_free_str(q);
        crate::string_slab::str_free(k2);
        crate::obj::olive_free_obj(dict);
    }
}
