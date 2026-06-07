//! Result-fusion tag: how a call's Python return value converts directly
//! into the Olive scalar the checker already knows the call produces,
//! instead of wrapping a handle first and paying a second boundary crossing
//! (GIL pair plus decref) to unwrap it back out.
//!
//! Packed into the top 4 bits of the same `arg_tags` word `arg_tag_at` reads
//! the per-argument encode tags from: the arity-specialized entry points
//! never use more than the low 16 bits of that word for real argument tags
//! (4 args, 4 bits each), leaving bits 60-63 free.

use crate::python::python_coerce_ffi::{raw_py_to_float, raw_py_to_int, raw_py_to_str};
use crate::python::*;

/// Keep the handle: wrap `res` exactly as every pre-R10 call did. The
/// default when the checker's result type isn't one of the scalars below.
pub(crate) const RET_HANDLE: i64 = 0;
pub(crate) const RET_INT: i64 = 1;
pub(crate) const RET_FLOAT: i64 = 2;
pub(crate) const RET_STR: i64 = 3;
pub(crate) const RET_BOOL: i64 = 4;
/// A genuinely dynamic (`Any`-typed) result: box it the same way any other
/// value entering an `Any` slot boxes, instead of realizing it later.
pub(crate) const RET_ANY: i64 = 5;
/// The result is never read (a `None`-typed stub, or a discarded
/// statement-position call): decref immediately, hand back nothing.
pub(crate) const RET_NONE: i64 = 6;

pub(crate) fn ret_tag_of(tags: i64) -> i64 {
    ((tags as u64) >> 60) as i64
}

/// Converts a call's raw, still-owned Python result into the fused
/// representation `ret_tag` names, releasing the reference this call
/// produced immediately instead of leaving a handle for the caller to
/// unwrap and decref in a second boundary crossing. Must run under the same
/// GIL region the call itself used, with `res` non-null and no Python error
/// pending (the caller's own `res.is_null()`/`PY_ERR_OCCURRED` check already
/// covers that before this runs). `ret_tag` must not be `RET_HANDLE`.
pub(crate) unsafe fn finish_ret(res: PyObject, ret_tag: i64) -> i64 {
    unsafe {
        let out = match ret_tag {
            RET_INT | RET_BOOL => raw_py_to_int(res),
            RET_FLOAT => raw_py_to_float(res).to_bits() as i64,
            RET_STR => raw_py_to_str(res),
            RET_ANY => py_to_any_internal(res),
            RET_NONE => 0,
            _ => unreachable!("finish_ret: {ret_tag} is not a fused ret_tag"),
        };
        PY_DEC_REF(res);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::python_coerce::pyobject_slab_test_lock;
    use std::ffi::CString;

    unsafe fn eval_main_fn(src: &str, name: &str) -> PyObject {
        unsafe {
            let c_src = CString::new(src).unwrap();
            PY_RUN_SIMPLE_STRING(c_src.as_ptr());
            let main_mod = PY_IMPORT_IMPORT_MODULE(b"__main__\0".as_ptr() as *const _);
            let c_name = CString::new(name).unwrap();
            let f = PY_OBJECT_GET_ATTR_STRING(main_mod, c_name.as_ptr());
            PY_DEC_REF(main_mod);
            olive_py_wrap_owned(f)
        }
    }

    /// Every fused `ret_tag` converts to the right value through `olive_py_call0`,
    /// the arity-0 shell (`arg_tags` here carries only `ret_tag`).
    #[test]
    fn arity0_fused_ret_tags_convert_correctly() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let (f_int, f_float, f_str, f_bool, f_any, f_none) = with_gil(|| {
                (
                    eval_main_fn("def __r_int():\n    return 42\n", "__r_int"),
                    eval_main_fn("def __r_float():\n    return 3.5\n", "__r_float"),
                    eval_main_fn("def __r_str():\n    return 'hi'\n", "__r_str"),
                    eval_main_fn("def __r_bool():\n    return True\n", "__r_bool"),
                    eval_main_fn("def __r_any():\n    return 99\n", "__r_any"),
                    eval_main_fn("def __r_none():\n    return None\n", "__r_none"),
                )
            });

            let ri = olive_py_call0(f_int, RET_INT << 60);
            assert_eq!(ri as i64, 42);

            let rf = olive_py_call0(f_float, RET_FLOAT << 60);
            assert_eq!(f64::from_bits(rf as u64), 3.5);

            let rs = olive_py_call0(f_str, RET_STR << 60);
            assert_eq!(crate::olive_str_from_ptr(rs as i64), "hi");

            let rb = olive_py_call0(f_bool, RET_BOOL << 60);
            assert_eq!(rb as i64, 1);

            let ra = olive_py_call0(f_any, RET_ANY << 60);
            assert_eq!(ra as i64, crate::boxed::olive_box_int(99));

            let rn = olive_py_call0(f_none, RET_NONE << 60);
            assert_eq!(rn as i64, 0);

            olive_py_decref(f_int);
            olive_py_decref(f_float);
            olive_py_decref(f_str);
            olive_py_decref(f_bool);
            olive_py_decref(f_any);
            olive_py_decref(f_none);
        }
    }

    /// `finish_ret` must decref the raw Python result exactly once per call
    /// regardless of `ret_tag`: fuse the return of `len(x)` (`RET_INT`) on a
    /// shared list argument and check that list's own refcount is unmoved
    /// after many calls -- unifies R7's arg-refcount check with a fused
    /// scalar return on the same call.
    #[test]
    fn fused_int_result_refcount_stable_across_many_calls() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let (len_fn, target_handle, target_raw) = with_gil(|| {
                let f = eval_main_fn("def __r_len(x):\n    return len(x)\n", "__r_len");
                let target = PY_LIST_NEW(0);
                let handle = olive_py_wrap_owned(target);
                (f, handle, target)
            });

            let baseline = with_gil(|| *(target_raw as *const isize));

            for _ in 0..100_000 {
                let tags1 = ARG_PYOBJECT | (RET_INT << 60);
                let r = olive_py_call1(len_fn, 0, tags1, target_handle as i64);
                assert_eq!(r as i64, 0);
            }

            let after = with_gil(|| *(target_raw as *const isize));
            assert_eq!(
                after, baseline,
                "refcount leak or over-release across repeated fused-int-return calls"
            );

            olive_py_decref(target_handle);
            olive_py_decref(len_fn);
        }
    }
}
