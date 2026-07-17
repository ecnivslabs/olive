//! DAP wire framing and envelope helpers. Framing is byte-identical to LSP
//! (`Content-Length: N\r\n\r\n` prefix, other headers ignored); copied rather
//! than shared since `tooling::lsp::protocol` is a private module. The
//! envelope shapes differ from JSON-RPC: requests/events carry their own
//! `seq`, responses echo the request's `seq` back as `request_seq`.

use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicI64, Ordering};

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

/// Session-wide outgoing sequence counter. DAP requires `seq` to be strictly
/// increasing across every message the server sends, responses and events
/// alike, so every writer (main thread, event/output pump threads) shares
/// one of these.
pub struct Seq(AtomicI64);

impl Seq {
    pub fn new() -> Self {
        Self(AtomicI64::new(1))
    }

    pub fn next(&self) -> i64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

impl Default for Seq {
    fn default() -> Self {
        Self::new()
    }
}

pub fn response(seq: &Seq, request_seq: i64, command: &str, body: Value) -> Value {
    json!({
        "seq": seq.next(),
        "type": "response",
        "request_seq": request_seq,
        "success": true,
        "command": command,
        "body": body,
    })
}

pub fn error_response(seq: &Seq, request_seq: i64, command: &str, message: &str) -> Value {
    json!({
        "seq": seq.next(),
        "type": "response",
        "request_seq": request_seq,
        "success": false,
        "command": command,
        "message": message,
    })
}

pub fn event(seq: &Seq, name: &str, body: Value) -> Value {
    json!({
        "seq": seq.next(),
        "type": "event",
        "event": name,
        "body": body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_single_message() {
        let mut buf = Vec::new();
        let msg = json!({"seq": 1, "type": "request", "command": "initialize"});
        write_message(&mut buf, &msg).unwrap();

        let mut cursor = Cursor::new(buf);
        let read = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(read, msg);
    }

    #[test]
    fn clean_eof_returns_none() {
        let mut cursor = Cursor::new(Vec::new());
        assert!(read_message(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn seq_increases_monotonically_across_helpers() {
        let seq = Seq::new();
        let a = response(&seq, 1, "launch", json!({}));
        let b = event(&seq, "initialized", json!({}));
        let c = error_response(&seq, 2, "evaluate", "bad expression");
        assert!(a["seq"].as_i64().unwrap() < b["seq"].as_i64().unwrap());
        assert!(b["seq"].as_i64().unwrap() < c["seq"].as_i64().unwrap());
        assert_eq!(c["success"], false);
        assert_eq!(c["request_seq"], 2);
    }
}
