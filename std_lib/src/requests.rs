use crate::{OliveObj, olive_str_from_ptr, olive_str_internal};

fn url_from_ptr(ptr: i64) -> String {
    if ptr == 0 {
        return String::new();
    }
    let p = ptr & !1;
    let c_str = unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) };
    c_str.to_string_lossy().into_owned()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_get(url_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    match ureq::get(&url).call() {
        Ok(resp) => match resp.into_string() {
            Ok(body) => olive_str_internal(&body),
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_post(url_ptr: i64, body_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    let body = if body_ptr == 0 {
        String::new()
    } else {
        olive_str_from_ptr(body_ptr)
    };
    match ureq::post(&url).send_bytes(body.as_bytes()) {
        Ok(resp) => match resp.into_string() {
            Ok(s) => olive_str_internal(&s),
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_post_json(url_ptr: i64, body_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    let body = if body_ptr == 0 {
        String::new()
    } else {
        olive_str_from_ptr(body_ptr)
    };
    match ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_bytes(body.as_bytes())
    {
        Ok(resp) => match resp.into_string() {
            Ok(s) => olive_str_internal(&s),
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_put(url_ptr: i64, body_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    let body = if body_ptr == 0 {
        String::new()
    } else {
        olive_str_from_ptr(body_ptr)
    };
    match ureq::put(&url).send_bytes(body.as_bytes()) {
        Ok(resp) => match resp.into_string() {
            Ok(s) => olive_str_internal(&s),
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_delete(url_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    match ureq::delete(&url).call() {
        Ok(resp) => resp.status() as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_get_status(url_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    match ureq::get(&url).call() {
        Ok(resp) => resp.status() as i64,
        Err(ureq::Error::Status(code, _)) => code as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_http_get_with_headers(url_ptr: i64, headers_ptr: i64) -> i64 {
    if url_ptr == 0 {
        return 0;
    }
    let url = url_from_ptr(url_ptr);
    let mut req = ureq::get(&url);
    if headers_ptr != 0 {
        let obj = unsafe { &*(headers_ptr as *const OliveObj) };
        for (k, &v) in &obj.fields {
            let val = crate::olive_str_from_ptr(v);
            req = req.set(k, &val);
        }
    }
    match req.call() {
        Ok(resp) => match resp.into_string() {
            Ok(body) => olive_str_internal(&body),
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_get_returns_zero_on_bad_url() {
        let url = crate::olive_str_internal("http://localhost:19999/nonexistent_olive_test");
        assert_eq!(olive_http_get(url), 0);
    }

    #[test]
    fn http_get_null_url() {
        assert_eq!(olive_http_get(0), 0);
    }

    #[test]
    fn http_post_null_url() {
        assert_eq!(olive_http_post(0, 0), 0);
    }
}
