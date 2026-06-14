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
    free: Vec<usize>,
    used: usize,
}

impl Chunk {
    fn new() -> Self {
        Chunk {
            data: Box::new(unsafe { std::mem::MaybeUninit::uninit().assume_init() }),
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
        unsafe {
            let slot = self.data[idx].as_mut_ptr();
            slot.write(obj);
            slot
        }
    }

    fn free_slot(&mut self, ptr: *mut OlivePyObject) {
        let base = self.data.as_ptr() as usize;
        let end = base + CHUNK_CAP * std::mem::size_of::<OlivePyObject>();
        let addr = ptr as usize;
        if addr >= base && addr < end {
            let idx = (addr - base) / std::mem::size_of::<OlivePyObject>();
            self.free.push(idx);
        }
    }

    fn contains(&self, ptr: usize) -> bool {
        let base = self.data.as_ptr() as usize;
        let end = base + CHUNK_CAP * std::mem::size_of::<OlivePyObject>();
        ptr >= base && ptr < end
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

    fn free(&mut self, ptr: *mut OlivePyObject) {
        for chunk in self.chunks.iter_mut() {
            if chunk.contains(ptr as usize) {
                chunk.free_slot(ptr);
                return;
            }
        }
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

pub unsafe fn olive_py_wrap_owned(py_ptr: PyObject) -> PyObject {
    if py_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let obj = OlivePyObject {
        kind: crate::KIND_PYOBJECT,
        py_ptr,
    };
    let raw = arena().write().unwrap().alloc(obj);
    crate::register_object(raw as i64);
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

pub fn olive_to_py(val: i64) -> PyObject {
    if val > 0x10000 && val & 1 != 0 {
        unsafe { PY_UNICODE_FROM_STRING((val & !1) as *const c_char) }
    } else {
        let ptr = val as *const c_void;
        if crate::is_active_object(val) {
            unsafe {
                let kind = *(ptr as *const i64);
                match kind {
                    crate::KIND_LIST => olive_py_create_list_proxy(val),
                    crate::KIND_OBJ => olive_py_create_dict_proxy(val),
                    crate::KIND_SET => {
                        let hs = &*(ptr as *const crate::OliveHashSet);
                        let pys = PY_SET_NEW(std::ptr::null_mut());
                        for i in 0..hs.len {
                            let v = *hs.ptr.add(i);
                            let py_v = olive_to_py(v);
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
            PY_LONG_AS_LONG(py_val).into()
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
            let seq_len = PY_OBJECT_LENGTH(py_val);
            if seq_len >= 0 {
                olive_py_to_list_internal(py_val)
            } else {
                PY_ERR_CLEAR();
                olive_py_wrap(py_val) as i64
            }
        }
    }
}

pub unsafe fn olive_py_to_list_internal(obj: PyObject) -> i64 {
    unsafe {
        let len = PY_OBJECT_LENGTH(obj) as usize;
        let list_ptr = crate::olive_list_new(len as i64);
        if len > 0 {
            let sv = &mut *(list_ptr as *mut crate::StableVec);
            let ty = raw_ob_type(obj);
            let is_list = !ty.is_null()
                && !PY_LIST_TYPE.is_null()
                && (ty == PY_LIST_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_LIST_TYPE) != 0);
            let is_tuple = !ty.is_null()
                && !PY_TUPLE_TYPE.is_null()
                && (ty == PY_TUPLE_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_TUPLE_TYPE) != 0);

            for i in 0..len {
                let py_item = if is_list {
                    let item = PY_LIST_GET_ITEM(obj, i as isize);
                    if !item.is_null() {
                        PY_INC_REF(item);
                    }
                    item
                } else if is_tuple {
                    let item = PY_TUPLE_GET_ITEM(obj, i as isize);
                    if !item.is_null() {
                        PY_INC_REF(item);
                    }
                    item
                } else {
                    let index_obj = PY_LONG_FROM_LONG(i as c_long);
                    let item = PY_OBJECT_GET_ITEM(obj, index_obj);
                    if !index_obj.is_null() {
                        PY_DEC_REF(index_obj);
                    }
                    item
                };
                *sv.ptr.add(i) = py_to_olive_internal(py_item);
                if !py_item.is_null() {
                    PY_DEC_REF(py_item);
                }
            }
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
    crate::unregister_object(obj as i64);
    if is_arena_ptr(obj as usize) {
        let raw = obj as *mut OlivePyObject;
        let py_ptr = unsafe { (*raw).py_ptr };
        if !py_ptr.is_null() {
            with_gil(|| unsafe {
                PY_DEC_REF(py_ptr);
            });
        }
        arena().write().unwrap().free(raw);
    }
}
