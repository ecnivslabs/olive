//! `file://` URI <-> path conversion; hand-rolled, no URL crate in the tree.

use std::path::{Path, PathBuf};

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 3 <= bytes.len()
            && let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Strips the Windows drive-letter leading slash on that platform.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = percent_decode(rest);
    if cfg!(windows) {
        Some(PathBuf::from(decoded.strip_prefix('/').unwrap_or(&decoded)))
    } else {
        Some(PathBuf::from(decoded))
    }
}

/// The inverse of `uri_to_path`.
pub fn path_to_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if cfg!(windows) {
        let slashed = raw.replace('\\', "/");
        let with_leading = if slashed.starts_with('/') {
            slashed
        } else {
            format!("/{slashed}")
        };
        format!("file://{}", percent_encode(&with_leading))
    } else {
        format!("file://{}", percent_encode(&raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_simple_path() {
        let uri = "file:///home/user/project/main.liv";
        let path = uri_to_path(uri).unwrap();
        assert_eq!(path, PathBuf::from("/home/user/project/main.liv"));
        assert_eq!(path_to_uri(&path), uri);
    }

    #[test]
    fn decodes_percent_encoded_space() {
        let uri = "file:///home/user/my%20project/main.liv";
        let path = uri_to_path(uri).unwrap();
        assert_eq!(path, PathBuf::from("/home/user/my project/main.liv"));
    }

    #[test]
    fn encodes_space_back_to_percent20() {
        let path = PathBuf::from("/home/user/my project/main.liv");
        let uri = path_to_uri(&path);
        assert_eq!(uri, "file:///home/user/my%20project/main.liv");
    }

    #[test]
    fn non_file_uri_returns_none() {
        assert!(uri_to_path("http://example.com/main.liv").is_none());
    }
}
