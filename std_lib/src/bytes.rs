use crate::slab::GenSlab;
use crate::{KIND_BYTES, olive_str_from_ptr, olive_str_internal};
use std::cell::UnsafeCell;
use std::os::raw::c_void;

thread_local! {
    static BYTES_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::new(std::mem::size_of::<OliveBytes>())) };
}

#[repr(C)]
pub struct OliveBytes {
    pub kind: i64,
    pub ptr: *mut u8,
    pub len: i64,
    pub cap: i64,
    /// Non-null: `ptr` borrows this owned `PyBytes`; buffer is read-only.
    pub py: *mut c_void,
    /// Native backing already crossed to Python once; next crossing promotes.
    pub exported: i64,
}

impl OliveBytes {
    /// Copies a Python-backed buffer into a native `Vec` and drops the
    /// backing reference. No-op for native backing. Every mutation path
    /// must run this first: the `PyBytes` payload is shared and immutable.
    pub fn realize(&mut self) {
        if self.py.is_null() {
            return;
        }
        let data = self.as_slice().to_vec();
        let py = self.py;
        self.py = std::ptr::null_mut();
        self.set_vec(data);
        crate::python::olive_py_backing_release(py);
    }

    /// Replaces the internal buffer with the given `Vec<u8>`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use olive_std::bytes::OliveBytes;
    /// let mut b = OliveBytes::empty();
    /// b.set_vec(vec![10, 20, 30]);
    /// assert_eq!(b.len, 3);
    /// ```
    pub fn set_vec(&mut self, mut v: Vec<u8>) {
        if !self.py.is_null() {
            let py = self.py;
            self.py = std::ptr::null_mut();
            crate::python::olive_py_backing_release(py);
        }
        self.ptr = v.as_mut_ptr();
        self.len = v.len() as i64;
        self.cap = v.capacity() as i64;
        std::mem::forget(v);
    }

    /// Fresh native-backed value with no buffer.
    pub const fn empty() -> Self {
        OliveBytes {
            kind: KIND_BYTES,
            ptr: std::ptr::null_mut(),
            len: 0,
            cap: 0,
            py: std::ptr::null_mut(),
            exported: 0,
        }
    }

    /// # Safety
    /// Caller must own the buffer; the fields are cleared so a later
    /// `set_vec` or drop sees a consistent state.
    ///
    /// # Examples
    ///
    /// ```
    /// # use olive_std::bytes::OliveBytes;
    /// let mut b = OliveBytes::empty();
    /// b.set_vec(vec![1, 2, 3]);
    /// unsafe {
    ///     let v = b.take_vec();
    ///     assert_eq!(v, vec![1, 2, 3]);
    ///     assert_eq!(b.len, 0);
    /// }
    /// ```
    pub unsafe fn take_vec(&mut self) -> Vec<u8> {
        self.realize();
        let v = unsafe { Vec::from_raw_parts(self.ptr, self.len as usize, self.cap as usize) };
        self.ptr = std::ptr::null_mut();
        self.len = 0;
        self.cap = 0;
        v
    }

    /// Returns a shared reference to the byte contents.
    ///
    /// # Examples
    ///
    /// ```
    /// # use olive_std::bytes::OliveBytes;
    /// let mut b = OliveBytes::empty();
    /// b.set_vec(vec![10, 20, 30]);
    /// assert_eq!(b.as_slice(), &[10, 20, 30]);
    /// ```
    pub fn as_slice(&self) -> &[u8] {
        if self.ptr.is_null() {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len as usize) }
        }
    }

    /// Returns a mutable reference to the byte contents.
    ///
    /// # Examples
    ///
    /// ```
    /// # use olive_std::bytes::OliveBytes;
    /// let mut b = OliveBytes::empty();
    /// b.set_vec(vec![1, 2, 3]);
    /// b.as_mut_slice()[1] = 42;
    /// assert_eq!(b.as_slice(), &[1, 42, 3]);
    /// ```
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.realize();
        if self.ptr.is_null() {
            &mut []
        } else {
            unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len as usize) }
        }
    }

    /// Provides mutable access to the internal `Vec<u8>` via a closure.
    ///
    /// # Examples
    ///
    /// ```
    /// # use olive_std::bytes::OliveBytes;
    /// let mut b = OliveBytes::empty();
    /// b.set_vec(vec![1, 2, 3]);
    /// let len = b.with_vec(|v| v.len());
    /// assert_eq!(len, 3);
    /// ```
    pub fn with_vec<R>(&mut self, f: impl FnOnce(&mut Vec<u8>) -> R) -> R {
        let mut v = unsafe { self.take_vec() };
        let r = f(&mut v);
        self.set_vec(v);
        r
    }

    #[inline]
    /// Appends bytes to the internal buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// # use olive_std::bytes::OliveBytes;
    /// let mut b = OliveBytes::empty();
    /// b.set_vec(vec![1, 2]);
    /// b.append(&[3, 4]);
    /// assert_eq!(b.as_slice(), &[1, 2, 3, 4]);
    /// ```
    pub fn append(&mut self, data: &[u8]) {
        self.realize();
        let n = data.len() as i64;
        if self.len + n <= self.cap {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    self.ptr.add(self.len as usize),
                    data.len(),
                );
            }
            self.len += n;
        } else {
            self.with_vec(|v| v.extend_from_slice(data));
        }
    }
}

/// Creates a new heap-allocated `OliveBytes` wrapping the given `Vec<u8>`.
///
/// # Examples
///
/// ```
/// use olive_std::bytes::new_buf;
/// let ptr = new_buf(vec![10, 20, 30]);
/// assert!(ptr != 0);
/// ```
pub fn new_buf(data: Vec<u8>) -> i64 {
    let mut b = OliveBytes::empty();
    b.set_vec(data);
    alloc_slot(b)
}

/// Wraps an owned `PyBytes` reference without copying. Caller transfers a
/// strong reference taken under the GIL; `ptr`/`len` must come from
/// `PyBytes_AS_STRING`/`PyBytes_Size` on that same object.
pub fn new_buf_py_backed(py: *mut c_void, ptr: *mut u8, len: i64) -> i64 {
    alloc_slot(OliveBytes {
        kind: KIND_BYTES,
        ptr,
        len,
        cap: 0,
        py,
        exported: 0,
    })
}

/// Deep copy for escape/copy-typed paths. A Python-backed source shares
/// the immutable payload via a fresh reference instead of copying.
pub fn clone_buf(src: i64) -> i64 {
    let b = unsafe { &*(src as *const OliveBytes) };
    if b.py.is_null() {
        new_buf(b.as_slice().to_vec())
    } else {
        crate::python::olive_py_backing_incref(b.py);
        alloc_slot(OliveBytes {
            kind: KIND_BYTES,
            ptr: b.ptr,
            len: b.len,
            cap: 0,
            py: b.py,
            exported: 0,
        })
    }
}

fn alloc_slot(b: OliveBytes) -> i64 {
    BYTES_SLAB.with(|sl| {
        let sl = unsafe { &mut *sl.get() };
        let (body, _) = sl.alloc();
        unsafe {
            std::ptr::write(body as *mut OliveBytes, b);
        }
        body as i64
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_new(cap: i64) -> i64 {
    new_buf(Vec::with_capacity(cap.max(0) as usize))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_new_zeroed(len: i64) -> i64 {
    new_buf(vec![0u8; len.max(0) as usize])
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
    unsafe { &*(buf as *const OliveBytes) }.len
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_push(buf: i64, byte: i64) {
    if buf == 0 {
        return;
    }
    let b = unsafe { &mut *(buf as *mut OliveBytes) };
    b.append(&[(byte & 0xFF) as u8]);
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_push_u16_le(buf: i64, val: i64) {
    if buf == 0 {
        return;
    }
    let b = unsafe { &mut *(buf as *mut OliveBytes) };
    b.append(&(val as u16).to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_push_u32_le(buf: i64, val: i64) {
    if buf == 0 {
        return;
    }
    let b = unsafe { &mut *(buf as *mut OliveBytes) };
    b.append(&(val as u32).to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_get(buf: i64, idx: i64) -> i64 {
    if buf == 0 {
        return -1;
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    match b.as_slice().get(idx as usize) {
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
    if idx >= 0 && idx < b.len {
        b.as_mut_slice()[idx as usize] = (val & 0xFF) as u8;
    }
}

/// Python-style repr of a byte buffer: printable ASCII inline, rest escaped.
pub(crate) fn format_bytes(buf: i64) -> String {
    if buf == 0 || !crate::slab::slot_is_live(buf) {
        return "b''".to_string();
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    let mut out = String::with_capacity(b.len as usize + 3);
    out.push_str("b'");
    for &c in b.as_slice() {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            0x20..=0x7e => out.push(c as char),
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "\\x{c:02x}");
            }
        }
    }
    out.push('\'');
    out
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_to_str(buf: i64) -> i64 {
    if buf == 0 {
        return olive_str_internal("");
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    olive_str_internal(&String::from_utf8_lossy(b.as_slice()))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_to_hex(buf: i64) -> i64 {
    if buf == 0 {
        return olive_str_internal("");
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    olive_str_internal(&hex::encode(b.as_slice()))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_concat(a: i64, b: i64) -> i64 {
    let da: &[u8] = if a == 0 {
        &[]
    } else {
        unsafe { &*(a as *const OliveBytes) }.as_slice()
    };
    let db: &[u8] = if b == 0 {
        &[]
    } else {
        unsafe { &*(b as *const OliveBytes) }.as_slice()
    };
    let mut combined = Vec::with_capacity(da.len() + db.len());
    combined.extend_from_slice(da);
    combined.extend_from_slice(db);
    new_buf(combined)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_slice(buf: i64, start: i64, end: i64) -> i64 {
    if buf == 0 {
        return new_buf(vec![]);
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    let data = b.as_slice();
    let s = (start.max(0) as usize).min(data.len());
    let e = (end.max(0) as usize).min(data.len());
    if s > e {
        return new_buf(vec![]);
    }
    new_buf(data[s..e].to_vec())
}

/// `buf[a:b:c]` with full Python slice semantics, shared with list/str via
/// `slice_bounds`. Contiguous forward slices copy in one pass.
#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_getslice(
    buf: i64,
    start: i64,
    stop: i64,
    step: i64,
    flags: i64,
) -> i64 {
    if buf == 0 {
        return new_buf(Vec::new());
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    let data = b.as_slice();
    let (start, stop, step) =
        crate::list::slice_bounds(data.len() as i64, start, stop, step, flags);
    if step == 1 {
        let s = start as usize;
        let e = stop.max(start) as usize;
        return new_buf(data[s..e].to_vec());
    }
    let mut out = Vec::new();
    let mut i = start;
    if step > 0 {
        while i < stop {
            out.push(data[i as usize]);
            i += step;
        }
    } else {
        while i > stop {
            out.push(data[i as usize]);
            i += step;
        }
    }
    new_buf(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_free(buf: i64) {
    if buf == 0 || !crate::slab::ptr_in_slab_span(buf) {
        return;
    }
    if crate::slab::slot_is_live(buf) {
        unsafe {
            let b = &mut *(buf as *mut OliveBytes);
            if b.py.is_null() {
                drop(b.take_vec());
            } else {
                let py = b.py;
                b.py = std::ptr::null_mut();
                b.ptr = std::ptr::null_mut();
                b.len = 0;
                b.cap = 0;
                crate::python::olive_py_backing_release(py);
            }
        }
    }
    BYTES_SLAB.with(|sl| {
        unsafe { &mut *sl.get() }.free(buf as *mut u8);
    });
}

fn read_bytes<const N: usize>(buf: i64, offset: i64) -> Option<[u8; N]> {
    if buf == 0 || offset < 0 {
        return None;
    }
    let b = unsafe { &*(buf as *const OliveBytes) };
    let off = offset as usize;
    b.as_slice()
        .get(off..off + N)
        .map(|s| s.try_into().unwrap())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_read_u16_le(buf: i64, offset: i64) -> i64 {
    read_bytes::<2>(buf, offset).map_or(-1, |b| i64::from(u16::from_le_bytes(b)))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_read_u16_be(buf: i64, offset: i64) -> i64 {
    read_bytes::<2>(buf, offset).map_or(-1, |b| i64::from(u16::from_be_bytes(b)))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_read_u32_le(buf: i64, offset: i64) -> i64 {
    read_bytes::<4>(buf, offset).map_or(-1, |b| i64::from(u32::from_le_bytes(b)))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_read_u32_be(buf: i64, offset: i64) -> i64 {
    read_bytes::<4>(buf, offset).map_or(-1, |b| i64::from(u32::from_be_bytes(b)))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_read_u64_le(buf: i64, offset: i64) -> i64 {
    read_bytes::<8>(buf, offset).map_or(-1, |b| u64::from_le_bytes(b) as i64)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_read_u64_be(buf: i64, offset: i64) -> i64 {
    read_bytes::<8>(buf, offset).map_or(-1, |b| u64::from_be_bytes(b) as i64)
}

fn write_bytes(buf: i64, offset: i64, data: &[u8]) {
    if buf == 0 || offset < 0 {
        return;
    }
    let b = unsafe { &mut *(buf as *mut OliveBytes) };
    let off = offset as usize;
    let end = off + data.len();
    if end as i64 <= b.len {
        b.as_mut_slice()[off..end].copy_from_slice(data);
    } else {
        b.with_vec(|v| {
            v.resize(end, 0);
            v[off..end].copy_from_slice(data);
        });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_write_u16_le(buf: i64, offset: i64, val: i64) {
    write_bytes(buf, offset, &(val as u16).to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_write_u16_be(buf: i64, offset: i64, val: i64) {
    write_bytes(buf, offset, &(val as u16).to_be_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_write_u32_le(buf: i64, offset: i64, val: i64) {
    write_bytes(buf, offset, &(val as u32).to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_write_u32_be(buf: i64, offset: i64, val: i64) {
    write_bytes(buf, offset, &(val as u32).to_be_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_write_u64_le(buf: i64, offset: i64, val: i64) {
    write_bytes(buf, offset, &(val as u64).to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_buf_write_u64_be(buf: i64, offset: i64, val: i64) {
    write_bytes(buf, offset, &(val as u64).to_be_bytes());
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
    fn buf_new_zeroed() {
        let b = olive_buf_new_zeroed(8);
        assert_eq!(olive_buf_len(b), 8);
        for i in 0..8 {
            assert_eq!(olive_buf_get(b, i), 0);
        }
        olive_buf_set(b, 3, 7);
        assert_eq!(olive_buf_get(b, 3), 7);
        olive_buf_free(b);
    }

    #[test]
    fn buf_push_u32_u16_le() {
        let b = olive_buf_new(0);
        olive_buf_push_u32_le(b, 0xDEADBEEF_u32 as i64);
        olive_buf_push_u16_le(b, 0x0102);
        assert_eq!(olive_buf_len(b), 6);
        assert_eq!(olive_buf_read_u32_le(b, 0), 0xDEADBEEF_u32 as i64);
        assert_eq!(olive_buf_get(b, 4), 0x02);
        assert_eq!(olive_buf_get(b, 5), 0x01);
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
    fn buf_getslice_full_semantics() {
        let b = olive_buf_from_str(s("abcdef"));
        let has_all = 1 | 2 | 4;
        let simple = olive_buf_getslice(b, 1, 4, 1, has_all);
        assert_eq!(crate::olive_str_from_ptr(olive_buf_to_str(simple)), "bcd");
        let neg = olive_buf_getslice(b, -2, 0, 1, 1);
        assert_eq!(crate::olive_str_from_ptr(olive_buf_to_str(neg)), "ef");
        let stepped = olive_buf_getslice(b, 0, 6, 2, has_all);
        assert_eq!(crate::olive_str_from_ptr(olive_buf_to_str(stepped)), "ace");
        let rev = olive_buf_getslice(b, 0, 0, -1, 4);
        assert_eq!(crate::olive_str_from_ptr(olive_buf_to_str(rev)), "fedcba");
        let empty = olive_buf_getslice(b, 4, 1, 1, has_all);
        assert_eq!(olive_buf_len(empty), 0);
        let clamped = olive_buf_getslice(b, 2, 99, 1, has_all);
        assert_eq!(crate::olive_str_from_ptr(olive_buf_to_str(clamped)), "cdef");
        for h in [b, simple, neg, stepped, rev, empty, clamped] {
            olive_buf_free(h);
        }
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
