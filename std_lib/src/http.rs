#[allow(unused_imports)]
use crate::olive_str_from_ptr;
use crate::olive_str_internal;

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_get(url_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let p = url_ptr & !1;
    let c_str = unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) };
    let url = c_str.to_string_lossy();

    if let Ok(resp) = ureq::get(url.as_ref()).call() {
        if let Ok(body) = resp.into_string() {
            return olive_str_internal(&body);
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_post(url_ptr: i64, body_ptr: i64) -> i64 {
    if url_ptr == 0 || body_ptr == 0 {
        return 0;
    }
    let p_url = url_ptr & !1;
    let p_body = body_ptr & !1;
    let c_url = unsafe { std::ffi::CStr::from_ptr(p_url as *const std::ffi::c_char) };
    let c_body = unsafe { std::ffi::CStr::from_ptr(p_body as *const std::ffi::c_char) };

    if let Ok(resp) = ureq::post(c_url.to_string_lossy().as_ref()).send_bytes(c_body.to_bytes()) {
        if let Ok(res_body) = resp.into_string() {
            return olive_str_internal(&res_body);
        }
    }
    0
}
