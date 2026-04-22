use crate::python::*;

macro_rules! impl_math_op {
    ($name:ident, $py_fn:ident, $op_name:expr) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(left: PyObject, right: PyObject) -> PyObject {
            if !is_python_available() {
                let err_str_ptr = crate::olive_str_internal(
                    "Python interop unavailable: libpython3 could not be loaded",
                );
                crate::olive_panic(err_str_ptr);
            }
            let unwrapped_l = unsafe { olive_py_unwrap(left) };
            let unwrapped_r = unsafe { olive_py_unwrap(right) };
            if unwrapped_l.is_null() || unwrapped_r.is_null() {
                let err_str_ptr = crate::olive_str_internal("Null object pointer in math op");
                crate::olive_panic(err_str_ptr);
            }
            with_gil(|| unsafe {
                let res = $py_fn(unwrapped_l, unwrapped_r);
                if res.is_null() {
                    crate::python::python_error::handle_py_error();
                }
                olive_py_wrap_owned(res)
            })
        }
    };
}

impl_math_op!(olive_py_add, PY_NUMBER_ADD, "addition");
impl_math_op!(olive_py_sub, PY_NUMBER_SUBTRACT, "subtraction");
impl_math_op!(olive_py_mul, PY_NUMBER_MULTIPLY, "multiplication");
impl_math_op!(olive_py_div, PY_NUMBER_TRUEDIVIDE, "division");
impl_math_op!(olive_py_mod, PY_NUMBER_REMAINDER, "modulo");

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_pow(left: PyObject, right: PyObject) -> i64 {
    if !is_python_available() {
        let err_str_ptr =
            crate::olive_str_internal("Python interop unavailable: libpython3 could not be loaded");
        return crate::result::olive_result_err(err_str_ptr);
    }
    let unwrapped_l = unsafe { olive_py_unwrap(left) };
    let unwrapped_r = unsafe { olive_py_unwrap(right) };
    if unwrapped_l.is_null() || unwrapped_r.is_null() {
        let err_str_ptr = crate::olive_str_internal("Null object pointer in pow op");
        return crate::result::olive_result_err(err_str_ptr);
    }
    with_gil(|| unsafe {
        let res = PY_NUMBER_POWER(unwrapped_l, unwrapped_r, _PY_NONE_STRUCT);
        if res.is_null() {
            if let Some(err_msg) = catch_py_exception_msg() {
                let err_str_ptr = crate::olive_str_internal(&err_msg);
                return crate::result::olive_result_err(err_str_ptr);
            }
            let err_str_ptr = crate::olive_str_internal("Unknown error during power");
            return crate::result::olive_result_err(err_str_ptr);
        }
        let wrapped = olive_py_wrap_owned(res);
        crate::result::olive_result_ok(wrapped as i64)
    })
}
