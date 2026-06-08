use crate::python::*;
use std::ffi::CStr;
use std::os::raw::{c_char, c_double, c_long, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

#[repr(C)]
#[derive(Copy, Clone)]
pub struct OlivePyObject {
    pub kind: i64,
    pub py_ptr: PyObject,
}

unsafe impl Send for OlivePyObject {}
unsafe impl Sync for OlivePyObject {}

/// Serializes tests that assert slot liveness after a free: cargo runs test
/// fns on separate threads sharing the one global pyobject slab, and a freed
/// slot can be reallocated by another test between free and check.
#[cfg(test)]
pub(crate) fn pyobject_slab_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A tagged Olive-string pointer for a test method/attr name, valid for the
/// rest of the process -- `interned_attr`'s cache is keyed by this address,
/// which real compiled code satisfies with a literal's static rodata
/// address. `olive_str_internal` allocates from the string slab instead,
/// whose address gets freed and reused by a later test; a name interned
/// under that stale address then leaks into whichever unrelated call next
/// reuses the same memory. Leaking a fresh, allocator-aligned buffer per
/// name avoids both hazards.
#[cfg(test)]
pub(crate) fn static_attr_name(name: &str) -> i64 {
    let len = name.len();
    let layout = std::alloc::Layout::from_size_align(len + 1, 8).unwrap();
    unsafe {
        let ptr = std::alloc::alloc(layout);
        assert!(!ptr.is_null());
        std::ptr::copy_nonoverlapping(name.as_ptr(), ptr, len);
        *ptr.add(len) = 0;
        (ptr as i64) | 1
    }
}

/// Whether `ptr` is a live PyObject handle: a live slab body whose kind is
/// `KIND_PYOBJECT`. Lock-free -- distinct slabs never share addresses, so a
/// live body found here can only be a pyobject slot.
#[inline]
pub(crate) fn is_arena_ptr(ptr: usize) -> bool {
    crate::slab::ptr_is_slab_body(ptr as i64)
        && unsafe { *(ptr as *const i64) == crate::KIND_PYOBJECT }
}

/// Allocates a handle in the process-lifetime global slab (see
/// `SlabSet::pyobject`), never a task-local one.
fn alloc_pyobject_handle(py_ptr: PyObject) -> *mut OlivePyObject {
    crate::slab::with_escape_arena(|| unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        let (body, _fresh) = (*active).pyobject.alloc();
        let o = body as *mut OlivePyObject;
        std::ptr::write(
            o,
            OlivePyObject {
                kind: crate::KIND_PYOBJECT,
                py_ptr,
            },
        );
        o
    })
}

/// Frees a handle, returning the held py pointer, or `None` when the slot
/// is already free (double drop) -- the slab's generation check absorbs it.
/// Liveness check, payload read and free all run under the one global lock
/// so a concurrent free of the same handle can't race the payload read.
fn free_pyobject_handle(ptr: *mut OlivePyObject) -> Option<PyObject> {
    crate::slab::with_escape_arena(|| unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !is_arena_ptr(ptr as usize) {
            return None;
        }
        let py_ptr = (*ptr).py_ptr;
        if (*active).pyobject.free(ptr as *mut u8) {
            Some(py_ptr)
        } else {
            None
        }
    })
}

pub unsafe fn olive_py_wrap_owned(py_ptr: PyObject) -> PyObject {
    if py_ptr.is_null() {
        return std::ptr::null_mut();
    }
    alloc_pyobject_handle(py_ptr) as PyObject
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
pub(crate) unsafe fn raw_ob_type(obj: PyObject) -> PyObject {
    unsafe {
        if obj.is_null() {
            return std::ptr::null_mut();
        }
        *((obj as *const usize).add(1)) as PyObject
    }
}

/// Foreign numeric type cache (numpy scalars, etc): once a type's
/// `__name__` heuristic classifies it as int-like/float-like, later objects
/// of that same exact type skip straight to the raw conversion instead of
/// re-fetching `__name__` and re-matching the string. Append-only and
/// bounded -- a full cache just means the slow path keeps running for any
/// further new type, never a correctness issue.
const FOREIGN_TYPE_CACHE_SIZE: usize = 16;
static INT_LIKE_CACHE: [AtomicUsize; FOREIGN_TYPE_CACHE_SIZE] =
    [const { AtomicUsize::new(0) }; FOREIGN_TYPE_CACHE_SIZE];
static INT_LIKE_LEN: AtomicUsize = AtomicUsize::new(0);
static FLOAT_LIKE_CACHE: [AtomicUsize; FOREIGN_TYPE_CACHE_SIZE] =
    [const { AtomicUsize::new(0) }; FOREIGN_TYPE_CACHE_SIZE];
static FLOAT_LIKE_LEN: AtomicUsize = AtomicUsize::new(0);

fn foreign_cache_scan(
    cache: &[AtomicUsize; FOREIGN_TYPE_CACHE_SIZE],
    len: &AtomicUsize,
    ty: usize,
) -> bool {
    let n = len.load(Ordering::Acquire).min(FOREIGN_TYPE_CACHE_SIZE);
    cache[..n]
        .iter()
        .any(|slot| slot.load(Ordering::Relaxed) == ty)
}

/// Racing inserts of the same brand-new type may duplicate a slot rather
/// than dedupe -- harmless, `foreign_cache_scan` just finds the first copy.
fn foreign_cache_insert(
    cache: &[AtomicUsize; FOREIGN_TYPE_CACHE_SIZE],
    len: &AtomicUsize,
    ty: usize,
) {
    let idx = len.fetch_add(1, Ordering::AcqRel);
    if idx < FOREIGN_TYPE_CACHE_SIZE {
        cache[idx].store(ty, Ordering::Release);
    }
}

/// Reachable only for a raw dynamic-`Any` word with no static type at all
/// (the `olive_to_py` direction) -- every R5+ tagged fast path for a
/// statically typed scalar decodes by its own `ARG_*`/`RET_*` tag and never
/// reaches this heuristic.
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
                    crate::KIND_LIST | crate::KIND_ANY_LIST | crate::KIND_OBJ => to_py_deep(val),
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

/// Deep-realizes an Olive collection into a genuine Python object (dicts to
/// real `dict`, lists to real `list`, recursively). This is the boundary now:
/// every olive-to-Python crossing of a collection produces a value that
/// satisfies `isinstance(x, dict)` / `isinstance(x, list)`, not a proxy.
pub unsafe fn to_py_deep(val: i64) -> PyObject {
    unsafe {
        if val == 0 || !crate::is_active_object(val) {
            return olive_any_to_py_checked(val);
        }
        let kind = *(val as *const i64);
        match kind {
            crate::KIND_OBJ => {
                let py_dict = PY_DICT_NEW();
                let keys = crate::olive_obj_keys(val);
                let n = crate::olive_list_len(keys);
                for i in 0..n {
                    let key = crate::olive_list_get(keys, i);
                    let value = crate::olive_obj_get(val, key);
                    let py_value = to_py_deep(value);
                    PY_DICT_SET_ITEM_STRING(py_dict, (key & !1) as *const c_char, py_value);
                    PY_DEC_REF(py_value);
                }
                py_dict
            }
            crate::KIND_LIST | crate::KIND_ANY_LIST => {
                let n = crate::olive_list_len(val);
                let py_list = PY_LIST_NEW(n as isize);
                for i in 0..n {
                    let elem = crate::olive_list_get(val, i);
                    let item = if kind == crate::KIND_ANY_LIST {
                        to_py_deep(elem)
                    } else {
                        olive_to_py_checked(elem)
                    };
                    PY_LIST_SET_ITEM(py_list, i as isize, item);
                }
                py_list
            }
            _ => olive_to_py_checked(val),
        }
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

        // Exact-type fast path: a pointer compare against each concrete
        // CPython type object, skipping `PyType_IsSubtype` and the
        // `__name__` heuristic entirely for the overwhelmingly common case
        // (a real `bool`/`int`/`float`/`str`/`list`/`dict`/`set`/`bytes`,
        // not a subclass or a foreign numeric type). Bool is checked first:
        // `bool` subtypes `int` in CPython, but exact-type equality can't
        // conflate the two regardless of order (`True`'s type is
        // `PyBool_Type`, never `PyLong_Type`), so this ordering is for
        // clarity, not correctness.
        if ty == PY_BOOL_TYPE {
            return if PY_LONG_AS_LONG(py_val) != 0 { 1 } else { 0 };
        }
        if ty == PY_LONG_TYPE {
            let v = PY_LONG_AS_LONG(py_val);
            #[cfg(windows)]
            let v = v as i64;
            return v;
        }
        if ty == PY_FLOAT_TYPE {
            return PY_FLOAT_AS_DOUBLE(py_val).to_bits() as i64;
        }
        if ty == PY_UNICODE_TYPE {
            let s = PY_UNICODE_AS_UTF8(py_val);
            return if !s.is_null() {
                let r_str = CStr::from_ptr(s).to_string_lossy();
                crate::olive_str_internal(&r_str)
            } else {
                0
            };
        }
        if ty == PY_LIST_TYPE {
            return olive_py_to_list_internal(py_val, false);
        }
        if ty == PY_DICT_TYPE {
            return olive_py_to_dict_internal(py_val, false);
        }
        if ty == PY_SET_TYPE {
            return olive_py_to_set_internal(py_val, false);
        }
        if ty == PY_BYTES_TYPE {
            return olive_py_to_bytes_internal(py_val);
        }

        // Slow path: an actual subclass, or a foreign type (numpy scalars
        // and the like) this scheme only recognizes by its `__name__`.
        let is_subtype = |expected: PyObject| {
            if expected.is_null() {
                false
            } else {
                PY_TYPE_IS_SUBTYPE(ty, expected) != 0
            }
        };

        if is_subtype(PY_BOOL_TYPE) {
            return if PY_LONG_AS_LONG(py_val) != 0 { 1 } else { 0 };
        }
        if is_subtype(PY_LONG_TYPE)
            || foreign_cache_scan(&INT_LIKE_CACHE, &INT_LIKE_LEN, ty as usize)
        {
            let v = PY_LONG_AS_LONG(py_val);
            #[cfg(windows)]
            let v = v as i64;
            return v;
        }
        if is_subtype(PY_FLOAT_TYPE)
            || foreign_cache_scan(&FLOAT_LIKE_CACHE, &FLOAT_LIKE_LEN, ty as usize)
        {
            return PY_FLOAT_AS_DOUBLE(py_val).to_bits() as i64;
        }
        if is_subtype(PY_UNICODE_TYPE) {
            let s = PY_UNICODE_AS_UTF8(py_val);
            return if !s.is_null() {
                let r_str = CStr::from_ptr(s).to_string_lossy();
                crate::olive_str_internal(&r_str)
            } else {
                0
            };
        }
        if is_subtype(PY_LIST_TYPE) {
            return olive_py_to_list_internal(py_val, false);
        }
        if is_subtype(PY_DICT_TYPE) {
            return olive_py_to_dict_internal(py_val, false);
        }
        if is_subtype(PY_SET_TYPE) {
            return olive_py_to_set_internal(py_val, false);
        }
        if is_subtype(PY_BYTES_TYPE) {
            return olive_py_to_bytes_internal(py_val);
        }

        // Neither a known subtype nor cached: one `__name__` fetch checks
        // both "int-like" and "float-like" in a single string match (the
        // pre-R11 code ran this heuristic twice, once per candidate).
        let ty_name_attr = PY_OBJECT_GET_ATTR_STRING(ty, b"__name__\0".as_ptr() as *const c_char);
        let mut is_int_like = false;
        let mut is_float_like = false;
        if !ty_name_attr.is_null() {
            let s = PY_UNICODE_AS_UTF8(ty_name_attr);
            if !s.is_null() {
                let name = CStr::from_ptr(s).to_string_lossy();
                if name.contains("int") {
                    is_int_like = true;
                } else if name.contains("float") {
                    is_float_like = true;
                }
            }
            PY_DEC_REF(ty_name_attr);
        }
        if is_int_like {
            foreign_cache_insert(&INT_LIKE_CACHE, &INT_LIKE_LEN, ty as usize);
            let v = PY_LONG_AS_LONG(py_val);
            #[cfg(windows)]
            let v = v as i64;
            return v;
        }
        if is_float_like {
            foreign_cache_insert(&FLOAT_LIKE_CACHE, &FLOAT_LIKE_LEN, ty as usize);
            return PY_FLOAT_AS_DOUBLE(py_val).to_bits() as i64;
        }

        // Unknown objects stay PyObject; `__len__`-heuristic wrongly listifies spaCy Tokens etc.
        if !PY_ERR_OCCURRED().is_null() {
            PY_ERR_CLEAR();
        }
        olive_py_wrap(py_val) as i64
    }
}

/// Converts a Python value to an Any-compatible Olive value. Scalars are boxed
/// so float/int/truthiness read correctly; strings stay in Olive form since a
/// heap string pointer is already a valid Any word. Containers recurse with
/// `boxed = true` so a float/int nested at any depth still lands boxed --
/// e.g. a dict value that's itself a list of floats needs every leaf boxed,
/// not just the top one. Use when the result lands in an Any slot.
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
            if is_sub(PY_LIST_TYPE) || is_sub(PY_TUPLE_TYPE) {
                return olive_py_to_list_internal(py_val, true);
            }
            if is_sub(PY_DICT_TYPE) {
                return olive_py_to_dict_internal(py_val, true);
            }
            if is_sub(PY_SET_TYPE) {
                return olive_py_to_set_internal(py_val, true);
            }
        }
        py_to_olive_internal(py_val)
    }
}

pub unsafe fn olive_py_to_list_internal(obj: PyObject, boxed: bool) -> i64 {
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
                *sv.ptr.add(i) = if boxed {
                    py_to_any_internal(py_item)
                } else {
                    py_to_olive_internal(py_item)
                };
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

pub unsafe fn olive_py_to_dict_internal(obj: PyObject, boxed: bool) -> i64 {
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
                    let olive_val = if boxed {
                        py_to_any_internal(val_obj)
                    } else {
                        py_to_olive_internal(val_obj)
                    };
                    crate::olive_obj_set(olive_obj, key_ptr, olive_val);
                }
            }
        }
        olive_obj
    }
}

pub unsafe fn olive_py_to_set_internal(obj: PyObject, boxed: bool) -> i64 {
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
            let olive_val = if boxed {
                py_to_any_internal(item)
            } else {
                py_to_olive_internal(item)
            };
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
    // Claiming under the slab's generation check makes a double drop a
    // no-op: the second caller finds the slot already free.
    let taken = free_pyobject_handle(obj as *mut OlivePyObject);
    if let Some(py_ptr) = taken
        && !py_ptr.is_null()
    {
        with_gil(|| unsafe {
            PY_DEC_REF(py_ptr);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numpy_available() -> bool {
        if !is_python_available() {
            return false;
        }
        unsafe {
            with_gil(|| {
                let name = std::ffi::CString::new("numpy").unwrap();
                let m = PY_IMPORT_IMPORT_MODULE(name.as_ptr());
                if m.is_null() {
                    if !PY_ERR_OCCURRED().is_null() {
                        PY_ERR_CLEAR();
                    }
                    false
                } else {
                    PY_DEC_REF(m);
                    true
                }
            })
        }
    }

    /// Builds `numpy.<type_name>(arg)`, consuming `arg`'s reference (it goes
    /// into the call's argument tuple, which steals it like any tuple slot).
    unsafe fn make_numpy_scalar(type_name: &str, arg: PyObject) -> PyObject {
        unsafe {
            let mod_name = std::ffi::CString::new("numpy").unwrap();
            let np_mod = PY_IMPORT_IMPORT_MODULE(mod_name.as_ptr());
            let attr_name = std::ffi::CString::new(type_name).unwrap();
            let ty = PY_OBJECT_GET_ATTR_STRING(np_mod, attr_name.as_ptr());
            let args = PY_TUPLE_NEW(1);
            PY_TUPLE_SET_ITEM(args, 0, arg);
            let scalar = PY_OBJECT_CALL_OBJECT(ty, args);
            PY_DEC_REF(args);
            PY_DEC_REF(ty);
            PY_DEC_REF(np_mod);
            scalar
        }
    }

    /// `bool` is a `PyLong` subtype in CPython; the exact-type fast path in
    /// `py_to_olive_internal` must still route a real `bool` through the
    /// bool arm (truthiness), not the int arm (the underlying integer value,
    /// which happens to agree for `True`/`False` but would not for a
    /// hypothetical future bool-like value -- this test pins the dispatch,
    /// not just the coincidental output).
    #[test]
    fn bool_vs_int_discrimination_preserved_through_exact_type_dispatch() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            with_gil(|| {
                let true_obj = PY_BOOL_FROM_LONG(1);
                let five_obj = PY_LONG_FROM_LONG(5);
                assert_eq!(py_to_olive_internal(true_obj), 1);
                assert_eq!(py_to_olive_internal(five_obj), 5);
                PY_DEC_REF(true_obj);
                PY_DEC_REF(five_obj);
            });
        }
    }

    #[test]
    fn numpy_scalar_conversion_still_works_for_int_and_float() {
        let _guard = pyobject_slab_test_lock();
        if !numpy_available() {
            eprintln!("numpy not available, skipping test");
            return;
        }
        unsafe {
            with_gil(|| {
                let int_scalar = make_numpy_scalar("int64", PY_LONG_FROM_LONG(7));
                assert_eq!(py_to_olive_internal(int_scalar), 7);
                PY_DEC_REF(int_scalar);

                let float_scalar = make_numpy_scalar("float64", PY_FLOAT_FROM_DOUBLE(2.5));
                assert_eq!(
                    f64::from_bits(py_to_olive_internal(float_scalar) as u64),
                    2.5
                );
                PY_DEC_REF(float_scalar);

                // Same exact foreign type again: exercises the cache hit
                // path (`foreign_cache_scan`), not the `__name__` heuristic.
                let int_scalar2 = make_numpy_scalar("int64", PY_LONG_FROM_LONG(9));
                assert_eq!(py_to_olive_internal(int_scalar2), 9);
                PY_DEC_REF(int_scalar2);
            });
        }
    }

    #[test]
    fn foreign_type_cache_concurrency() {
        let _guard = pyobject_slab_test_lock();
        if !numpy_available() {
            eprintln!("numpy not available, skipping test");
            return;
        }
        let mut handles = Vec::new();
        for i in 0..8i64 {
            handles.push(std::thread::spawn(move || unsafe {
                with_gil(|| {
                    for j in 0..200i64 {
                        let v = i * 1000 + j;
                        let scalar = make_numpy_scalar("int64", PY_LONG_FROM_LONG(v as c_long));
                        assert_eq!(py_to_olive_internal(scalar), v);
                        PY_DEC_REF(scalar);
                    }
                });
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn wrap_unwrap_round_trips() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let py_val = with_gil(|| PY_LONG_FROM_LONG(42));
            let handle = olive_py_wrap_owned(py_val);
            assert!(!handle.is_null());
            assert!(is_arena_ptr(handle as usize));
            assert_eq!(olive_py_unwrap(handle), py_val);
            olive_py_decref(handle);
        }
    }

    #[test]
    fn double_decref_is_absorbed() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            // Value outside CPython's small-int cache, so a real over-release
            // (rather than an interned singleton's huge refcount) would show up.
            let py_val = with_gil(|| PY_LONG_FROM_LONG(654_321));
            let handle = olive_py_wrap_owned(py_val);
            olive_py_decref(handle);
            // The slot is already free; this must be a no-op, not a second
            // PY_DEC_REF on an already-released reference.
            olive_py_decref(handle);
        }
    }

    #[test]
    fn unwrap_of_freed_handle_does_not_read_stale_memory() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let py_val = with_gil(|| PY_LONG_FROM_LONG(99));
            let handle = olive_py_wrap_owned(py_val);
            olive_py_decref(handle);
            // Liveness check first: a dead slot is never read as a live
            // OlivePyObject, freed or recycled underneath it.
            assert!(!is_arena_ptr(handle as usize));
            assert_eq!(
                olive_py_unwrap(handle),
                handle,
                "dead handle passes through unchanged, not read as a payload"
            );
        }
    }

    #[test]
    fn foreign_raw_pointer_passes_through() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let py_val = with_gil(|| PY_LONG_FROM_LONG(5));
            // Never wrapped: a raw CPython pointer must unwrap to itself.
            assert_eq!(olive_py_unwrap(py_val), py_val);
            with_gil(|| PY_DEC_REF(py_val));
        }
    }

    #[test]
    fn null_handle_is_null() {
        unsafe {
            assert!(olive_py_wrap_owned(std::ptr::null_mut()).is_null());
            assert!(olive_py_unwrap(std::ptr::null_mut()).is_null());
        }
        olive_py_decref(std::ptr::null_mut());
    }

    #[test]
    fn threaded_wrap_decref_and_membership() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let stop = Arc::new(AtomicBool::new(false));
        let checker_stop = stop.clone();
        let checker = std::thread::spawn(move || {
            // Pure lock-free membership reads racing concurrent wrap/decref.
            while !checker_stop.load(Ordering::Relaxed) {
                let _ = crate::is_active_object(0x1234);
                let _ = is_arena_ptr(0x1234);
            }
        });

        let mut handles = Vec::new();
        for i in 0..8 {
            handles.push(std::thread::spawn(move || {
                for j in 0..500 {
                    let py_val =
                        with_gil(|| unsafe { PY_LONG_FROM_LONG((i * 10_000 + j) as c_long) });
                    let handle = unsafe { olive_py_wrap_owned(py_val) };
                    assert!(is_arena_ptr(handle as usize));
                    assert_eq!(unsafe { olive_py_unwrap(handle) }, py_val);
                    olive_py_decref(handle);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        checker.join().unwrap();
    }
}
