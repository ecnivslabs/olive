use crate::{olive_panic, olive_str_from_ptr, olive_str_internal};

pub(crate) const KIND_RESULT: i64 = 9;

#[repr(C)]
pub struct OliveResult {
    pub kind: i64,
    pub tag: i64,
    pub payload: i64,
}

fn make_result(ok: bool, payload: i64) -> i64 {
    let res = Box::into_raw(Box::new(OliveResult {
        kind: KIND_RESULT,
        tag: if ok { 1 } else { 0 },
        payload,
    })) as i64;
    crate::register_object(res);
    res
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_result(ptr: i64) {
    if ptr != 0 {
        crate::unregister_object(ptr);
        unsafe {
            let res = Box::from_raw(ptr as *mut OliveResult);
            if crate::is_active_object(res.payload) {
                crate::olive_free_any(res.payload);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_ok(val: i64) -> i64 {
    make_result(true, val)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_err(msg: i64) -> i64 {
    make_result(false, msg)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_is_ok(r: i64) -> i64 {
    if r == 0 {
        return 0;
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    obj.tag
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_is_err(r: i64) -> i64 {
    if r == 0 {
        return 1;
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    if obj.tag == 1 { 0 } else { 1 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_unwrap(r: i64) -> i64 {
    if r == 0 {
        olive_panic(olive_str_internal("unwrap called on null result"));
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    if obj.tag != 1 {
        let err = obj.payload;
        let msg = if err == 0 {
            olive_str_internal("unwrap called on Err result")
        } else {
            let s = olive_str_from_ptr(err);
            olive_str_internal(&format!("unwrap called on Err: {s}"))
        };
        olive_panic(msg);
    }
    obj.payload
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_unwrap_err(r: i64) -> i64 {
    if r == 0 {
        olive_panic(olive_str_internal("unwrap_err called on null result"));
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    if obj.tag == 1 {
        olive_panic(olive_str_internal("unwrap_err called on Ok result"));
    }
    obj.payload
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_unwrap_or(r: i64, default: i64) -> i64 {
    if r == 0 {
        return default;
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    if obj.tag == 1 { obj.payload } else { default }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_err_msg(r: i64) -> i64 {
    if r == 0 {
        return olive_str_internal("");
    }
    let obj = unsafe { &*(r as *const OliveResult) };
    if obj.tag == 0 { obj.payload } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;

    fn s(text: &str) -> i64 {
        olive_str_internal(text)
    }

    fn from_ptr(ptr: i64) -> String {
        crate::olive_str_from_ptr(ptr)
    }

    #[test]
    fn result_ok_is_ok() {
        let r = olive_result_ok(42);
        assert_eq!(olive_result_is_ok(r), 1);
        assert_eq!(olive_result_is_err(r), 0);
        assert_eq!(olive_result_unwrap(r), 42);
    }

    #[test]
    fn result_err_is_err() {
        let r = olive_result_err(s("something went wrong"));
        assert_eq!(olive_result_is_ok(r), 0);
        assert_eq!(olive_result_is_err(r), 1);
        let msg = from_ptr(olive_result_unwrap_err(r));
        assert_eq!(msg, "something went wrong");
    }

    #[test]
    fn result_unwrap_or() {
        let ok = olive_result_ok(99);
        let err = olive_result_err(s("fail"));
        assert_eq!(olive_result_unwrap_or(ok, 0), 99);
        assert_eq!(olive_result_unwrap_or(err, 0), 0);
        assert_eq!(olive_result_unwrap_or(0, 7), 7);
    }

    #[test]
    fn result_err_msg() {
        let r = olive_result_err(s("oops"));
        assert_eq!(from_ptr(olive_result_err_msg(r)), "oops");
    }

    #[test]
    fn result_ok_err_msg_zero() {
        let r = olive_result_ok(1);
        assert_eq!(olive_result_err_msg(r), 0);
    }
}
