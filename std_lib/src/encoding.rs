use crate::{olive_str_from_ptr, olive_str_internal};
use base64::{Engine, engine::general_purpose::STANDARD};

#[unsafe(no_mangle)]
pub extern "C" fn olive_base64_encode(s: i64) -> i64 {
    if s == 0 {
        return olive_str_internal("");
    }
    let text = olive_str_from_ptr(s);
    olive_str_internal(&STANDARD.encode(text.as_bytes()))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_base64_decode(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = olive_str_from_ptr(s);
    match STANDARD.decode(text.as_bytes()) {
        Ok(bytes) => {
            let decoded = String::from_utf8_lossy(&bytes).into_owned();
            olive_str_internal(&decoded)
        }
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_base64_encode_bytes(data: i64, len: i64) -> i64 {
    if data == 0 || len <= 0 {
        return olive_str_internal("");
    }
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, len as usize) };
    olive_str_internal(&STANDARD.encode(bytes))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_url_encode(s: i64) -> i64 {
    if s == 0 {
        return olive_str_internal("");
    }
    let text = olive_str_from_ptr(s);
    olive_str_internal(&url_encode_str(&text))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_url_decode(s: i64) -> i64 {
    if s == 0 {
        return olive_str_internal("");
    }
    let text = olive_str_from_ptr(s);
    olive_str_internal(&url_decode_str(&text))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_hex_encode(s: i64) -> i64 {
    if s == 0 {
        return olive_str_internal("");
    }
    let text = olive_str_from_ptr(s);
    olive_str_internal(&hex::encode(text.as_bytes()))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_hex_decode(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = olive_str_from_ptr(s);
    match hex::decode(text.trim()) {
        Ok(bytes) => {
            let decoded = String::from_utf8_lossy(&bytes).into_owned();
            olive_str_internal(&decoded)
        }
        Err(_) => 0,
    }
}

fn url_encode_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(
                    char::from_digit((b >> 4) as u32, 16)
                        .unwrap_or('0')
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit((b & 0xf) as u32, 16)
                        .unwrap_or('0')
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}

fn url_decode_str(s: &str) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
    fn base64_roundtrip() {
        let encoded = from_ptr(olive_base64_encode(s("hello world")));
        assert_eq!(encoded, "aGVsbG8gd29ybGQ=");
        let decoded = from_ptr(olive_base64_decode(s("aGVsbG8gd29ybGQ=")));
        assert_eq!(decoded, "hello world");
    }

    #[test]
    fn base64_empty() {
        let enc = from_ptr(olive_base64_encode(s("")));
        assert_eq!(enc, "");
    }

    #[test]
    fn base64_invalid_decode() {
        assert_eq!(olive_base64_decode(s("not!!valid$$base64")), 0);
    }

    #[test]
    fn url_encode_spaces_and_special() {
        let encoded = from_ptr(olive_url_encode(s("hello world!@#")));
        assert_eq!(encoded, "hello%20world%21%40%23");
    }

    #[test]
    fn url_decode_percent_encoded() {
        let decoded = from_ptr(olive_url_decode(s("hello%20world%21")));
        assert_eq!(decoded, "hello world!");
    }

    #[test]
    fn url_decode_plus_as_space() {
        let decoded = from_ptr(olive_url_decode(s("hello+world")));
        assert_eq!(decoded, "hello world");
    }

    #[test]
    fn url_roundtrip() {
        let original = "foo bar/baz?q=1&r=2";
        let encoded = from_ptr(olive_url_encode(s(original)));
        let decoded = from_ptr(olive_url_decode(s(&encoded)));
        assert_eq!(decoded, original);
    }

    #[test]
    fn hex_roundtrip() {
        let encoded = from_ptr(olive_hex_encode(s("hello")));
        assert_eq!(encoded, "68656c6c6f");
        let decoded = from_ptr(olive_hex_decode(s("68656c6c6f")));
        assert_eq!(decoded, "hello");
    }

    #[test]
    fn hex_invalid_decode() {
        assert_eq!(olive_hex_decode(s("gg")), 0);
    }
}
