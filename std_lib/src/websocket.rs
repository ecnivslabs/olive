use crate::{olive_str_from_ptr, olive_str_internal};
use std::net::TcpStream;
use tungstenite::{Message, WebSocket, connect, stream::MaybeTlsStream};

type WsConn = WebSocket<MaybeTlsStream<TcpStream>>;

#[unsafe(no_mangle)]
pub extern "C" fn olive_websocket_connect(url: i64) -> i64 {
    if url == 0 {
        return 0;
    }
    let url_str = olive_str_from_ptr(url);
    match connect(&url_str) {
        Ok((ws, _)) => Box::into_raw(Box::new(ws)) as i64,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_websocket_send(handle: i64, msg: i64) -> i64 {
    if handle == 0 || msg == 0 {
        return 0;
    }
    let ws = unsafe { &mut *(handle as *mut WsConn) };
    let text = olive_str_from_ptr(msg);
    if ws.send(Message::Text(text)).is_ok() {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_websocket_send_binary(handle: i64, buf: i64) -> i64 {
    if handle == 0 || buf == 0 {
        return 0;
    }
    let ws = unsafe { &mut *(handle as *mut WsConn) };
    let bytes_obj = unsafe { &*(buf as *const crate::bytes::OliveBytes) };
    if ws.send(Message::Binary(bytes_obj.as_slice().to_vec())).is_ok() {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_websocket_recv(handle: i64) -> i64 {
    if handle == 0 {
        return 0;
    }
    let ws = unsafe { &mut *(handle as *mut WsConn) };
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => return olive_str_internal(&text),
            Ok(Message::Binary(data)) => {
                return olive_str_internal(&String::from_utf8_lossy(&data));
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
            Ok(Message::Close(_)) | Err(_) => return 0,
            Ok(_) => return 0,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_websocket_recv_binary(handle: i64) -> i64 {
    if handle == 0 {
        return 0;
    }
    let ws = unsafe { &mut *(handle as *mut WsConn) };
    loop {
        match ws.read() {
            Ok(Message::Binary(data)) => {
                return crate::bytes::new_buf(data);
            }
            Ok(Message::Text(text)) => {
                return crate::bytes::new_buf(text.into_bytes());
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
            Ok(Message::Close(_)) | Err(_) => return 0,
            Ok(_) => return 0,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_websocket_close(handle: i64) {
    if handle != 0 {
        let ws = unsafe { &mut *(handle as *mut WsConn) };
        let _ = ws.close(None);
        unsafe { drop(Box::from_raw(handle as *mut WsConn)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_connect_bad_url_returns_zero() {
        let url = crate::olive_str_internal("ws://localhost:19998/no_such_server");
        assert_eq!(olive_websocket_connect(url), 0);
    }

    #[test]
    fn websocket_null_url_returns_zero() {
        assert_eq!(olive_websocket_connect(0), 0);
    }

    #[test]
    fn websocket_send_null_handle() {
        let msg = crate::olive_str_internal("hello");
        assert_eq!(olive_websocket_send(0, msg), 0);
    }
}
