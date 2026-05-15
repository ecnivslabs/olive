pub fn olive_str_internal(s: &str) -> i64 {
    let c_str = std::ffi::CString::new(s).unwrap();
    c_str.into_raw() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_read(path: i64) -> i64 {
    if path == 0 {
        return 0;
    }
    let p = path & !1;
    let c_str = unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) };
    let path_str = c_str.to_string_lossy();
    if let Ok(content) = std::fs::read_to_string(path_str.as_ref()) {
        olive_str_internal(&content)
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_file_write(path: i64, data: i64) -> i64 {
    if path == 0 || data == 0 {
        return 0;
    }
    let p_path = path & !1;
    let p_data = data & !1;
    let c_path = unsafe { std::ffi::CStr::from_ptr(p_path as *const std::ffi::c_char) };
    let c_data = unsafe { std::ffi::CStr::from_ptr(p_data as *const std::ffi::c_char) };
    if std::fs::write(c_path.to_string_lossy().as_ref(), c_data.to_bytes()).is_ok() {
        1
    } else {
        0
    }
}
