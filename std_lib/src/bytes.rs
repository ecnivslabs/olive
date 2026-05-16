use crate::{KIND_BYTES, olive_str_from_ptr, olive_str_internal};

#[repr(C)]
pub struct OliveBytes {
    pub kind: i64,
    pub data: Vec<u8>,
}

fn new_buf(data: Vec<u8>) -> i64 {
    Box::into_raw(Box::new(OliveBytes {
        kind: KIND_BYTES,
        data,
    })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_new(cap: i64) -> i64 {
    new_buf(Vec::with_capacity(cap.max(0) as usize))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_from_str(s: i64) -> i64 {
    let text = if s == 0 {
        String::new()
    } else {
        olive_str_from_ptr(s)
    };
    new_buf(text.into_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_len(buf: i64) -> i64 {
    if buf == 0 {
        return 0;
    }
    unsafe { &*(buf as *const OliveBytes) }.data.len() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_push(buf: i64, byte: i64) {
    if buf == 0 {
        return;
    }
    let b = unsafe { &mut *(buf as *mut OliveBytes) };
    b.data.push((byte & 0xFF) as u8);
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_get(buf: i64, idx: i64) -> i64 {
    if buf == 0 {
        return -1;
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    match b.data.get(idx as usize) {
        Some(&v) => v as i64,
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_set(buf: i64, idx: i64, val: i64) {
    if buf == 0 {
        return;
    }
    let b = unsafe { &mut *(buf as *mut OliveBytes) };
    if (idx as usize) < b.data.len() {
        b.data[idx as usize] = (val & 0xFF) as u8;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_to_str(buf: i64) -> i64 {
    if buf == 0 {
        return olive_str_internal("");
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    olive_str_internal(&String::from_utf8_lossy(&b.data))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_to_hex(buf: i64) -> i64 {
    if buf == 0 {
        return olive_str_internal("");
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    olive_str_internal(&hex::encode(&b.data))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_concat(a: i64, b: i64) -> i64 {
    let da = if a == 0 {
        vec![]
    } else {
        unsafe { &*(a as *const OliveBytes) }.data.clone()
    };
    let db = if b == 0 {
        vec![]
    } else {
        unsafe { &*(b as *const OliveBytes) }.data.clone()
    };
    let mut combined = da;
    combined.extend_from_slice(&db);
    new_buf(combined)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_slice(buf: i64, start: i64, end: i64) -> i64 {
    if buf == 0 {
        return new_buf(vec![]);
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    let s = (start as usize).min(b.data.len());
    let e = (end as usize).min(b.data.len());
    if s > e {
        return new_buf(vec![]);
    }
    new_buf(b.data[s..e].to_vec())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_free(buf: i64) {
    if buf != 0 {
        unsafe { drop(Box::from_raw(buf as *mut OliveBytes)) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_read_u16_le(buf: i64, offset: i64) -> i64 {
    if buf == 0 {
        return -1;
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    let off = offset as usize;
    if off + 2 > b.data.len() {
        return -1;
    }
    i64::from(u16::from_le_bytes(b.data[off..off + 2].try_into().unwrap()))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_read_u16_be(buf: i64, offset: i64) -> i64 {
    if buf == 0 {
        return -1;
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    let off = offset as usize;
    if off + 2 > b.data.len() {
        return -1;
    }
    i64::from(u16::from_be_bytes(b.data[off..off + 2].try_into().unwrap()))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_read_u32_le(buf: i64, offset: i64) -> i64 {
    if buf == 0 {
        return -1;
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    let off = offset as usize;
    if off + 4 > b.data.len() {
        return -1;
    }
    i64::from(u32::from_le_bytes(b.data[off..off + 4].try_into().unwrap()))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_read_u32_be(buf: i64, offset: i64) -> i64 {
    if buf == 0 {
        return -1;
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    let off = offset as usize;
    if off + 4 > b.data.len() {
        return -1;
    }
    i64::from(u32::from_be_bytes(b.data[off..off + 4].try_into().unwrap()))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_read_u64_le(buf: i64, offset: i64) -> i64 {
    if buf == 0 {
        return -1;
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    let off = offset as usize;
    if off + 8 > b.data.len() {
        return -1;
    }
    u64::from_le_bytes(b.data[off..off + 8].try_into().unwrap()) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_read_u64_be(buf: i64, offset: i64) -> i64 {
    if buf == 0 {
        return -1;
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    let off = offset as usize;
    if off + 8 > b.data.len() {
        return -1;
    }
    u64::from_be_bytes(b.data[off..off + 8].try_into().unwrap()) as i64
}

fn ensure_len(b: &mut OliveBytes, needed: usize) {
    if b.data.len() < needed {
        b.data.resize(needed, 0);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_write_u16_le(buf: i64, offset: i64, val: i64) {
    if buf == 0 {
        return;
    }
    let b = unsafe { &mut *(buf as *mut OliveBytes) };
    let off = offset as usize;
    ensure_len(b, off + 2);
    b.data[off..off + 2].copy_from_slice(&(val as u16).to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_write_u16_be(buf: i64, offset: i64, val: i64) {
    if buf == 0 {
        return;
    }
    let b = unsafe { &mut *(buf as *mut OliveBytes) };
    let off = offset as usize;
    ensure_len(b, off + 2);
    b.data[off..off + 2].copy_from_slice(&(val as u16).to_be_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_write_u32_le(buf: i64, offset: i64, val: i64) {
    if buf == 0 {
        return;
    }
    let b = unsafe { &mut *(buf as *mut OliveBytes) };
    let off = offset as usize;
    ensure_len(b, off + 4);
    b.data[off..off + 4].copy_from_slice(&(val as u32).to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_write_u32_be(buf: i64, offset: i64, val: i64) {
    if buf == 0 {
        return;
    }
    let b = unsafe { &mut *(buf as *mut OliveBytes) };
    let off = offset as usize;
    ensure_len(b, off + 4);
    b.data[off..off + 4].copy_from_slice(&(val as u32).to_be_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_write_u64_le(buf: i64, offset: i64, val: i64) {
    if buf == 0 {
        return;
    }
    let b = unsafe { &mut *(buf as *mut OliveBytes) };
    let off = offset as usize;
    ensure_len(b, off + 8);
    b.data[off..off + 8].copy_from_slice(&(val as u64).to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_write_u64_be(buf: i64, offset: i64, val: i64) {
    if buf == 0 {
        return;
    }
    let b = unsafe { &mut *(buf as *mut OliveBytes) };
    let off = offset as usize;
    ensure_len(b, off + 8);
    b.data[off..off + 8].copy_from_slice(&(val as u64).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;

    fn s(text: &str) -> i64 {
        olive_str_internal(text)
    }

    #[test]
    fn buf_new_push_get() {
        let b = olive_buf_new(4);
        olive_buf_push(b, 0x41);
        olive_buf_push(b, 0x42);
        assert_eq!(olive_buf_len(b), 2);
        assert_eq!(olive_buf_get(b, 0), 0x41);
        assert_eq!(olive_buf_get(b, 1), 0x42);
        assert_eq!(olive_buf_get(b, 99), -1);
        olive_buf_free(b);
    }

    #[test]
    fn buf_from_str_to_str() {
        let b = olive_buf_from_str(s("hello"));
        assert_eq!(olive_buf_len(b), 5);
        let out = crate::olive_str_from_ptr(olive_buf_to_str(b));
        assert_eq!(out, "hello");
        olive_buf_free(b);
    }

    #[test]
    fn buf_hex() {
        let b = olive_buf_new(0);
        olive_buf_push(b, 0xDE);
        olive_buf_push(b, 0xAD);
        let h = crate::olive_str_from_ptr(olive_buf_to_hex(b));
        assert_eq!(h, "dead");
        olive_buf_free(b);
    }

    #[test]
    fn buf_slice() {
        let b = olive_buf_from_str(s("hello"));
        let sl = olive_buf_slice(b, 1, 4);
        let out = crate::olive_str_from_ptr(olive_buf_to_str(sl));
        assert_eq!(out, "ell");
        olive_buf_free(b);
        olive_buf_free(sl);
    }

    #[test]
    fn buf_concat() {
        let a = olive_buf_from_str(s("foo"));
        let b = olive_buf_from_str(s("bar"));
        let c = olive_buf_concat(a, b);
        let out = crate::olive_str_from_ptr(olive_buf_to_str(c));
        assert_eq!(out, "foobar");
        olive_buf_free(a);
        olive_buf_free(b);
        olive_buf_free(c);
    }

    #[test]
    fn buf_endian_u32_roundtrip() {
        let b = olive_buf_new(4);
        olive_buf_write_u32_le(b, 0, 0xDEADBEEF_u32 as i64);
        assert_eq!(olive_buf_read_u32_le(b, 0), 0xDEADBEEF_u32 as i64);
        olive_buf_write_u32_be(b, 0, 0x12345678);
        assert_eq!(olive_buf_read_u32_be(b, 0), 0x12345678);
        olive_buf_free(b);
    }

    #[test]
    fn buf_endian_u16_le_be() {
        let b = olive_buf_new(4);
        olive_buf_write_u16_le(b, 0, 0x0102);
        assert_eq!(olive_buf_get(b, 0), 0x02);
        assert_eq!(olive_buf_get(b, 1), 0x01);
        olive_buf_write_u16_be(b, 2, 0x0304);
        assert_eq!(olive_buf_get(b, 2), 0x03);
        assert_eq!(olive_buf_get(b, 3), 0x04);
        olive_buf_free(b);
    }

    #[test]
    fn buf_endian_u64_roundtrip() {
        let b = olive_buf_new(8);
        olive_buf_write_u64_le(b, 0, 0x0102030405060708_i64);
        assert_eq!(olive_buf_read_u64_le(b, 0), 0x0102030405060708_i64);
        olive_buf_free(b);
    }

    #[test]
    fn buf_out_of_bounds_read() {
        let b = olive_buf_new(2);
        olive_buf_push(b, 1);
        assert_eq!(olive_buf_read_u32_le(b, 0), -1);
        olive_buf_free(b);
    }

    #[test]
    fn buf_set() {
        let b = olive_buf_from_str(s("abc"));
        olive_buf_set(b, 1, 0x58);
        assert_eq!(olive_buf_get(b, 1), 0x58);
        olive_buf_free(b);
    }
}
