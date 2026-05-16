#[allow(unused_imports)]
use crate::olive_str_from_ptr;
use crate::{OliveObj, olive_str_internal};
use rustc_hash::FxHashMap as HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_tcp_connect(addr: i64) -> i64 {
    if addr == 0 {
        return 0;
    }
    let addr_str = crate::olive_str_from_ptr(addr);
    if let Ok(stream) = TcpStream::connect(&addr_str) {
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
    let data_str = crate::olive_str_from_ptr(data);
    if stream.write_all(data_str.as_bytes()).is_ok() {
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

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_tcp_listen(addr: i64) -> i64 {
    if addr == 0 {
        return 0;
    }
    let addr_str = crate::olive_str_from_ptr(addr);
    match TcpListener::bind(&addr_str) {
        Ok(listener) => Box::into_raw(Box::new(listener)) as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_tcp_accept(listener_ptr: i64) -> i64 {
    if listener_ptr == 0 {
        return 0;
    }
    let listener = unsafe { &*(listener_ptr as *const TcpListener) };
    match listener.accept() {
        Ok((stream, _addr)) => Box::into_raw(Box::new(stream)) as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_tcp_listener_addr(listener_ptr: i64) -> i64 {
    if listener_ptr == 0 {
        return 0;
    }
    let listener = unsafe { &*(listener_ptr as *const TcpListener) };
    match listener.local_addr() {
        Ok(addr) => olive_str_internal(&addr.to_string()),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_tcp_listener_close(listener_ptr: i64) {
    if listener_ptr != 0 {
        unsafe { drop(Box::from_raw(listener_ptr as *mut TcpListener)) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_tcp_peer_addr(stream_ptr: i64) -> i64 {
    if stream_ptr == 0 {
        return 0;
    }
    let stream = unsafe { &*(stream_ptr as *const TcpStream) };
    match stream.peer_addr() {
        Ok(addr) => olive_str_internal(&addr.to_string()),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_tcp_set_timeout(stream_ptr: i64, secs: f64) -> i64 {
    if stream_ptr == 0 {
        return 0;
    }
    let stream = unsafe { &*(stream_ptr as *const TcpStream) };
    let dur = std::time::Duration::from_secs_f64(secs);
    let read_ok = stream.set_read_timeout(Some(dur)).is_ok();
    let write_ok = stream.set_write_timeout(Some(dur)).is_ok();
    if read_ok && write_ok { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_udp_open(bind_addr: i64) -> i64 {
    let addr = if bind_addr == 0 {
        "0.0.0.0:0".to_string()
    } else {
        crate::olive_str_from_ptr(bind_addr)
    };
    match UdpSocket::bind(&addr) {
        Ok(sock) => Box::into_raw(Box::new(sock)) as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_udp_send(sock_ptr: i64, addr: i64, data: i64) -> i64 {
    if sock_ptr == 0 || addr == 0 || data == 0 {
        return -1;
    }
    let sock = unsafe { &*(sock_ptr as *const UdpSocket) };
    let addr_str = crate::olive_str_from_ptr(addr);
    let data_str = crate::olive_str_from_ptr(data);
    match sock.send_to(data_str.as_bytes(), &addr_str) {
        Ok(n) => n as i64,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_udp_recv(sock_ptr: i64, max_len: i64) -> i64 {
    if sock_ptr == 0 {
        return 0;
    }
    let sock = unsafe { &*(sock_ptr as *const UdpSocket) };
    let mut buf = vec![0u8; max_len as usize];
    match sock.recv_from(&mut buf) {
        Ok((n, src_addr)) => {
            buf.truncate(n);
            let data_str = String::from_utf8_lossy(&buf).into_owned();
            let mut fields = HashMap::default();
            fields.insert("data".to_string(), olive_str_internal(&data_str));
            fields.insert(
                "addr".to_string(),
                olive_str_internal(&src_addr.to_string()),
            );
            Box::into_raw(Box::new(OliveObj {
                kind: crate::KIND_OBJ,
                fields,
            })) as i64
        }
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_udp_set_timeout(sock_ptr: i64, secs: f64) -> i64 {
    if sock_ptr == 0 {
        return 0;
    }
    let sock = unsafe { &*(sock_ptr as *const UdpSocket) };
    let dur = std::time::Duration::from_secs_f64(secs);
    if sock.set_read_timeout(Some(dur)).is_ok() {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_udp_close(sock_ptr: i64) {
    if sock_ptr != 0 {
        unsafe { drop(Box::from_raw(sock_ptr as *mut UdpSocket)) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_dns_lookup(hostname: i64) -> i64 {
    if hostname == 0 {
        return 0;
    }
    let host = crate::olive_str_from_ptr(hostname);
    let addr = format!("{}:0", host);
    match std::net::ToSocketAddrs::to_socket_addrs(&addr.as_str()) {
        Ok(mut addrs) => match addrs.next() {
            Some(addr) => olive_str_internal(&addr.ip().to_string()),
            None => 0,
        },
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_net_dns_lookup_all(hostname: i64) -> i64 {
    use crate::{KIND_LIST, StableVec};
    let empty_list = || {
        Box::into_raw(Box::new(StableVec {
            kind: KIND_LIST,
            ptr: std::ptr::null_mut(),
            cap: 0,
            len: 0,
        })) as i64
    };
    if hostname == 0 {
        return empty_list();
    }
    let host = crate::olive_str_from_ptr(hostname);
    let addr = format!("{}:0", host);
    match std::net::ToSocketAddrs::to_socket_addrs(&addr.as_str()) {
        Ok(addrs) => {
            let mut ptrs: Vec<i64> = addrs
                .map(|a| olive_str_internal(&a.ip().to_string()))
                .collect();
            let ptr = ptrs.as_mut_ptr();
            let cap = ptrs.capacity();
            let len = ptrs.len();
            std::mem::forget(ptrs);
            Box::into_raw(Box::new(StableVec {
                kind: KIND_LIST,
                ptr,
                cap,
                len,
            })) as i64
        }
        Err(_) => empty_list(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> i64 {
        crate::olive_str_internal(text)
    }

    fn from_ptr(ptr: i64) -> String {
        crate::olive_str_from_ptr(ptr)
    }

    #[test]
    fn tcp_connect_to_bad_addr_returns_zero() {
        assert_eq!(olive_net_tcp_connect(s("localhost:19999")), 0);
    }

    #[test]
    fn tcp_null_returns_zero() {
        assert_eq!(olive_net_tcp_connect(0), 0);
    }

    #[test]
    fn tcp_listen_accept_send_recv() {
        let listener = olive_net_tcp_listen(s("127.0.0.1:0"));
        assert_ne!(listener, 0);
        let addr_ptr = olive_net_tcp_listener_addr(listener);
        assert_ne!(addr_ptr, 0);
        let addr = from_ptr(addr_ptr);

        let client = olive_net_tcp_connect(s(&addr));
        assert_ne!(client, 0);

        let server_conn = olive_net_tcp_accept(listener);
        assert_ne!(server_conn, 0);

        olive_net_tcp_send(client, s("hello"));
        let received = from_ptr(olive_net_tcp_recv(server_conn, 64));
        assert_eq!(received, "hello");

        olive_net_tcp_close(client);
        olive_net_tcp_close(server_conn);
        olive_net_tcp_listener_close(listener);
    }

    #[test]
    fn udp_send_recv() {
        let server = olive_net_udp_open(s("127.0.0.1:0"));
        assert_ne!(server, 0);
        let server_sock = unsafe { &*(server as *const UdpSocket) };
        let server_addr = server_sock.local_addr().unwrap().to_string();

        let client = olive_net_udp_open(s("127.0.0.1:0"));
        assert_ne!(client, 0);

        olive_net_udp_set_timeout(server, 2.0);
        olive_net_udp_send(client, s(&server_addr), s("ping"));

        let result = olive_net_udp_recv(server, 64);
        assert_ne!(result, 0);
        let obj = unsafe { &*(result as *const OliveObj) };
        let data = from_ptr(*obj.fields.get("data").unwrap());
        assert_eq!(data, "ping");

        olive_net_udp_close(client);
        olive_net_udp_close(server);
    }

    #[test]
    fn dns_lookup_localhost() {
        let result = olive_net_dns_lookup(s("localhost"));
        if result != 0 {
            let ip = from_ptr(result);
            assert!(ip == "127.0.0.1" || ip == "::1");
        }
    }

    #[test]
    fn dns_lookup_null() {
        assert_eq!(olive_net_dns_lookup(0), 0);
    }
}
