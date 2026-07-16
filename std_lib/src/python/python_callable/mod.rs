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
    ARG_ANY_LIST, ARG_BOOL, ARG_BOOL_LIST, ARG_FLOAT, ARG_FLOAT_LIST, ARG_INT, ARG_INT_LIST,
    ARG_PYOBJECT, ARG_STR, ARG_STR_LIST, decode_scalar_arg,
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
unsafe fn decode_py_arg(obj: PyObject, tag: i64) -> i64 {
    unsafe {
        match tag {
            ARG_INT | ARG_BOOL => {
                if raw_ob_type(obj) == PY_LONG_TYPE {
                    let v = PY_LONG_AS_LONG(obj);
                    #[cfg(windows)]
                    let v = v as i64;
                    v
                } else {
                    raw_py_to_int(obj)
                }
            }
            ARG_FLOAT => raw_py_to_float(obj).to_bits() as i64,
            ARG_STR => {
                if raw_ob_type(obj) == PY_UNICODE_TYPE {
                    py_str_to_olive(obj)
                } else {
                    raw_py_to_str(obj)
                }
            }
            ARG_PYOBJECT => olive_py_wrap_borrowed(obj) as i64,
            ARG_FLOAT_LIST | ARG_INT_LIST | ARG_STR_LIST | ARG_BOOL_LIST => {
                // Python `list` → typed Olive `[T]`. `olive_py_to_list_internal`
                // with `boxed=false` stores each element as a raw word using
                // `py_to_olive_internal`, which is correct for the known scalar
                // element type (float bit pattern, int, etc.).
                crate::python::python_coerce::olive_py_to_list_internal(obj, false)
            }
            ARG_ANY_LIST => {
                // Python `list` → boxed Olive `[Any]`. Each element goes through
                // `py_to_any_internal` so scalars carry their inline tag bits.
                crate::python::python_coerce::olive_py_to_list_internal(obj, true)
            }
            _ => unreachable!("decode_py_arg: tag {tag} is not a recognised param tag"),
        }
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

        let mut args = [0i64; 4];
        let mut float_mask: u8 = 0;
        for (i, slot) in args.iter_mut().enumerate().take(arity as usize) {
            let tag = param_tag_at(tags, i as i64);
            *slot = decode_py_arg(get_arg(i), tag);
            if tag == ARG_FLOAT {
                float_mask |= 1 << i;
            }
        }

        let ret_tag = ret_tag_of(tags);
        let raw_result = dispatch::invoke_thunk(
            descriptor.thunk_ptr,
            descriptor.record_ptr,
            &args[..arity as usize],
            float_mask,
            ret_tag == ARG_FLOAT,
        );

        let py_result = decode_scalar_arg(raw_result, ret_tag);
        // The thunk's return value is an owned olive value (ordinary
        // move-return convention); `decode_scalar_arg` only ever *reads*
        // it, so the olive-side resource still needs releasing here.
        match ret_tag {
            ARG_PYOBJECT => olive_py_decref(raw_result as PyObject),
            ARG_STR => crate::string_slab::str_free(raw_result),
            ARG_FLOAT_LIST | ARG_INT_LIST | ARG_STR_LIST | ARG_BOOL_LIST | ARG_ANY_LIST => {
                crate::olive_free_list(raw_result);
            }
            _ => {}
        }
        if py_result.is_null() {
            handle_py_error();
        }
        py_result
    }
}

unsafe extern "C" fn trampoline_varargs(self_capsule: PyObject, args_tuple: PyObject) -> PyObject {
    unsafe {
        let argc = PY_TUPLE_SIZE(args_tuple).max(0) as usize;
        run_trampoline(self_capsule, argc, |i| {
            PY_TUPLE_GET_ITEM(args_tuple, i as isize)
        })
    }
}

unsafe extern "C" fn trampoline_fastcall(
    self_capsule: PyObject,
    args: *const PyObject,
    nargs: isize,
) -> PyObject {
    unsafe {
        let argc = nargs.max(0) as usize;
        run_trampoline(self_capsule, argc, |i| *args.add(i))
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
        let desc_ptr = desc_tagged & !1;
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
/// 0-15), the real arity (bits 56-59), and the return `ARG_*` tag (bits
/// 60-63, mirroring `python_ret::ret_tag_of`'s bit position). `record_ptr`
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

    unsafe extern "C" fn add_one_thunk(env: i64) -> i64 {
        env + 1
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
                let v = PY_LONG_AS_LONG(res);
                PY_DEC_REF(res);
                v
            });
            // `add_one_thunk(env) = env + 1`, called with env = the
            // record's own address: just checks the call reaches the thunk.
            assert_eq!(result as i64, record + 1);
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
                let v = PY_LONG_AS_LONG(res);
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
