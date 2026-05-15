use crate::{olive_str_from_ptr, olive_str_internal};
use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use std::io::{Read, Write};

#[unsafe(no_mangle)]
pub extern "C" fn olive_gzip_compress(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let data = olive_str_from_ptr(s);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    if encoder.write_all(data.as_bytes()).is_err() {
        return 0;
    }
    match encoder.finish() {
        Ok(compressed) => olive_str_internal(&STANDARD.encode(&compressed)),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_gzip_decompress(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let encoded = olive_str_from_ptr(s);
    let compressed = match STANDARD.decode(encoded.as_bytes()) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    let mut decoder = GzDecoder::new(compressed.as_slice());
    let mut decompressed = String::new();
    match decoder.read_to_string(&mut decompressed) {
        Ok(_) => olive_str_internal(&decompressed),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_zstd_compress(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let data = olive_str_from_ptr(s);
    match zstd::encode_all(data.as_bytes(), 3) {
        Ok(compressed) => olive_str_internal(&STANDARD.encode(&compressed)),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_zstd_decompress(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let encoded = olive_str_from_ptr(s);
    let compressed = match STANDARD.decode(encoded.as_bytes()) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    match zstd::decode_all(compressed.as_slice()) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            olive_str_internal(&text)
        }
        Err(_) => 0,
    }
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
    fn gzip_roundtrip() {
        let original = "hello world, this is a test string for compression!";
        let compressed = olive_gzip_compress(s(original));
        assert_ne!(compressed, 0);
        let decompressed = from_ptr(olive_gzip_decompress(compressed));
        assert_eq!(decompressed, original);
    }

    #[test]
    fn gzip_compress_not_zero() {
        let result = olive_gzip_compress(s("test data"));
        assert_ne!(result, 0);
    }

    #[test]
    fn gzip_invalid_decompress() {
        let bad = olive_str_internal("not_base64_gzip_data!!!");
        assert_eq!(olive_gzip_decompress(bad), 0);
    }

    #[test]
    fn zstd_roundtrip() {
        let original = "zstd compression test with repeating data data data data data";
        let compressed = olive_zstd_compress(s(original));
        assert_ne!(compressed, 0);
        let decompressed = from_ptr(olive_zstd_decompress(compressed));
        assert_eq!(decompressed, original);
    }

    #[test]
    fn zstd_null_input() {
        assert_eq!(olive_zstd_compress(0), 0);
        assert_eq!(olive_zstd_decompress(0), 0);
    }

    #[test]
    fn gzip_null_input() {
        assert_eq!(olive_gzip_compress(0), 0);
        assert_eq!(olive_gzip_decompress(0), 0);
    }
}
