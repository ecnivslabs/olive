//! LSP wire framing: `Content-Length: N\r\n\r\n` prefix, other headers ignored.

use serde_json::Value;
use std::io::{self, BufRead, Write};

/// `Ok(None)` on clean EOF (client closed stdin).
pub fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut header = String::new();
        let n = reader.read_line(&mut header)?;
        if n == 0 {
            return Ok(None);
        }
        let header = header.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        if let Some(rest) = header
            .split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Length"))
            .map(|(_, v)| v)
        {
            let len = rest.trim().parse::<usize>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "malformed Content-Length header",
                )
            })?;
            content_length = Some(len);
        }
    }

    let len = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
    })?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    let value =
        serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(value))
}

/// Flushes so the client sees it immediately.
pub fn write_message<W: Write>(writer: &mut W, value: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    #[test]
    fn roundtrip_single_message() {
        let mut buf = Vec::new();
        let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"});
        write_message(&mut buf, &msg).unwrap();

        let mut cursor = Cursor::new(buf);
        let read = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(read, msg);
    }

    #[test]
    fn roundtrip_multiple_messages_back_to_back() {
        let mut buf = Vec::new();
        let a = json!({"jsonrpc": "2.0", "method": "a"});
        let b = json!({"jsonrpc": "2.0", "method": "b"});
        write_message(&mut buf, &a).unwrap();
        write_message(&mut buf, &b).unwrap();

        let mut cursor = Cursor::new(buf);
        assert_eq!(read_message(&mut cursor).unwrap().unwrap(), a);
        assert_eq!(read_message(&mut cursor).unwrap().unwrap(), b);
        assert!(read_message(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn clean_eof_returns_none() {
        let mut cursor = Cursor::new(Vec::new());
        assert!(read_message(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn header_is_case_insensitive() {
        let mut raw = Vec::new();
        let body = br#"{"jsonrpc":"2.0","method":"x"}"#;
        write!(raw, "content-length: {}\r\n\r\n", body.len()).unwrap();
        raw.extend_from_slice(body);
        let mut cursor = Cursor::new(raw);
        let read = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(read["method"], "x");
    }

    #[test]
    fn tolerates_extra_headers_like_content_type() {
        // VSCode's languageclient sends Content-Type too; must be skipped, not error.
        let mut raw = Vec::new();
        let body = br#"{"jsonrpc":"2.0","method":"x"}"#;
        write!(
            raw,
            "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .unwrap();
        raw.extend_from_slice(body);
        let mut cursor = Cursor::new(raw);
        let read = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(read["method"], "x");
    }

    #[test]
    fn missing_content_length_is_an_error() {
        let mut cursor = Cursor::new(b"\r\n".to_vec());
        assert!(read_message(&mut cursor).is_err());
    }
}
