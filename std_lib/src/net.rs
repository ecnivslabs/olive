#[allow(unused_imports)]
use crate::olive_str_from_ptr;
use crate::olive_str_internal;
use std::io::{Read, Write};
use std::net::TcpStream;

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_tcp_connect(addr: i64) -> i64 {
    if addr == 0 {
        return 0;
    }
    let p = addr & !1;
    let c_str = unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) };
    let addr_str = c_str.to_string_lossy();

    if let Ok(stream) = TcpStream::connect(addr_str.as_ref()) {
        Box::into_raw(Box::new(stream)) as i64
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_tcp_send(stream_ptr: i64, data: i64) -> i64 {
    if stream_ptr == 0 || data == 0 {
        return -1;
    }
    let stream = unsafe { &mut *(stream_ptr as *mut TcpStream) };
    let p = data & !1;
    let c_str = unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) };

    if stream.write_all(c_str.to_bytes()).is_ok() {
        0
    } else {
        -1
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_tcp_recv(stream_ptr: i64, len: i64) -> i64 {
    if stream_ptr == 0 {
        return 0;
    }
    let stream = unsafe { &mut *(stream_ptr as *mut TcpStream) };
    let mut buf = vec![0u8; len as usize];

    if let Ok(n) = stream.read(&mut buf) {
        buf.truncate(n);
        let s = String::from_utf8_lossy(&buf);
        olive_str_internal(&s)
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_tcp_close(stream_ptr: i64) {
    if stream_ptr != 0 {
        unsafe { drop(Box::from_raw(stream_ptr as *mut TcpStream)) };
    }
}
