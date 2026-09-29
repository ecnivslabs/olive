//! R19: exports an Olive function value as a genuine Python `PyCFunction`,
//! so Python code (`sorted(xs, key=fn)`, `dataset.map(fn)`, framework
//! hooks) can hold and invoke it like any other callable -- a C-function
//! callback that also dispatches faster in CPython than a pure-Python one.
//!
//! Reuses R5's tag vocabulary (`python_writeback::ARG_*`) for both
//! directions and the uniform closure-record layout every `Type::Fn`
//! value already has (`closures.rs::build_closure_value`): word 1 is the
//! calling thunk, word 2 the type descriptor -- `translate_call.rs`'s
//! indirect-call branch and `translate.rs`'s `Drop`-for-`Fn` branch read
//! the exact same two words for the exact same reasons (call it, free
//! it), so this is a third reader of an already-established layout, not a
//! new one. Ownership of the record transfers into the returned capsule
//! (`RUNTIME_ESCAPES` marks the compiler's call-site argument as
//! escaping); the capsule's destructor is the one that eventually frees
//! it, via the same generic descriptor-driven free path a normal closure
//! drop uses.

mod dispatch;

use crate::python::python_writeback::{
    ARG_ANY, ARG_ANY_LIST, ARG_BOOL, ARG_BOOL_LIST, ARG_BYTES, ARG_FLOAT, ARG_FLOAT_LIST, ARG_INT,
    ARG_INT_LIST, ARG_NONE, ARG_PYOBJECT, ARG_SCALAR_F32, ARG_SCALAR_U64, ARG_STR, ARG_STR_LIST,
    decode_scalar_arg_wide,
};
use crate::python::*;
use std::os::raw::{c_char, c_void};

// Short on purpose: `PyCapsule_GetPointer` validates this by `strcmp`
// on every single call the callable receives.
const CAPSULE_NAME: &[u8] = b"ofn\0";
const METHOD_NAME: &[u8] = b"olive_fn\0";

/// Heap-boxed, owned by the capsule for the callable's lifetime.
/// `thunk_ptr`/`record_ptr` are the closure record's own word 1 (read once
/// at creation, not on every call) and the record pointer itself, kept
/// alive as the thunk's trailing env argument and freed together on
/// capsule teardown.
struct CallableDescriptor {
    record_ptr: i64,
    thunk_ptr: i64,
    tags: i64,
}

fn arity_of(tags: i64) -> i64 {
    (((tags as u64) >> 56) & 0xF) as i64
}
fn ret_tag_of(tags: i64) -> i64 {
    ((tags as u64) >> 60) as i64
}
fn param_tag_at(tags: i64, i: i64) -> i64 {
    (((tags as u64) >> (i as u32 * 4)) & 0xF) as i64
}

/// Pymodule-export list width flags, packed into the tag word's spare bits
/// 16-25 by `type_to_arg_tag`'s caller (`setup/pymodule.rs::collect_exports`).
/// The base 4-bit param/return tags cannot distinguish `[float]` from `[f32]`
/// or `[int]` from `[u64]` (one 4-bit slot free, two widths needed), so each
/// `[f32]`/`[u64]` list sets one flag bit here while keeping its `float`/`int`
/// base tag. Bit positions must match the codegen side exactly.
const LIST_F32_PARAM_SHIFT: u32 = 16;
const LIST_U64_PARAM_SHIFT: u32 = 20;
const RET_F32_LIST_FLAG: i64 = 1 << 24;
const RET_U64_LIST_FLAG: i64 = 1 << 25;

fn param_list_width(tags: i64, i: usize) -> (bool, bool) {
    (
        ((tags >> (LIST_F32_PARAM_SHIFT + i as u32)) & 1) != 0,
        ((tags >> (LIST_U64_PARAM_SHIFT + i as u32)) & 1) != 0,
    )
}

fn ret_list_width(tags: i64) -> (bool, bool) {
    (
        (tags & RET_F32_LIST_FLAG) != 0,
        (tags & RET_U64_LIST_FLAG) != 0,
    )
}

/// Leaked static descriptor bytes for the two list ret/param tags, laid out
/// exactly like `type_descriptor`'s encoding so `olive_free_typed` walks the
/// same element shapes a compiled Drop does. The untyped `olive_free_list`
/// skips tagged-string elements (`is_active_object` rejects odd words), so
/// an `[Any]` or `[str]` decoded from a Python list would leak one heap
/// string per element per call without this.
fn list_desc(elem: u8) -> i64 {
    static ANY_DESC: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    static STR_DESC: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let slot = match elem {
        crate::format::D_ANY => &ANY_DESC,
        _ => &STR_DESC,
    };
    *slot.get_or_init(|| {
        Box::leak(vec![crate::format::D_LIST, elem].into_boxed_slice()).as_ptr() as i64
    })
}

fn methoddef(fastcall: bool) -> &'static PyMethodDef {
    static VARARGS: std::sync::OnceLock<PyMethodDef> = std::sync::OnceLock::new();
    static FASTCALL: std::sync::OnceLock<PyMethodDef> = std::sync::OnceLock::new();
    if fastcall {
        FASTCALL.get_or_init(|| PyMethodDef {
            ml_name: METHOD_NAME.as_ptr() as *const c_char,
            ml_meth: trampoline_fastcall as *const c_void,
            ml_flags: METH_FASTCALL,
            ml_doc: std::ptr::null(),
        })
    } else {
        VARARGS.get_or_init(|| PyMethodDef {
            ml_name: METHOD_NAME.as_ptr() as *const c_char,
            ml_meth: trampoline_varargs as *const c_void,
            ml_flags: METH_VARARGS,
            ml_doc: std::ptr::null(),
        })
    }
}

/// Decodes one incoming Python argument to a raw olive word by its static
/// param tag. Mirrors `python_ret::finish_ret`'s Python-result decode, but
/// for a borrowed argument (no decref) instead of a consumed call result.
///
/// Exact-type fast path for `ARG_INT`/`ARG_STR`, the same reasoning
/// `py_to_olive_internal` already uses for its own exact-type dispatch: on
/// the overwhelmingly common case (a real `int`/`str`, not a subclass or a
/// foreign numeric type) this skips `PyNumber_Long`'s extra
/// alloc-or-incref-then-decref round trip and `PyObject_Str`'s extra
/// incref-then-decref, both of which are on the hot path for every call a
/// Python-driven loop (`map`, `sorted`) makes into an exported callback.
///
/// Returns `None` when the argument cannot convert, with a Python error set:
/// an overflowing `int` propagates its `OverflowError`, an unconvertible one
/// its `TypeError`, and a failed `str` decode gets a `TypeError` so the
/// trampoline returns null to Python instead of handing the thunk a null
/// string word. List params decode by element tag (`[f32]` narrows to 32-bit
/// words, `[u64]` rejects negatives) rather than the old untyped path, which
/// stored whatever `py_to_olive_internal` guessed.
///
/// Scalar element tags for `py_to_typed_scalar_internal`: 1 int, 2 float,
/// 3 f32, 4 bool, 5 str, 7 u64.
unsafe fn decode_py_arg(obj: PyObject, tag: i64, list_f32: bool, list_u64: bool) -> Option<i64> {
    unsafe {
        match tag {
            ARG_INT | ARG_BOOL => {
                if raw_ob_type(obj) == PY_LONG_TYPE {
                    let v = py_long_as_i64(obj);
                    if !PY_ERR_OCCURRED().is_null() {
                        return None;
                    }
                    Some(v)
                } else {
                    let int_obj = PY_NUMBER_LONG(obj);
                    if int_obj.is_null() {
                        return None;
                    }
                    let v = py_long_as_i64(int_obj);
                    PY_DEC_REF(int_obj);
                    if !PY_ERR_OCCURRED().is_null() {
                        return None;
                    }
                    Some(v)
                }
            }
            ARG_FLOAT => {
                let v = PY_FLOAT_AS_DOUBLE(obj);
                if v == -1.0 && !PY_ERR_OCCURRED().is_null() {
                    return None;
                }
                Some(v.to_bits() as i64)
            }
            ARG_SCALAR_F32 => {
                let v = PY_FLOAT_AS_DOUBLE(obj);
                if v == -1.0 && !PY_ERR_OCCURRED().is_null() {
                    return None;
                }
                Some((v as f32).to_bits() as i64)
            }
            ARG_SCALAR_U64 => match py_u64_from_python(obj) {
                Some(v) => Some(v as i64),
                None => None,
            },
            ARG_STR => {
                if raw_ob_type(obj) == PY_UNICODE_TYPE {
                    let v = py_str_to_olive(obj);
                    if v == 0 {
                        if PY_ERR_OCCURRED().is_null() {
                            raise_type_error(
                                "olive callback: str argument is not valid UTF-8",
                            );
                        }
                        return None;
                    }
                    Some(v)
                } else {
                    let str_obj = PY_OBJECT_STR(obj);
                    if str_obj.is_null() {
                        return None;
                    }
                    let v = py_str_to_olive(str_obj);
                    PY_DEC_REF(str_obj);
                    if v == 0 {
                        if PY_ERR_OCCURRED().is_null() {
                            raise_type_error(
                                "olive callback: str argument is not valid UTF-8",
                            );
                        }
                        return None;
                    }
                    Some(v)
                }
            }
            ARG_PYOBJECT => Some(olive_py_wrap_borrowed(obj) as i64),
            ARG_NONE => Some(0),
            ARG_BYTES => Some(crate::python::python_coerce_ffi::olive_py_to_bytes(obj)),
            ARG_ANY => Some(crate::python::python_coerce::py_to_any_internal(obj)),
            ARG_FLOAT_LIST => Some(crate::python::python_coerce::olive_py_to_list_tagged_internal(
                obj,
                if list_f32 { 3 } else { 2 },
                false,
            )),
            ARG_INT_LIST => Some(crate::python::python_coerce::olive_py_to_list_tagged_internal(
                obj,
                if list_u64 { 7 } else { 1 },
                false,
            )),
            ARG_STR_LIST => Some(crate::python::python_coerce::olive_py_to_list_tagged_internal(
                obj, 5, false,
            )),
            ARG_BOOL_LIST => Some(crate::python::python_coerce::olive_py_to_list_tagged_internal(
                obj, 4, false,
            )),
            ARG_ANY_LIST => {
                // Python `list` → boxed Olive `[Any]`. Each element goes through
                // `py_to_any_internal` so scalars carry their inline tag bits.
                Some(crate::python::python_coerce::olive_py_to_list_internal(obj, true))
            }
            _ => {
                raise_type_error("olive callback: unsupported argument type");
                return None;
            }
        }
    }
}

/// Releases one trampoline-decoded argument word by its param tag, the same
/// ownership the post-call cleanup applies on the success path. Used both
/// there and when a later argument fails to decode, so a failed call leaks
/// nothing it already converted.
unsafe fn free_decoded_arg(word: i64, tag: i64) {
    match tag {
        ARG_PYOBJECT => olive_py_decref(word as PyObject),
        ARG_STR => crate::string_slab::str_free(word),
        ARG_ANY_LIST => {
            crate::free_typed::olive_free_typed(word, list_desc(crate::format::D_ANY));
        }
        ARG_STR_LIST => {
            crate::free_typed::olive_free_typed(word, list_desc(crate::format::D_STR));
        }
        ARG_FLOAT_LIST | ARG_INT_LIST | ARG_BOOL_LIST => {
            crate::olive_free_list(word);
        }
        ARG_BYTES | ARG_ANY => crate::olive_free_any(word),
        _ => {}
    }
}

unsafe fn raise_type_error(msg: &str) -> PyObject {
    unsafe {
        if !PY_EXC_TYPE_ERROR.is_null() {
            let c_msg = format!("{msg}\0");
            PY_ERR_SET_STRING(PY_EXC_TYPE_ERROR, c_msg.as_ptr() as *const c_char);
        }
        std::ptr::null_mut()
    }
}

unsafe fn raise_arity_error(want: i64, got: usize) -> PyObject {
    unsafe {
        let plural = if want == 1 { "" } else { "s" };
        raise_type_error(&format!(
            "olive callback takes {want} argument{plural} ({got} given)"
        ))
    }
}

unsafe fn run_trampoline(
    self_capsule: PyObject,
    argc: usize,
    get_arg: impl Fn(usize) -> PyObject,
) -> PyObject {
    unsafe {
        let raw = PY_CAPSULE_GET_POINTER(self_capsule, CAPSULE_NAME.as_ptr() as *const c_char);
        if raw.is_null() {
            return raise_type_error("olive callback: invalid or stale capsule");
        }
        let descriptor = &*(raw as *const CallableDescriptor);
        let tags = descriptor.tags;
        let arity = arity_of(tags);
        if argc as i64 != arity {
            return raise_arity_error(arity, argc);
        }
        let _external_guard = crate::python::python_subinterp::ExternalCallbackGuard::enter();

        let mut args = [0i64; 4];
        let mut param_tags = [0i64; 4];
        let mut arg_kinds = [0u8; 4];
        for (i, slot) in args.iter_mut().enumerate().take(arity as usize) {
            let tag = param_tag_at(tags, i as i64);
            param_tags[i] = tag;
            let (list_f32, list_u64) = param_list_width(tags, i);
            match decode_py_arg(get_arg(i), tag, list_f32, list_u64) {
                Some(v) => *slot = v,
                None => {
                    for j in 0..i {
                        free_decoded_arg(args[j], param_tags[j]);
                    }
                    return std::ptr::null_mut();
                }
            }
            arg_kinds[i] = match tag {
                ARG_FLOAT => 1,
                ARG_SCALAR_F32 => 2,
                _ => 0,
            };
        }

        let ret_tag = ret_tag_of(tags);
        if !(0..=4).contains(&arity) || arity as usize > 4 {
            for j in 0..arity as usize {
                if j < 4 {
                    free_decoded_arg(args[j], param_tags[j]);
                }
            }
            return raise_type_error("olive callback: invalid arity");
        }
        let ret_kind = match ret_tag {
            ARG_FLOAT => 1,
            ARG_SCALAR_F32 => 2,
            _ => 0,
        };
        if !PY_ERR_OCCURRED().is_null() {
            PY_ERR_CLEAR();
        }
        let raw_result = dispatch::invoke_thunk(
            descriptor.thunk_ptr,
            descriptor.record_ptr,
            &args[..arity as usize],
            arg_kinds,
            ret_kind,
        );
        if !PY_ERR_OCCURRED().is_null() {
            for i in 0..arity as usize {
                free_decoded_arg(args[i], param_tags[i]);
            }
            return std::ptr::null_mut();
        }

        let (ret_f32, ret_u64) = ret_list_width(tags);
        let py_result = decode_scalar_arg_wide(raw_result, ret_tag, ret_f32, ret_u64);
        // Thunk parameters are borrowed (every compiled fn declares its params
        // non-owning; a returned param escapes through a deep copy), so the
        // decoded allocations are ours to release once the call returns.
        for i in 0..arity as usize {
            free_decoded_arg(args[i], param_tags[i]);
        }
        // The thunk's return value is an owned olive value (ordinary
        // move-return convention); `decode_scalar_arg_wide` only ever *reads*
        // it, so the olive-side resource still needs releasing here.
        match ret_tag {
            ARG_PYOBJECT => olive_py_decref(raw_result as PyObject),
            ARG_STR => crate::string_slab::str_free(raw_result),
            ARG_ANY_LIST => {
                crate::free_typed::olive_free_typed(raw_result, list_desc(crate::format::D_ANY));
            }
            ARG_STR_LIST => {
                crate::free_typed::olive_free_typed(raw_result, list_desc(crate::format::D_STR));
            }
            ARG_FLOAT_LIST | ARG_INT_LIST | ARG_BOOL_LIST => {
                crate::olive_free_list(raw_result);
            }
            ARG_BYTES | ARG_ANY => crate::olive_free_any(raw_result),
            _ => {}
        }
        // A null PyObject handle is Olive's spelling of Python `None` (every
        // ingest path maps `Py_None` and null alike to the zero word), so a
        // `PyObject`-returning export hands back real `None` instead of
        // tripping the failure path below.
        let py_result = if py_result.is_null() && ret_tag == ARG_PYOBJECT {
            let none = _PY_NONE_STRUCT as PyObject;
            PY_INC_REF(none);
            none
        } else {
            py_result
        };
        if py_result.is_null() {
            if PY_ERR_OCCURRED().is_null() {
                return raise_type_error("olive callback: failed to convert return value");
            }
            return std::ptr::null_mut();
        }
        py_result
    }
}

unsafe extern "C" fn trampoline_varargs(self_capsule: PyObject, args_tuple: PyObject) -> PyObject {
    unsafe {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let argc = PY_TUPLE_SIZE(args_tuple).max(0) as usize;
            run_trampoline(self_capsule, argc, |i| {
                PY_TUPLE_GET_ITEM(args_tuple, i as isize)
            })
        }));
        match result {
            Ok(py_result) => py_result,
            Err(_) => {
                if PY_ERR_OCCURRED().is_null() {
                    raise_type_error("olive callback panicked")
                } else {
                    std::ptr::null_mut()
                }
            }
        }
    }
}

unsafe extern "C" fn trampoline_fastcall(
    self_capsule: PyObject,
    args: *const PyObject,
    nargs: isize,
) -> PyObject {
    unsafe {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let argc = nargs.max(0) as usize;
            run_trampoline(self_capsule, argc, |i| *args.add(i))
        }));
        match result {
            Ok(py_result) => py_result,
            Err(_) => {
                if PY_ERR_OCCURRED().is_null() {
                    raise_type_error("olive callback panicked")
                } else {
                    std::ptr::null_mut()
                }
            }
        }
    }
}

/// Frees the closure record ownership transferred in by
/// `olive_py_make_callable`: reads the record's own `__desc` field (word
/// 2, low tag bit stripped -- it's an ordinary tagged `Constant::Str`,
/// same convention `translate.rs`'s `Drop`-for-`Fn` branch strips) and
/// hands both to the same generic descriptor-driven free path, so
/// captures free recursively exactly like an ordinary closure drop would.
unsafe fn free_closure_record(record_ptr: i64) {
    unsafe {
        if record_ptr == 0 {
            return;
        }
        let desc_tagged = *((record_ptr as *const i64).add(2));
        let desc_ptr = crate::string_slab::str_body(desc_tagged);
        crate::free_typed::olive_free_typed(record_ptr, desc_ptr);
    }
}

unsafe extern "C" fn capsule_deleter(capsule: PyObject) {
    unsafe {
        let raw = PY_CAPSULE_GET_POINTER(capsule, CAPSULE_NAME.as_ptr() as *const c_char);
        if raw.is_null() {
            return;
        }
        let descriptor = Box::from_raw(raw as *mut CallableDescriptor);
        free_closure_record(descriptor.record_ptr);
    }
}

/// Wraps an Olive closure-record value as a genuine `PyCFunction`. `tags`
/// packs, low to high: up to 4 per-param `ARG_*` tags (4 bits each, bits
/// 0-15), pymodule-export list widths (bits 16-23 params, 24-25 return, see
/// `param_list_width`/`ret_list_width`), the real arity (bits 56-59), and
/// the return `ARG_*` tag (bits 60-63, mirroring `python_ret::ret_tag_of`'s
/// bit position). `record_ptr`
/// is the closure record `build_closure_value` built for the source
/// `Type::Fn` value -- a bare top-level fn's record has zero captures, a
/// capturing closure's has one field per capture, both read identically
/// here since only word 1 (the thunk) and word 2 (the descriptor) matter
/// at this boundary.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_make_callable(record_ptr: i64, tags: i64) -> PyObject {
    check_python_loaded();
    with_gil(|| unsafe {
        if record_ptr == 0 {
            return std::ptr::null_mut();
        }
        let thunk_ptr = *((record_ptr as *const i64).add(1));
        let desc_tagged = *((record_ptr as *const i64).add(2));
        let desc = crate::string_slab::str_body(desc_tagged);
        let source_record = record_ptr;
        let record_ptr = crate::copy_typed::olive_relocate_typed(source_record, desc);
        crate::free_typed::olive_free_typed(source_record, desc);
        let descriptor = Box::into_raw(Box::new(CallableDescriptor {
            record_ptr,
            thunk_ptr,
            tags,
        }));

        let capsule = PY_CAPSULE_NEW(
            descriptor as *mut c_void,
            CAPSULE_NAME.as_ptr() as *const c_char,
            Some(capsule_deleter),
        );
        if capsule.is_null() {
            // The capsule never took ownership: reclaim both ourselves
            // instead of leaking the descriptor and the closure record.
            let descriptor = Box::from_raw(descriptor);
            free_closure_record(descriptor.record_ptr);
            handle_py_error();
            return std::ptr::null_mut();
        }

        let use_fastcall = HAS_VECTORCALL.load(std::sync::atomic::Ordering::Relaxed);
        let def = methoddef(use_fastcall) as *const PyMethodDef as *mut PyMethodDef;
        let callable = PY_CFUNCTION_NEW_EX(def, capsule, std::ptr::null_mut());
        // `PyCFunction_NewEx` takes its own reference to `self`; release
        // the one `PyCapsule_New` handed us so the callable ends up sole owner.
        PY_DEC_REF(capsule);
        if callable.is_null() {
            handle_py_error();
        }
        olive_py_wrap_owned(callable)
    })
}

/// Wraps a top-level compiled function pointer for module export (R20).
/// Unlike `olive_py_make_callable`, this does not reference a heap-allocated
/// closure record with captures — the function IS the thunk, and no record
/// needs freeing on capsule teardown.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_make_export(fn_ptr: i64, tags: i64) -> PyObject {
    check_python_loaded();
    with_gil(|| unsafe {
        if fn_ptr == 0 {
            return std::ptr::null_mut();
        }
        let desc = Box::into_raw(Box::new(CallableDescriptor {
            record_ptr: fn_ptr,
            thunk_ptr: fn_ptr,
            tags,
        }));
        let capsule = PY_CAPSULE_NEW(
            desc as *mut c_void,
            CAPSULE_NAME.as_ptr() as *const c_char,
            Some(capsule_deleter_export),
        );
        if capsule.is_null() {
            drop(Box::from_raw(desc));
            handle_py_error();
            return std::ptr::null_mut();
        }
        let use_fastcall = HAS_VECTORCALL.load(std::sync::atomic::Ordering::Relaxed);
        let def = methoddef(use_fastcall) as *const PyMethodDef as *mut PyMethodDef;
        let callable = PY_CFUNCTION_NEW_EX(def, capsule, std::ptr::null_mut());
        PY_DEC_REF(capsule);
        if callable.is_null() {
            handle_py_error();
        }
        olive_py_wrap_owned(callable)
    })
}

unsafe extern "C" fn capsule_deleter_export(capsule: PyObject) {
    unsafe {
        let raw = PY_CAPSULE_GET_POINTER(capsule, CAPSULE_NAME.as_ptr() as *const c_char);
        if raw.is_null() {
            return;
        }
        drop(Box::from_raw(raw as *mut CallableDescriptor));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::python_coerce::pyobject_slab_test_lock;
    use std::ffi::CString;

    /// `D_STRUCT` byte descriptor for a struct with `n` plain `D_INT`
    /// fields (empty field/type names -- `free_typed`'s reader discards
    /// them, only the shape matters), matching a zero-capture closure
    /// record's real field types (`__thunk`/`__desc` are both stored as
    /// `Type::Int`, see `closures.rs::build_closure_value`). Leaked so the
    /// returned pointer is valid for the process lifetime, same as a real
    /// compiled program's interned descriptor constant.
    fn plain_int_struct_desc(n: u8) -> i64 {
        let mut bytes = vec![12u8, 13u8, 13 + n]; // D_STRUCT, name_len=0, field_count=n
        for _ in 0..n {
            bytes.push(13); // field name_len=0
            bytes.push(1); // D_INT
        }
        let leaked: &'static [u8] = bytes.leak();
        (leaked.as_ptr() as i64) | 1
    }

    /// Builds a closure record by hand (bypassing the MIR builder, which
    /// isn't reachable from a std_lib unit test): a `[header, thunk,
    /// desc, ..extra]` heap struct via `__olive_struct_alloc`, with a
    /// hand-written `extern "C"` thunk standing in for a compiled one.
    unsafe fn make_record(thunk: *const c_void, extra: &[i64]) -> i64 {
        unsafe {
            let n_fields = 2 + extra.len();
            let record = crate::olive_struct_alloc(n_fields as i64);
            *((record as *mut i64).add(1)) = thunk as i64;
            *((record as *mut i64).add(2)) = plain_int_struct_desc(n_fields as u8);
            for (i, &v) in extra.iter().enumerate() {
                *((record as *mut i64).add(3 + i)) = v;
            }
            record
        }
    }

    unsafe extern "C" fn add_one_thunk(_env: i64) -> i64 {
        42
    }

    fn make_tags(arity: i64, params: &[i64], ret: i64) -> i64 {
        let mut tags = (arity << 56) | (ret << 60);
        for (i, &p) in params.iter().enumerate() {
            tags |= p << (i * 4);
        }
        tags
    }

    #[test]
    fn wrong_arity_raises_type_error_not_abort() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let record = make_record(add_one_thunk as *const c_void, &[]);
            let tags = make_tags(0, &[], ARG_INT);
            let handle = olive_py_make_callable(record, tags);
            assert!(!handle.is_null());

            with_gil(|| {
                let callable = olive_py_unwrap(handle);
                let args = PY_TUPLE_NEW(1);
                PY_TUPLE_SET_ITEM(args, 0, PY_LONG_FROM_LONG(1));
                let res = PY_OBJECT_CALL_OBJECT(callable, args);
                PY_DEC_REF(args);
                assert!(res.is_null(), "wrong arity must fail the call");
                assert!(
                    !PY_ERR_OCCURRED().is_null(),
                    "wrong arity must raise, not silently fail"
                );
                PY_ERR_CLEAR();
            });
            olive_py_decref(handle);
        }
    }

    #[test]
    fn zero_arity_int_return_calls_through() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let record = make_record(add_one_thunk as *const c_void, &[]);
            let tags = make_tags(0, &[], ARG_INT);
            let handle = olive_py_make_callable(record, tags);
            assert!(!handle.is_null());

            let result = with_gil(|| {
                let callable = olive_py_unwrap(handle);
                let args = PY_TUPLE_NEW(0);
                let res = PY_OBJECT_CALL_OBJECT(callable, args);
                PY_DEC_REF(args);
                assert!(!res.is_null());
                let v = py_long_as_i64(res);
                PY_DEC_REF(res);
                v
            });
            assert_eq!(result, 42);
            olive_py_decref(handle);
        }
    }

    #[test]
    fn callable_record_survives_task_slab_teardown() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let mut slabs = Box::new(crate::slab::SlabSet::new());
            let old_active = crate::slab::ACTIVE_SLABS.get();
            crate::slab::ACTIVE_SLABS.set(slabs.as_mut() as *mut crate::slab::SlabSet);
            let record = make_record(add_one_thunk as *const c_void, &[]);
            let handle = olive_py_make_callable(record, make_tags(0, &[], ARG_INT));
            crate::slab::ACTIVE_SLABS.set(old_active);
            drop(slabs);
            assert!(!handle.is_null());

            with_gil(|| {
                let callable = olive_py_unwrap(handle);
                let args = PY_TUPLE_NEW(0);
                let result = PY_OBJECT_CALL_OBJECT(callable, args);
                PY_DEC_REF(args);
                assert!(!result.is_null());
                assert_eq!(py_long_as_i64(result), 42);
                PY_DEC_REF(result);
            });
            olive_py_decref(handle);
        }
    }

    #[test]
    fn capsule_destructor_runs_no_leak_over_many_cycles() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        for _ in 0..10_000 {
            unsafe {
                let record = make_record(add_one_thunk as *const c_void, &[]);
                let tags = make_tags(0, &[], ARG_INT);
                let handle = olive_py_make_callable(record, tags);
                assert!(!handle.is_null());
                olive_py_decref(handle);
                assert!(
                    !crate::slab::ptr_in_slab_span(record) || !crate::slab::slot_is_live(record),
                    "closure record must be freed once the capsule is collected"
                );
            }
        }
    }

    #[test]
    fn str_and_float_params_roundtrip() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe extern "C" fn concat_len_thunk(s: i64, f: f64, _env: i64) -> f64 {
            crate::olive_str_from_ptr(s).len() as f64 + f
        }
        unsafe {
            let record = make_record(concat_len_thunk as *const c_void, &[]);
            let tags = make_tags(2, &[ARG_STR, ARG_FLOAT], ARG_FLOAT);
            let handle = olive_py_make_callable(record, tags);
            assert!(!handle.is_null());

            let result = with_gil(|| {
                let callable = olive_py_unwrap(handle);
                let args = PY_TUPLE_NEW(2);
                let s = CString::new("hello").unwrap();
                PY_TUPLE_SET_ITEM(args, 0, PY_UNICODE_FROM_STRING(s.as_ptr()));
                PY_TUPLE_SET_ITEM(args, 1, PY_FLOAT_FROM_DOUBLE(0.5));
                let res = PY_OBJECT_CALL_OBJECT(callable, args);
                PY_DEC_REF(args);
                assert!(!res.is_null());
                let v = PY_FLOAT_AS_DOUBLE(res);
                PY_DEC_REF(res);
                v
            });
            assert_eq!(result, 5.5);
            olive_py_decref(handle);
        }
    }

    #[test]
    fn pyobject_param_and_return_roundtrip() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        // Identity callback: `PyObject -> PyObject`, wraps/unwraps once each
        // way, exercising `ARG_PYOBJECT` on both the param and return tag.
        unsafe extern "C" fn identity_thunk(obj: i64, _env: i64) -> i64 {
            obj
        }
        unsafe {
            let record = make_record(identity_thunk as *const c_void, &[]);
            let tags = make_tags(1, &[ARG_PYOBJECT], ARG_PYOBJECT);
            let handle = olive_py_make_callable(record, tags);
            assert!(!handle.is_null());

            let result = with_gil(|| {
                let callable = olive_py_unwrap(handle);
                let args = PY_TUPLE_NEW(1);
                PY_TUPLE_SET_ITEM(args, 0, PY_LONG_FROM_LONG(99));
                let res = PY_OBJECT_CALL_OBJECT(callable, args);
                PY_DEC_REF(args);
                assert!(!res.is_null());
                let v = py_long_as_i64(res);
                PY_DEC_REF(res);
                v
            });
            assert_eq!(result, 99);
            olive_py_decref(handle);
        }
    }

    #[test]
    fn reentrant_callback_calling_back_into_python() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        // The thunk itself calls back into Python (via `with_gil`, nested)
        // while already inside the trampoline's own `with_gil` region --
        // `GIL_DEPTH` must make the inner call a no-op re-entry, not a
        // second `PyGILState_Ensure`.
        unsafe extern "C" fn reentrant_thunk(env: i64) -> i64 {
            crate::python::with_gil(|| unsafe {
                let obj = PY_LONG_FROM_LONG(7);
                PY_DEC_REF(obj);
            });
            env
        }
        unsafe {
            let record = make_record(reentrant_thunk as *const c_void, &[]);
            let tags = make_tags(0, &[], ARG_INT);
            let handle = olive_py_make_callable(record, tags);
            assert!(!handle.is_null());

            with_gil(|| {
                let callable = olive_py_unwrap(handle);
                let args = PY_TUPLE_NEW(0);
                let res = PY_OBJECT_CALL_OBJECT(callable, args);
                PY_DEC_REF(args);
                assert!(!res.is_null(), "reentrant call must not fault");
                PY_DEC_REF(res);
            });
            olive_py_decref(handle);
        }
    }

    #[test]
    fn called_1e5_times_refcounts_stable() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let record = make_record(add_one_thunk as *const c_void, &[]);
            let tags = make_tags(0, &[], ARG_INT);
            let handle = olive_py_make_callable(record, tags);
            assert!(!handle.is_null());

            with_gil(|| {
                let callable = olive_py_unwrap(handle);
                let baseline = *(callable as *const isize);
                for _ in 0..100_000 {
                    let args = PY_TUPLE_NEW(0);
                    let res = PY_OBJECT_CALL_OBJECT(callable, args);
                    PY_DEC_REF(args);
                    assert!(!res.is_null());
                    PY_DEC_REF(res);
                }
                let after = *(callable as *const isize);
                assert_eq!(after, baseline, "callable's own refcount must be stable");
            });
            olive_py_decref(handle);
        }
    }
}
