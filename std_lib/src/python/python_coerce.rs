use crate::python::*;
use std::ffi::CStr;
use std::os::raw::{c_char, c_double, c_long, c_void};
use std::sync::RwLock;

const CHUNK_CAP: usize = 512;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct OlivePyObject {
    pub kind: i64,
    pub py_ptr: PyObject,
}

struct Chunk {
    data: Box<[std::mem::MaybeUninit<OlivePyObject>; CHUNK_CAP]>,
    live: Box<[u64; CHUNK_CAP / 64]>,
    free: Vec<usize>,
    used: usize,
}

impl Chunk {
    fn new() -> Self {
        Chunk {
            data: Box::new(unsafe { std::mem::MaybeUninit::uninit().assume_init() }),
            live: Box::new([0; CHUNK_CAP / 64]),
            free: Vec::new(),
            used: 0,
        }
    }

    fn alloc(&mut self, obj: OlivePyObject) -> *mut OlivePyObject {
        let idx = if let Some(i) = self.free.pop() {
            i
        } else if self.used < CHUNK_CAP {
            let i = self.used;
            self.used += 1;
            i
        } else {
            return std::ptr::null_mut();
        };
        self.live[idx / 64] |= 1 << (idx % 64);
        unsafe {
            let slot = self.data[idx].as_mut_ptr();
            slot.write(obj);
            slot
        }
    }

    fn slot_index(&self, ptr: usize) -> Option<usize> {
        let base = self.data.as_ptr() as usize;
        let end = base + CHUNK_CAP * std::mem::size_of::<OlivePyObject>();
        if ptr < base
            || ptr >= end
            || !(ptr - base).is_multiple_of(std::mem::size_of::<OlivePyObject>())
        {
            return None;
        }
        Some((ptr - base) / std::mem::size_of::<OlivePyObject>())
    }

    fn free_slot(&mut self, ptr: *mut OlivePyObject) {
        if let Some(idx) = self.slot_index(ptr as usize)
            && self.live[idx / 64] & (1 << (idx % 64)) != 0
        {
            self.live[idx / 64] &= !(1 << (idx % 64));
            self.free.push(idx);
        }
    }

    fn contains(&self, ptr: usize) -> bool {
        let base = self.data.as_ptr() as usize;
        let end = base + CHUNK_CAP * std::mem::size_of::<OlivePyObject>();
        ptr >= base && ptr < end
    }

    fn slot_live(&self, ptr: usize) -> bool {
        match self.slot_index(ptr) {
            Some(idx) => self.live[idx / 64] & (1 << (idx % 64)) != 0,
            None => false,
        }
    }
}

struct Arena {
    chunks: Vec<Chunk>,
}

impl Arena {
    fn new() -> Self {
        Arena {
            chunks: vec![Chunk::new()],
        }
    }

    fn alloc(&mut self, obj: OlivePyObject) -> *mut OlivePyObject {
        for chunk in self.chunks.iter_mut() {
            let ptr = chunk.alloc(obj);
            if !ptr.is_null() {
                return ptr;
            }
        }
        let mut chunk = Chunk::new();
        let ptr = chunk.alloc(obj);
        self.chunks.push(chunk);
        ptr
    }

    /// Claims a live slot for release: clears the live bit and returns the
    /// held py pointer, or None when the slot is already free (double drop).
    fn take(&mut self, ptr: *mut OlivePyObject) -> Option<PyObject> {
        for chunk in self.chunks.iter_mut() {
            if chunk.contains(ptr as usize) {
                if !chunk.slot_live(ptr as usize) {
                    return None;
                }
                let py_ptr = unsafe { (*ptr).py_ptr };
                chunk.free_slot(ptr);
                return Some(py_ptr);
            }
        }
        None
    }

    fn contains(&self, ptr: usize) -> bool {
        for chunk in &self.chunks {
            if chunk.contains(ptr) {
                return true;
            }
        }
        false
    }
}

static ARENA: std::sync::OnceLock<RwLock<Arena>> = std::sync::OnceLock::new();

/// Serializes arena-liveness tests: cargo runs test fns on separate threads,
/// and a freed slot can be reallocated by another test between free and check.
#[cfg(test)]
pub(crate) fn arena_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

unsafe impl Send for OlivePyObject {}
unsafe impl Sync for OlivePyObject {}

fn arena() -> &'static RwLock<Arena> {
    ARENA.get_or_init(|| RwLock::new(Arena::new()))
}

#[inline]
fn is_arena_ptr(ptr: usize) -> bool {
    if let Ok(a) = arena().read() {
        a.contains(ptr)
    } else {
        false
    }
}

/// Whether `ptr` is a live arena handle. False before the arena exists,
/// so non-Python programs pay one static load.
pub(crate) fn arena_slot_live(ptr: usize) -> bool {
    let Some(lock) = ARENA.get() else {
        return false;
    };
    match lock.read() {
        Ok(a) => a.chunks.iter().any(|c| c.slot_live(ptr)),
        Err(_) => false,
    }
}

pub unsafe fn olive_py_wrap_owned(py_ptr: PyObject) -> PyObject {
    if py_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let obj = OlivePyObject {
        kind: crate::KIND_PYOBJECT,
        py_ptr,
    };
    let raw = arena().write().unwrap().alloc(obj);
    raw as PyObject
}

pub unsafe fn olive_py_wrap_borrowed(py_ptr: PyObject) -> PyObject {
    unsafe {
        if py_ptr.is_null() {
            return std::ptr::null_mut();
        }
        with_gil(|| {
            PY_INC_REF(py_ptr);
        });
        olive_py_wrap_owned(py_ptr)
    }
}

pub unsafe fn olive_py_wrap(py_ptr: PyObject) -> PyObject {
    unsafe { olive_py_wrap_borrowed(py_ptr) }
}

pub unsafe fn olive_py_unwrap(val: PyObject) -> PyObject {
    unsafe {
        if val.is_null() {
            return std::ptr::null_mut();
        }
        if is_arena_ptr(val as usize) {
            let obj = &*(val as *const OlivePyObject);
            return obj.py_ptr;
        }
        val
    }
}

#[inline]
unsafe fn raw_ob_type(obj: PyObject) -> PyObject {
    unsafe {
        if obj.is_null() {
            return std::ptr::null_mut();
        }
        *((obj as *const usize).add(1)) as PyObject
    }
}

fn looks_like_float(val: i64) -> bool {
    let f = f64::from_bits(val as u64);
    if f.is_nan() || f.is_infinite() || f.is_subnormal() {
        return false;
    }
    let abs_f = f.abs();
    abs_f > 1e-100 && abs_f < 1e100
}

/// Decodes inline Any-tagged scalars; use for container elements, not raw scalars.
pub fn olive_any_to_py(val: i64) -> PyObject {
    match val & crate::boxed::TAG_MASK {
        crate::boxed::TAG_INT => return unsafe { PY_LONG_FROM_LONG((val >> 3) as c_long) },
        crate::boxed::TAG_BOOL => return unsafe { PY_BOOL_FROM_LONG((val >> 3) as c_long) },
        crate::boxed::TAG_NULL => {
            return unsafe {
                let none = _PY_NONE_STRUCT as PyObject;
                PY_INC_REF(none);
                none
            };
        }
        _ => {}
    }
    olive_to_py(val)
}

pub fn olive_to_py(val: i64) -> PyObject {
    if val > 0x10000 && val & 1 != 0 {
        unsafe { PY_UNICODE_FROM_STRING((val & !1) as *const c_char) }
    } else {
        let ptr = val as *const c_void;
        if crate::is_active_object(val) {
            unsafe {
                let kind = *(ptr as *const i64);
                match kind {
                    crate::KIND_LIST | crate::KIND_ANY_LIST => olive_py_create_list_proxy(val),
                    crate::KIND_OBJ => olive_py_create_dict_proxy(val),
                    crate::KIND_SET => {
                        let hs = &*(ptr as *const crate::OliveHashSet);
                        let pys = PY_SET_NEW(std::ptr::null_mut());
                        for i in 0..hs.len {
                            let v = *hs.ptr.add(i);
                            let py_v = olive_any_to_py(v);
                            PY_SET_ADD(pys, py_v);
                            PY_DEC_REF(py_v);
                        }
                        pys
                    }
                    crate::KIND_BYTES => {
                        let b = &*(ptr as *const crate::bytes::OliveBytes);
                        let s = b.as_slice();
                        PY_BYTES_FROM_STRING_AND_SIZE(s.as_ptr(), s.len() as isize)
                    }
                    crate::KIND_PYOBJECT => {
                        let py_obj = &*(ptr as *const OlivePyObject);
                        PY_INC_REF(py_obj.py_ptr);
                        py_obj.py_ptr
                    }
                    // Heap-boxed `Any` scalars too wide to inline.
                    crate::KIND_INT => {
                        let b = &*(ptr as *const crate::boxed::OliveBoxed);
                        PY_LONG_FROM_LONG(b.bits as c_long)
                    }
                    crate::KIND_FLOAT => {
                        let b = &*(ptr as *const crate::boxed::OliveBoxed);
                        PY_FLOAT_FROM_DOUBLE(f64::from_bits(b.bits as u64) as c_double)
                    }
                    _ => {
                        if looks_like_float(val) {
                            let f = f64::from_bits(val as u64);
                            PY_FLOAT_FROM_DOUBLE(f as c_double)
                        } else {
                            PY_LONG_FROM_LONG(val as c_long)
                        }
                    }
                }
            }
        } else {
            unsafe {
                if looks_like_float(val) {
                    let f = f64::from_bits(val as u64);
                    PY_FLOAT_FROM_DOUBLE(f as c_double)
                } else {
                    PY_LONG_FROM_LONG(val as c_long)
                }
            }
        }
    }
}

/// Fails loudly; a pending exception here poisons the next C-API call.
pub unsafe fn olive_to_py_checked(val: i64) -> PyObject {
    let r = olive_to_py(val);
    unsafe {
        if r.is_null() || !PY_ERR_OCCURRED().is_null() {
            crate::python::python_error::handle_py_error();
        }
    }
    r
}

/// Checked variant of `olive_any_to_py`.
pub unsafe fn olive_any_to_py_checked(val: i64) -> PyObject {
    let r = olive_any_to_py(val);
    unsafe {
        if r.is_null() || !PY_ERR_OCCURRED().is_null() {
            crate::python::python_error::handle_py_error();
        }
    }
    r
}

pub unsafe fn olive_py_create_list_proxy(ptr: i64) -> PyObject {
    unsafe {
        let obj = crate::python_proxy::PY_TYPE_GENERIC_ALLOC(
            crate::python_proxy::OLIVE_LIST_PROXY_TYPE,
            0,
        );
        if !obj.is_null() {
            (*(obj as *mut crate::python_proxy::NativeProxy)).ptr = ptr;
        }
        obj
    }
}

pub unsafe fn olive_py_create_dict_proxy(ptr: i64) -> PyObject {
    unsafe {
        let obj = crate::python_proxy::PY_TYPE_GENERIC_ALLOC(
            crate::python_proxy::OLIVE_DICT_PROXY_TYPE,
            0,
        );
        if !obj.is_null() {
            (*(obj as *mut crate::python_proxy::NativeProxy)).ptr = ptr;
        }
        obj
    }
}

pub unsafe fn py_to_olive_internal(py_val: PyObject) -> i64 {
    unsafe {
        if py_val.is_null() || py_val == _PY_NONE_STRUCT {
            return 0;
        }

        let ty = raw_ob_type(py_val);
        if ty.is_null() {
            return 0;
        }

        let list_type = crate::python_proxy::OLIVE_LIST_PROXY_TYPE;
        let dict_type = crate::python_proxy::OLIVE_DICT_PROXY_TYPE;
        if (!list_type.is_null() && ty == list_type) || (!dict_type.is_null() && ty == dict_type) {
            let proxy = &*(py_val as *const crate::python_proxy::NativeProxy);
            return proxy.ptr;
        }

        let is_subtype = |expected: PyObject| {
            if expected.is_null() {
                false
            } else {
                PY_TYPE_IS_SUBTYPE(ty, expected) != 0
            }
        };

        if is_subtype(PY_BOOL_TYPE) {
            if PY_LONG_AS_LONG(py_val) != 0 { 1 } else { 0 }
        } else if is_subtype(PY_LONG_TYPE) || {
            let ty_name_attr =
                PY_OBJECT_GET_ATTR_STRING(ty, b"__name__\0".as_ptr() as *const c_char);
            let mut is_int_like = false;
            if !ty_name_attr.is_null() {
                let s = PY_UNICODE_AS_UTF8(ty_name_attr);
                if !s.is_null() {
                    let name = CStr::from_ptr(s).to_string_lossy();
                    if name.contains("int") {
                        is_int_like = true;
                    }
                }
                PY_DEC_REF(ty_name_attr);
            }
            is_int_like
        } {
            {
                let v = PY_LONG_AS_LONG(py_val);
                #[cfg(windows)]
                let v = v as i64;
                v
            }
        } else if is_subtype(PY_FLOAT_TYPE) || {
            let ty_name_attr =
                PY_OBJECT_GET_ATTR_STRING(ty, b"__name__\0".as_ptr() as *const c_char);
            let mut is_float_like = false;
            if !ty_name_attr.is_null() {
                let s = PY_UNICODE_AS_UTF8(ty_name_attr);
                if !s.is_null() {
                    let name = CStr::from_ptr(s).to_string_lossy();
                    if name.contains("float") {
                        is_float_like = true;
                    }
                }
                PY_DEC_REF(ty_name_attr);
            }
            is_float_like
        } {
            let f = PY_FLOAT_AS_DOUBLE(py_val);
            f.to_bits() as i64
        } else if is_subtype(PY_UNICODE_TYPE) {
            let s = PY_UNICODE_AS_UTF8(py_val);
            if !s.is_null() {
                let r_str = CStr::from_ptr(s).to_string_lossy();
                crate::olive_str_internal(&r_str)
            } else {
                0
            }
        } else if is_subtype(PY_LIST_TYPE) {
            olive_py_to_list_internal(py_val)
        } else if is_subtype(PY_DICT_TYPE) {
            olive_py_to_dict_internal(py_val)
        } else if is_subtype(PY_SET_TYPE) {
            olive_py_to_set_internal(py_val)
        } else if is_subtype(PY_BYTES_TYPE) {
            olive_py_to_bytes_internal(py_val)
        } else {
            // Unknown objects stay PyObject; `__len__`-heuristic wrongly listifies spaCy Tokens etc.
            if !PY_ERR_OCCURRED().is_null() {
                PY_ERR_CLEAR();
            }
            olive_py_wrap(py_val) as i64
        }
    }
}

/// Converts a Python value to an Any-compatible Olive value. Scalars are boxed
/// so float/int/truthiness read correctly; strings, lists, dicts stay in Olive
/// form. Use when the result lands in an Any slot.
pub unsafe fn py_to_any_internal(py_val: PyObject) -> i64 {
    unsafe {
        if py_val.is_null() || py_val == _PY_NONE_STRUCT {
            return crate::boxed::olive_box_null();
        }
        let ty = raw_ob_type(py_val);
        if !ty.is_null() {
            let is_sub = |expected: PyObject| {
                !expected.is_null() && (ty == expected || PY_TYPE_IS_SUBTYPE(ty, expected) != 0)
            };
            if is_sub(PY_BOOL_TYPE) {
                return crate::boxed::olive_box_bool(if PY_LONG_AS_LONG(py_val) != 0 {
                    1
                } else {
                    0
                });
            }
            if is_sub(PY_LONG_TYPE) {
                let v = PY_LONG_AS_LONG(py_val);
                #[cfg(windows)]
                let v = v as i64;
                return crate::boxed::olive_box_int(v);
            }
            if is_sub(PY_FLOAT_TYPE) {
                return crate::boxed::olive_box_float(PY_FLOAT_AS_DOUBLE(py_val));
            }
        }
        py_to_olive_internal(py_val)
    }
}

pub unsafe fn olive_py_to_list_internal(obj: PyObject) -> i64 {
    unsafe {
        let ty = raw_ob_type(obj);
        let is_list = !ty.is_null()
            && !PY_LIST_TYPE.is_null()
            && (ty == PY_LIST_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_LIST_TYPE) != 0);
        let is_tuple = !ty.is_null()
            && !PY_TUPLE_TYPE.is_null()
            && (ty == PY_TUPLE_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_TUPLE_TYPE) != 0);

        // Non-list/tuple iterables (generators, sets, spaCy Docs) go through PySequence_List.
        let mut materialized = std::ptr::null_mut();
        let source = if is_list || is_tuple {
            obj
        } else {
            materialized = PY_SEQUENCE_LIST(obj);
            if materialized.is_null() {
                PY_ERR_CLEAR();
                return crate::olive_list_new(0);
            }
            materialized
        };
        let from_real_list = is_list || !materialized.is_null();

        let len = if source == obj {
            PY_OBJECT_LENGTH(obj) as usize
        } else {
            PY_OBJECT_LENGTH(source) as usize
        };
        let list_ptr = crate::olive_list_new(len as i64);
        if len > 0 {
            let sv = &mut *(list_ptr as *mut crate::StableVec);
            for i in 0..len {
                let py_item = if from_real_list {
                    let item = PY_LIST_GET_ITEM(source, i as isize);
                    if !item.is_null() {
                        PY_INC_REF(item);
                    }
                    item
                } else {
                    let item = PY_TUPLE_GET_ITEM(source, i as isize);
                    if !item.is_null() {
                        PY_INC_REF(item);
                    }
                    item
                };
                *sv.ptr.add(i) = py_to_olive_internal(py_item);
                if !py_item.is_null() {
                    PY_DEC_REF(py_item);
                }
            }
        }
        if !materialized.is_null() {
            PY_DEC_REF(materialized);
        }
        list_ptr
    }
}

pub unsafe fn olive_py_to_dict_internal(obj: PyObject) -> i64 {
    unsafe {
        let olive_obj = crate::olive_obj_new();
        let mut pos: isize = 0;
        let mut key_obj: PyObject = std::ptr::null_mut();
        let mut val_obj: PyObject = std::ptr::null_mut();

        while PY_DICT_NEXT(obj, &mut pos, &mut key_obj, &mut val_obj) != 0 {
            if !key_obj.is_null() {
                let key_ty = raw_ob_type(key_obj);
                let is_unicode = !key_ty.is_null()
                    && !PY_UNICODE_TYPE.is_null()
                    && (key_ty == PY_UNICODE_TYPE
                        || PY_TYPE_IS_SUBTYPE(key_ty, PY_UNICODE_TYPE) != 0);

                let key_utf8 = if is_unicode {
                    PY_UNICODE_AS_UTF8(key_obj)
                } else {
                    let str_obj = PY_OBJECT_STR(key_obj);
                    if str_obj.is_null() {
                        continue;
                    }
                    let utf8 = PY_UNICODE_AS_UTF8(str_obj);
                    PY_DEC_REF(str_obj);
                    utf8
                };

                if !key_utf8.is_null() {
                    let key_str = CStr::from_ptr(key_utf8).to_string_lossy();
                    let key_ptr = crate::olive_str_internal(&key_str);
                    let olive_val = py_to_olive_internal(val_obj);
                    crate::olive_obj_set(olive_obj, key_ptr, olive_val);
                }
            }
        }
        olive_obj
    }
}

pub unsafe fn olive_py_to_set_internal(obj: PyObject) -> i64 {
    unsafe {
        let iter = PY_OBJECT_GET_ITER(obj);
        if iter.is_null() {
            PY_ERR_CLEAR();
            return crate::olive_set_new(0);
        }
        let size_hint = PY_OBJECT_LENGTH(obj).max(0) as i64;
        let set_ptr = crate::olive_set_new(size_hint);
        loop {
            let item = PY_ITER_NEXT(iter);
            if item.is_null() {
                PY_ERR_CLEAR();
                break;
            }
            let olive_val = py_to_olive_internal(item);
            crate::olive_set_add(set_ptr, olive_val);
            PY_DEC_REF(item);
        }
        PY_DEC_REF(iter);
        set_ptr
    }
}

pub unsafe fn olive_py_to_bytes_internal(obj: PyObject) -> i64 {
    unsafe {
        let size = PY_BYTES_SIZE(obj) as usize;
        let buf_ptr = PY_BYTES_AS_STRING(obj);
        let data = if size > 0 && !buf_ptr.is_null() {
            std::slice::from_raw_parts(buf_ptr as *const u8, size).to_vec()
        } else {
            Vec::new()
        };
        crate::bytes::new_buf(data)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_decref(obj: PyObject) {
    if obj.is_null() {
        return;
    }
    // Claiming under the write lock makes a double drop a no-op: the second
    // caller finds the slot already free instead of decrefing a stale pointer.
    let taken = arena().write().unwrap().take(obj as *mut OlivePyObject);
    if let Some(py_ptr) = taken
        && !py_ptr.is_null()
    {
        with_gil(|| unsafe {
            PY_DEC_REF(py_ptr);
        });
    }
}
