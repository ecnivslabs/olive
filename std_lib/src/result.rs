use crate::{OliveObj, olive_str_from_ptr, olive_str_internal, olive_panic};
use rustc_hash::FxHashMap as HashMap;

fn make_result(ok: bool, val: i64, err: i64) -> i64 {
    let mut fields = HashMap::default();
    fields.insert("ok".to_string(), if ok { 1 } else { 0 });
    if ok {
        fields.insert("val".to_string(), val);
    } else {
        fields.insert("err".to_string(), err);
    }
    Box::into_raw(Box::new(OliveObj { kind: crate::KIND_OBJ, fields })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_ok(val: i64) -> i64 {
    make_result(true, val, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_err(msg: i64) -> i64 {
    make_result(false, 0, msg)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_is_ok(r: i64) -> i64 {
    if r == 0 {
        return 0;
    }
    let obj = unsafe { &*(r as *const OliveObj) };
    *obj.fields.get("ok").unwrap_or(&0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_is_err(r: i64) -> i64 {
    if r == 0 {
        return 1;
    }
    let obj = unsafe { &*(r as *const OliveObj) };
    if *obj.fields.get("ok").unwrap_or(&0) == 1 { 0 } else { 1 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_unwrap(r: i64) -> i64 {
    if r == 0 {
        olive_panic(olive_str_internal("unwrap called on null result"));
    }
    let obj = unsafe { &*(r as *const OliveObj) };
    if *obj.fields.get("ok").unwrap_or(&0) != 1 {
        let err = *obj.fields.get("err").unwrap_or(&0);
        let msg = if err == 0 {
            olive_str_internal("unwrap called on Err result")
        } else {
            let s = olive_str_from_ptr(err);
            olive_str_internal(&format!("unwrap called on Err: {s}"))
        };
        olive_panic(msg);
    }
    *obj.fields.get("val").unwrap_or(&0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_unwrap_err(r: i64) -> i64 {
    if r == 0 {
        olive_panic(olive_str_internal("unwrap_err called on null result"));
    }
    let obj = unsafe { &*(r as *const OliveObj) };
    if *obj.fields.get("ok").unwrap_or(&0) == 1 {
        olive_panic(olive_str_internal("unwrap_err called on Ok result"));
    }
    *obj.fields.get("err").unwrap_or(&0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_unwrap_or(r: i64, default: i64) -> i64 {
    if r == 0 {
        return default;
    }
    let obj = unsafe { &*(r as *const OliveObj) };
    if *obj.fields.get("ok").unwrap_or(&0) == 1 {
        *obj.fields.get("val").unwrap_or(&default)
    } else {
        default
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_result_err_msg(r: i64) -> i64 {
    if r == 0 {
        return olive_str_internal("");
    }
    let obj = unsafe { &*(r as *const OliveObj) };
    *obj.fields.get("err").unwrap_or(&0)
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
