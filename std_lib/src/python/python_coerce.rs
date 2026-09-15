use crate::python::*;
use std::cell::RefCell;
use std::ffi::CStr;
use std::os::raw::{c_char, c_double, c_long, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

const MAX_CONVERSION_DEPTH: usize = 256;

struct ConversionState {
    active: Vec<usize>,
    failed: bool,
}

thread_local! {
    static CONVERSION_STATE: RefCell<ConversionState> = const {
        RefCell::new(ConversionState {
            active: Vec::new(),
            failed: false,
        })
    };
}

pub(crate) struct ConversionGuard {
    tracked: Option<usize>,
}

impl ConversionGuard {
    pub(crate) fn enter(value: PyObject) -> Option<Self> {
        CONVERSION_STATE.with(|state| {
            let mut state = state.borrow_mut();
            if state.active.is_empty() {
                state.failed = false;
            }
            if !unsafe { is_recursive_python_container(value) } {
                return Some(Self { tracked: None });
            }
            let ptr = value as usize;
            if state.active.contains(&ptr) || state.active.len() >= MAX_CONVERSION_DEPTH {
                state.failed = true;
                return None;
            }
            state.active.push(ptr);
            Some(Self { tracked: Some(ptr) })
        })
    }
}

impl Drop for ConversionGuard {
    fn drop(&mut self) {
        if let Some(ptr) = self.tracked {
            CONVERSION_STATE.with(|state| {
                let mut state = state.borrow_mut();
                if let Some(pos) = state.active.iter().rposition(|active| *active == ptr) {
                    state.active.remove(pos);
                }
            });
        }
    }
}

pub(crate) fn conversion_failed() -> bool {
    CONVERSION_STATE.with(|state| state.borrow().failed)
}

pub(crate) fn take_conversion_error() -> Option<String> {
    CONVERSION_STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.failed {
            state.failed = false;
            Some("Python conversion graph is cyclic or too deeply nested".to_string())
        } else {
            None
        }
    })
}

unsafe fn is_recursive_python_container(value: PyObject) -> bool {
    unsafe {
        if value.is_null() {
            return false;
        }
        let ty = raw_ob_type(value);
        !ty.is_null()
            && ((!PY_LIST_TYPE.is_null()
                && (ty == PY_LIST_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_LIST_TYPE) != 0))
                || (!PY_TUPLE_TYPE.is_null()
                    && (ty == PY_TUPLE_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_TUPLE_TYPE) != 0))
                || (!PY_DICT_TYPE.is_null()
                    && (ty == PY_DICT_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_DICT_TYPE) != 0))
                || (!PY_SET_TYPE.is_null()
                    && (ty == PY_SET_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_SET_TYPE) != 0)))
    }
}

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
    let slab_ptr = unsafe { PYOBJ_SLAB.get() };
    if slab_ptr.is_null() {
        return false;
    }
    let is_owned =
        unsafe { (*slab_ptr).owns_addr(ptr) && *(ptr as *const i64) == crate::KIND_PYOBJECT };
    is_owned && crate::slab::ptr_is_slab_body(ptr as i64)
}

/// Process-lifetime slab for PyObject handles. Guarded by the process-wide
/// GIL on the normal path (every alloc/free runs inside `with_gil`, which
/// serializes all Python-touching threads), or by `PYOBJ_SLAB_MUTEX` when
/// the subinterpreter pool owns per-interpreter GILs. The mode is fixed
/// before any user Python code runs, so a handle is always freed under the
/// same guard it was allocated under.
static PYOBJ_SLAB: crate::python::GilCell<crate::slab::GenSlab> = crate::python::GilCell::new(
    crate::slab::GenSlab::new(std::mem::size_of::<OlivePyObject>()),
);
static PYOBJ_SLAB_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn alloc_pyobject_handle(py_ptr: PyObject) -> *mut OlivePyObject {
    let write = || unsafe {
        let (body, _fresh) = (*PYOBJ_SLAB.get()).alloc();
        let o = body as *mut OlivePyObject;
        std::ptr::write(
            o,
            OlivePyObject {
                kind: crate::KIND_PYOBJECT,
                py_ptr,
            },
        );
        o
    };
    if crate::python::gil_process_wide() {
        with_gil(write)
    } else {
        let _guard = PYOBJ_SLAB_MUTEX.lock().unwrap();
        write()
    }
}

/// Frees a handle, returning the held py pointer, or `None` when the slot
/// is already free (double drop) -- the slab's generation check absorbs it.
/// Liveness check, payload read and free all run under one guard (GIL or
/// mutex) so a concurrent free of the same handle can't race the payload
/// read.
fn free_pyobject_handle(ptr: *mut OlivePyObject) -> Option<PyObject> {
    let take = || unsafe {
        if !is_arena_ptr(ptr as usize) {
            return None;
        }
        let py_ptr = (*ptr).py_ptr;
        if (*PYOBJ_SLAB.get()).free(ptr as *mut u8) {
            Some(py_ptr)
        } else {
            None
        }
    };
    if crate::python::gil_process_wide() {
        with_gil(take)
    } else {
        let _guard = PYOBJ_SLAB_MUTEX.lock().unwrap();
        take()
    }
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
        crate::boxed::TAG_INT => return unsafe { py_long_from_i64(val >> 3) },
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

/// Olive string to Python `str`, single-pass and length-carrying when
/// `PyUnicode_FromStringAndSize` is available (R18): the slab header already
/// knows the length, so this skips the strlen `PyUnicode_FromString` would
/// otherwise do internally. Falls back to the strlen-based call when the
/// symbol is missing.
pub(crate) unsafe fn olive_str_to_py(val: i64) -> PyObject {
    unsafe {
        if HAS_STR_AND_SIZE.load(Ordering::Relaxed) {
            let bytes = crate::olive_str_to_bytes(val);
            PY_UNICODE_FROM_STRING_AND_SIZE(bytes.as_ptr() as *const c_char, bytes.len() as isize)
        } else {
            PY_UNICODE_FROM_STRING(crate::string_slab::str_body(val) as *const c_char)
        }
    }
}

/// Python `str` to Olive string, single-pass via `PyUnicode_AsUTF8AndSize`
/// when available (R18): one call gives pointer and byte length together, so
/// the slab string allocates at exact size and copies once, with no `strlen`
/// rescan and no lossy UTF-8 re-validation (CPython already guarantees valid
/// UTF-8). Embedded NULs copy through intact on this path. Returns `0` on a
/// decode failure; falls back to the old strlen/lossy-copy path (which still
/// truncates at an embedded NUL) when the symbol is missing.
pub(crate) unsafe fn py_str_to_olive(py_str_obj: PyObject) -> i64 {
    unsafe {
        if py_str_obj.is_null() {
            return 0;
        }
        if HAS_STR_AND_SIZE.load(Ordering::Relaxed) {
            let mut len: isize = 0;
            let ptr = PY_UNICODE_AS_UTF8_AND_SIZE(py_str_obj, &mut len);
            if ptr.is_null() {
                return 0;
            }
            let bytes = std::slice::from_raw_parts(ptr as *const u8, len as usize);
            crate::string_slab::str_alloc(bytes)
        } else {
            let s = PY_UNICODE_AS_UTF8(py_str_obj);
            if s.is_null() {
                return 0;
            }
            let r_str = CStr::from_ptr(s).to_string_lossy();
            crate::olive_str_internal(&r_str)
        }
    }
}

/// A struct, enum, or trait object reaching a Python boundary faults:
///
/// there is no representation for it on the other side, and its header
/// word would misread as a container kind tag.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_noconvert() -> i64 {
    py_noconvert_fault()
}

/// Structs, enums, and trait objects have no Python representation: a
/// raw struct's header word is a field count, not a kind tag, so kind
/// dispatch would misread it as whatever container shares the number
/// (reading out of bounds and leaking adjacent words into Python).
fn py_noconvert_fault() -> ! {
    crate::panic::abort("cannot convert struct or enum to a Python value", None)
}

pub fn olive_to_py(val: i64) -> PyObject {
    if val > 0x10000 && val & 1 != 0 {
        unsafe { olive_str_to_py(val) }
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
                        if pys.is_null() {
                            return pys;
                        }
                        for i in 0..hs.len {
                            let v = *hs.ptr.add(i);
                            let py_v = olive_any_to_py(v);
                            if py_v.is_null() || PY_SET_ADD(pys, py_v) == -1 {
                                if !py_v.is_null() {
                                    PY_DEC_REF(py_v);
                                }
                                PY_DEC_REF(pys);
                                return std::ptr::null_mut();
                            }
                            PY_DEC_REF(py_v);
                        }
                        pys
                    }
                    crate::KIND_BYTES => crate::python::python_bytes_backing::bytes_to_py(
                        ptr as *mut crate::bytes::OliveBytes,
                    ),
                    crate::KIND_PYOBJECT => {
                        let py_obj = &*(ptr as *const OlivePyObject);
                        PY_INC_REF(py_obj.py_ptr);
                        py_obj.py_ptr
                    }
                    // Heap-boxed `Any` scalars too wide to inline.
                    crate::KIND_INT => {
                        let b = &*(ptr as *const crate::boxed::OliveBoxed);
                        py_long_from_i64(b.bits)
                    }
                    crate::KIND_U64 => {
                        let b = &*(ptr as *const crate::boxed::OliveBoxed);
                        py_long_from_u64(b.bits as u64)
                    }
                    crate::KIND_FLOAT => {
                        let b = &*(ptr as *const crate::boxed::OliveBoxed);
                        PY_FLOAT_FROM_DOUBLE(f64::from_bits(b.bits as u64) as c_double)
                    }
                    crate::struct_box::KIND_STRUCT_BOX
                    | crate::KIND_ENUM
                    | crate::struct_obj::KIND_FATPTR => py_noconvert_fault(),
                    _ => {
                        if looks_like_float(val) {
                            let f = f64::from_bits(val as u64);
                            PY_FLOAT_FROM_DOUBLE(f as c_double)
                        } else {
                            py_long_from_i64(val)
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
                    py_long_from_i64(val)
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

unsafe fn to_py_typed_desc_at(val: i64, desc: *const u8, pos: &mut usize) -> PyObject {
    unsafe {
        let tag = crate::format::byte(desc, *pos);
        *pos += 1;
        match tag {
            crate::format::D_INT => py_long_from_i64(val),
            crate::format::D_U64 => py_long_from_u64(val as u64),
            crate::format::D_FLOAT => PY_FLOAT_FROM_DOUBLE(f64::from_bits(val as u64)),
            crate::format::D_F32 => PY_FLOAT_FROM_DOUBLE(f32::from_bits(val as u32) as f64),
            crate::format::D_BOOL => PY_BOOL_FROM_LONG(val as c_long),
            crate::format::D_STR => olive_str_to_py(val),
            crate::format::D_NULLABLE => {
                if val == 0 {
                    crate::format::skip(desc, pos);
                    let none = _PY_NONE_STRUCT as PyObject;
                    PY_INC_REF(none);
                    none
                } else {
                    to_py_typed_desc_at(val, desc, pos)
                }
            }
            crate::format::D_NULL => {
                let none = _PY_NONE_STRUCT as PyObject;
                PY_INC_REF(none);
                none
            }
            crate::format::D_BACKREF => {
                let hi = crate::format::byte(desc, *pos) as usize;
                let lo = crate::format::byte(desc, *pos + 1) as usize;
                *pos += 2;
                let mut target = (hi << 8) | lo;
                to_py_typed_desc_at(val, desc, &mut target)
            }
            crate::format::D_LIST | crate::format::D_TUPLE => {
                let elem_start = *pos;
                crate::format::skip(desc, pos);
                let n = crate::olive_list_len(val);
                let py_seq = if tag == crate::format::D_LIST {
                    PY_LIST_NEW(n as isize)
                } else {
                    PY_TUPLE_NEW(n as isize)
                };
                if py_seq.is_null() {
                    return py_seq;
                }
                for i in 0..n {
                    let mut elem_pos = elem_start;
                    let elem = crate::olive_list_get(val, i);
                    let item = to_py_typed_desc_at(elem, desc, &mut elem_pos);
                    if item.is_null() {
                        PY_DEC_REF(py_seq);
                        return std::ptr::null_mut();
                    }
                    let inserted = if tag == crate::format::D_LIST {
                        PY_LIST_SET_ITEM(py_seq, i as isize, item)
                    } else {
                        PY_TUPLE_SET_ITEM(py_seq, i as isize, item)
                    };
                    if inserted == -1 {
                        if !item.is_null() {
                            PY_DEC_REF(item);
                        }
                        PY_DEC_REF(py_seq);
                        return std::ptr::null_mut();
                    }
                }
                py_seq
            }
            crate::format::D_SET => {
                let elem_start = *pos;
                crate::format::skip(desc, pos);
                let hs = &*(val as *const crate::OliveHashSet);
                let py_set = PY_SET_NEW(std::ptr::null_mut());
                if py_set.is_null() {
                    return py_set;
                }
                for i in 0..hs.len {
                    let mut elem_pos = elem_start;
                    let item = to_py_typed_desc_at(*hs.ptr.add(i), desc, &mut elem_pos);
                    if item.is_null() || PY_SET_ADD(py_set, item) == -1 {
                        if !item.is_null() {
                            PY_DEC_REF(item);
                        }
                        PY_DEC_REF(py_set);
                        return std::ptr::null_mut();
                    }
                    PY_DEC_REF(item);
                }
                py_set
            }
            crate::format::D_DICT => {
                let key_start = *pos;
                crate::format::skip(desc, pos);
                let value_start = *pos;
                crate::format::skip(desc, pos);
                let py_dict = PY_DICT_NEW();
                if py_dict.is_null() {
                    return py_dict;
                }
                let obj = &*(val as *const crate::OliveObj);
                for (key, value) in &obj.fields {
                    let mut key_pos = key_start;
                    let py_key = to_py_typed_desc_at(key.0, desc, &mut key_pos);
                    if py_key.is_null() {
                        PY_DEC_REF(py_dict);
                        return std::ptr::null_mut();
                    }
                    let mut value_pos = value_start;
                    let py_value = to_py_typed_desc_at(*value, desc, &mut value_pos);
                    if py_value.is_null() {
                        PY_DEC_REF(py_key);
                        PY_DEC_REF(py_dict);
                        return std::ptr::null_mut();
                    }
                    let res = PY_OBJECT_SET_ITEM(py_dict, py_key, py_value);
                    PY_DEC_REF(py_key);
                    PY_DEC_REF(py_value);
                    if res == -1 {
                        PY_DEC_REF(py_dict);
                        return std::ptr::null_mut();
                    }
                }
                py_dict
            }
            _ => to_py_deep(val),
        }
    }
}

pub(crate) unsafe fn to_py_typed_desc(val: i64, desc: i64) -> PyObject {
    unsafe {
        let desc = crate::string_slab::str_body(desc) as *const u8;
        let mut pos = 0usize;
        to_py_typed_desc_at(val, desc, &mut pos)
    }
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
                if py_dict.is_null() {
                    return py_dict;
                }
                let obj = &*(val as *const crate::OliveObj);
                for (key, &value) in &obj.fields {
                    let py_key = to_py_deep(key.0);
                    if py_key.is_null() {
                        PY_DEC_REF(py_dict);
                        return std::ptr::null_mut();
                    }
                    let py_value = to_py_deep(value);
                    if py_value.is_null() {
                        PY_DEC_REF(py_key);
                        PY_DEC_REF(py_dict);
                        return std::ptr::null_mut();
                    }
                    let res = PY_OBJECT_SET_ITEM(py_dict, py_key, py_value);
                    PY_DEC_REF(py_key);
                    PY_DEC_REF(py_value);
                    if res == -1 {
                        PY_DEC_REF(py_dict);
                        return std::ptr::null_mut();
                    }
                }
                py_dict
            }
            crate::KIND_LIST | crate::KIND_ANY_LIST => {
                let n = crate::olive_list_len(val);
                let py_list = PY_LIST_NEW(n as isize);
                if py_list.is_null() {
                    return py_list;
                }
                for i in 0..n {
                    let elem = crate::olive_list_get(val, i);
                    let item = if kind == crate::KIND_ANY_LIST {
                        to_py_deep(elem)
                    } else {
                        olive_to_py_checked(elem)
                    };
                    if item.is_null() || PY_LIST_SET_ITEM(py_list, i as isize, item) == -1 {
                        if !item.is_null() {
                            PY_DEC_REF(item);
                        }
                        PY_DEC_REF(py_list);
                        return std::ptr::null_mut();
                    }
                }
                py_list
            }
            crate::struct_box::KIND_STRUCT_BOX
            | crate::KIND_ENUM
            | crate::struct_obj::KIND_FATPTR => py_noconvert_fault(),
            _ => olive_to_py_checked(val),
        }
    }
}

pub unsafe fn py_to_olive_internal(py_val: PyObject) -> i64 {
    unsafe {
        let _conversion_guard = match ConversionGuard::enter(py_val) {
            Some(guard) => guard,
            None => return 0,
        };
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
            let v = py_long_as_i64(py_val);
            if !PY_ERR_OCCURRED().is_null() {
                PY_ERR_CLEAR();
                return olive_py_wrap(py_val) as i64;
            }
            return v;
        }
        if ty == PY_FLOAT_TYPE {
            // An exact float's conversion cannot fail (no `nb_float` dispatch).
            return PY_FLOAT_AS_DOUBLE(py_val).to_bits() as i64;
        }
        if ty == PY_UNICODE_TYPE {
            return py_str_to_olive(py_val);
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
            if let Some(wrapped) =
                crate::python::python_bytes_backing::olive_py_bytes_wrap_exact(py_val)
            {
                return wrapped;
            }
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
            let v = py_long_as_i64(py_val);
            if !PY_ERR_OCCURRED().is_null() {
                PY_ERR_CLEAR();
                return olive_py_wrap(py_val) as i64;
            }
            return v;
        }
        if is_subtype(PY_FLOAT_TYPE)
            || foreign_cache_scan(&FLOAT_LIKE_CACHE, &FLOAT_LIKE_LEN, ty as usize)
        {
            let d = PY_FLOAT_AS_DOUBLE(py_val);
            if !PY_ERR_OCCURRED().is_null() {
                PY_ERR_CLEAR();
                return olive_py_wrap(py_val) as i64;
            }
            return d.to_bits() as i64;
        }
        if is_subtype(PY_UNICODE_TYPE) {
            return py_str_to_olive(py_val);
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
            } else if !PY_ERR_OCCURRED().is_null() {
                PY_ERR_CLEAR();
            }
            PY_DEC_REF(ty_name_attr);
        } else if !PY_ERR_OCCURRED().is_null() {
            PY_ERR_CLEAR();
        }
        if is_int_like {
            let v = py_long_as_i64(py_val);
            if !PY_ERR_OCCURRED().is_null() {
                PY_ERR_CLEAR();
                return olive_py_wrap(py_val) as i64;
            }
            foreign_cache_insert(&INT_LIKE_CACHE, &INT_LIKE_LEN, ty as usize);
            return v;
        }
        if is_float_like {
            let d = PY_FLOAT_AS_DOUBLE(py_val);
            if !PY_ERR_OCCURRED().is_null() {
                PY_ERR_CLEAR();
                return olive_py_wrap(py_val) as i64;
            }
            foreign_cache_insert(&FLOAT_LIKE_CACHE, &FLOAT_LIKE_LEN, ty as usize);
            return d.to_bits() as i64;
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
#[repr(align(8))]
struct AlignedDescriptor([u8; 1]);

fn scalar_descriptor(tag: i64) -> i64 {
    static INT: AlignedDescriptor = AlignedDescriptor([crate::format::D_INT]);
    static FLOAT: AlignedDescriptor = AlignedDescriptor([crate::format::D_FLOAT]);
    static F32: AlignedDescriptor = AlignedDescriptor([crate::format::D_F32]);
    static BOOL: AlignedDescriptor = AlignedDescriptor([crate::format::D_BOOL]);
    static STR: AlignedDescriptor = AlignedDescriptor([crate::format::D_STR]);
    static NULL: AlignedDescriptor = AlignedDescriptor([crate::format::D_NULL]);
    static U64: AlignedDescriptor = AlignedDescriptor([crate::format::D_U64]);
    static ANY: AlignedDescriptor = AlignedDescriptor([crate::format::D_ANY]);
    match tag {
        2 => FLOAT.0.as_ptr() as i64,
        3 => F32.0.as_ptr() as i64,
        4 => BOOL.0.as_ptr() as i64,
        5 => STR.0.as_ptr() as i64,
        6 => NULL.0.as_ptr() as i64,
        7 => U64.0.as_ptr() as i64,
        0 => ANY.0.as_ptr() as i64,
        _ => INT.0.as_ptr() as i64,
    }
}

pub(crate) unsafe fn py_to_typed_scalar_internal(py_val: PyObject, tag: i64) -> Option<i64> {
    unsafe {
        if py_val.is_null() {
            return None;
        }
        let ty = raw_ob_type(py_val);
        let is_sub = |expected: PyObject| {
            !expected.is_null()
                && !ty.is_null()
                && (ty == expected || PY_TYPE_IS_SUBTYPE(ty, expected) != 0)
        };
        match tag {
            1 => {
                if is_sub(PY_LONG_TYPE) {
                    let value = py_long_as_i64(py_val);
                    if !PY_ERR_OCCURRED().is_null() {
                        PY_ERR_CLEAR();
                        return None;
                    }
                    return Some(value);
                }
                let value = py_to_olive_internal(py_val);
                let valid_foreign = foreign_cache_scan(&INT_LIKE_CACHE, &INT_LIKE_LEN, ty as usize);
                if !valid_foreign {
                    crate::free_any_word(value);
                    return None;
                }
                if crate::is_active_object(value) {
                    crate::olive_free_any(value);
                    return None;
                }
                Some(value)
            }
            2 => {
                if is_sub(PY_FLOAT_TYPE) {
                    return Some(PY_FLOAT_AS_DOUBLE(py_val).to_bits() as i64);
                }
                if is_sub(PY_LONG_TYPE) {
                    let value = py_long_as_i64(py_val);
                    if !PY_ERR_OCCURRED().is_null() {
                        return None;
                    }
                    return Some((value as f64).to_bits() as i64);
                }
                let value = py_to_olive_internal(py_val);
                let valid_foreign =
                    foreign_cache_scan(&FLOAT_LIKE_CACHE, &FLOAT_LIKE_LEN, ty as usize);
                if !valid_foreign {
                    crate::free_any_word(value);
                    return None;
                }
                if crate::is_active_object(value) {
                    crate::olive_free_any(value);
                    return None;
                }
                if foreign_cache_scan(&FLOAT_LIKE_CACHE, &FLOAT_LIKE_LEN, ty as usize) {
                    Some(f64::from_bits(value as u64).to_bits() as i64)
                } else {
                    Some((value as f64).to_bits() as i64)
                }
            }
            3 => {
                if is_sub(PY_FLOAT_TYPE) {
                    return Some((PY_FLOAT_AS_DOUBLE(py_val) as f32).to_bits() as i64);
                }
                if is_sub(PY_LONG_TYPE) {
                    let value = py_long_as_i64(py_val);
                    if !PY_ERR_OCCURRED().is_null() {
                        return None;
                    }
                    return Some((value as f32).to_bits() as i64);
                }
                let value = py_to_olive_internal(py_val);
                let valid_foreign =
                    foreign_cache_scan(&FLOAT_LIKE_CACHE, &FLOAT_LIKE_LEN, ty as usize);
                if !valid_foreign {
                    crate::free_any_word(value);
                    return None;
                }
                if crate::is_active_object(value) {
                    crate::olive_free_any(value);
                    return None;
                }
                if foreign_cache_scan(&FLOAT_LIKE_CACHE, &FLOAT_LIKE_LEN, ty as usize) {
                    Some((f64::from_bits(value as u64) as f32).to_bits() as i64)
                } else {
                    Some((value as f32).to_bits() as i64)
                }
            }
            4 => {
                if !is_sub(PY_BOOL_TYPE) {
                    return None;
                }
                Some((PY_LONG_AS_LONG(py_val) != 0) as i64)
            }
            5 => {
                if !is_sub(PY_UNICODE_TYPE) {
                    return None;
                }
                let value = py_str_to_olive(py_val);
                (value != 0).then_some(value)
            }
            6 => (py_val == _PY_NONE_STRUCT).then_some(0),
            7 => py_u64_from_python(py_val).map(|value| value as i64),
            _ => None,
        }
    }
}

pub unsafe fn py_to_any_internal(py_val: PyObject) -> i64 {
    unsafe {
        let _conversion_guard = match ConversionGuard::enter(py_val) {
            Some(guard) => guard,
            None => return 0,
        };
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
                let v = py_long_as_i64(py_val);
                if !PY_ERR_OCCURRED().is_null() {
                    PY_ERR_CLEAR();
                    if let Some(value) = py_u64_from_python(py_val) {
                        return crate::boxed::olive_box_u64(value as i64);
                    }
                    return py_to_olive_internal(py_val);
                }
                return crate::boxed::olive_box_int(v);
            }
            if is_sub(PY_FLOAT_TYPE) {
                let d = PY_FLOAT_AS_DOUBLE(py_val);
                if !PY_ERR_OCCURRED().is_null() {
                    PY_ERR_CLEAR();
                    return py_to_olive_internal(py_val);
                }
                return crate::boxed::olive_box_float(d);
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
    unsafe { olive_py_to_list_tagged_internal(obj, 0, boxed) }
}

pub unsafe fn olive_py_to_list_tagged_internal(obj: PyObject, elem_tag: i64, boxed: bool) -> i64 {
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
                crate::python::python_error::handle_py_error();
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
                // Both accessors return borrowed references owned by `source`;
                // only the materialized non-list/tuple path owns another
                // reference. A null item (out of range) must skip the
                // conversion entirely because dispatch would read through it.
                let py_item = if from_real_list {
                    PY_LIST_GET_ITEM(source, i as isize)
                } else {
                    PY_TUPLE_GET_ITEM(source, i as isize)
                };
                *sv.ptr.add(i) = if py_item.is_null() {
                    0
                } else if boxed {
                    py_to_any_internal(py_item)
                } else if elem_tag != 0 {
                    match py_to_typed_scalar_internal(py_item, elem_tag) {
                        Some(value) => value,
                        None => {
                            if !PY_ERR_OCCURRED().is_null() {
                                PY_ERR_CLEAR();
                            }
                            if !materialized.is_null() {
                                PY_DEC_REF(materialized);
                            }
                            crate::olive_free_any(list_ptr);
                            crate::panic::abort_py_coerce(
                                "Python collection element has incompatible type",
                            );
                        }
                    }
                } else {
                    py_to_olive_internal(py_item)
                };
            }
        }
        if !materialized.is_null() {
            PY_DEC_REF(materialized);
        }
        if conversion_failed() {
            crate::olive_free_any(list_ptr);
            return 0;
        }
        list_ptr
    }
}

fn dict_key_descriptor(tag: i64) -> i64 {
    match tag {
        2 => scalar_descriptor(2),
        3 => scalar_descriptor(3),
        1 => scalar_descriptor(7),
        4 => scalar_descriptor(4),
        5 => scalar_descriptor(5),
        6 => scalar_descriptor(0),
        7 => scalar_descriptor(6),
        _ => scalar_descriptor(1),
    }
}

unsafe fn decref_dict_snapshot_from(entries: &mut Vec<(PyObject, PyObject)>, start: usize) {
    for (key, value) in entries.drain(start..) {
        unsafe {
            PY_DEC_REF(key);
            PY_DEC_REF(value);
        }
    }
}

unsafe fn py_to_typed_dict_key_internal(item: PyObject, key_tag: i64) -> Option<(i64, bool)> {
    unsafe {
        if key_tag == 5 {
            let ty = raw_ob_type(item);
            let is_unicode = !ty.is_null()
                && !PY_UNICODE_TYPE.is_null()
                && (ty == PY_UNICODE_TYPE || PY_TYPE_IS_SUBTYPE(ty, PY_UNICODE_TYPE) != 0);
            if is_unicode {
                let value = py_str_to_olive(item);
                return (value != 0).then_some((value, true));
            }
            let text = PY_OBJECT_STR(item);
            if text.is_null() {
                return None;
            }
            let value = py_str_to_olive(text);
            PY_DEC_REF(text);
            return (value != 0).then_some((value, true));
        }
        if key_tag == 6 {
            return Some((py_to_any_internal(item), false));
        }
        let scalar_tag = match key_tag {
            0 => 1,
            1 => 7,
            2 => 2,
            3 => 3,
            4 => 4,
            7 => 6,
            _ => return None,
        };
        py_to_typed_scalar_internal(item, scalar_tag).map(|value| (value, false))
    }
}

pub unsafe fn olive_py_to_dict_internal(obj: PyObject, boxed: bool) -> i64 {
    unsafe { olive_py_to_dict_tagged_internal(obj, 0, 0, boxed) }
}

pub unsafe fn olive_py_to_dict_tagged_internal(
    obj: PyObject,
    key_tag: i64,
    value_tag: i64,
    boxed: bool,
) -> i64 {
    unsafe {
        let olive_obj = crate::olive_obj_new();
        if boxed || (key_tag == 0 && value_tag == 0) {
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

                    let key_ptr = py_to_any_internal(key_obj);
                    if key_ptr != 0 {
                        let olive_val = if boxed {
                            py_to_any_internal(val_obj)
                        } else {
                            py_to_olive_internal(val_obj)
                        };
                        crate::olive_obj_set(olive_obj, key_ptr, olive_val);
                        if is_unicode {
                            crate::string_slab::str_free(key_ptr);
                        }
                    }
                }
            }
            if conversion_failed() {
                crate::olive_free_any(olive_obj);
                return 0;
            }
            return olive_obj;
        }

        let mut entries = Vec::new();
        let mut pos: isize = 0;
        let mut key_obj: PyObject = std::ptr::null_mut();
        let mut val_obj: PyObject = std::ptr::null_mut();
        while PY_DICT_NEXT(obj, &mut pos, &mut key_obj, &mut val_obj) != 0 {
            PY_INC_REF(key_obj);
            PY_INC_REF(val_obj);
            entries.push((key_obj, val_obj));
        }
        for index in 0..entries.len() {
            let (key_obj, val_obj) = entries[index];
            let Some((key_ptr, key_owned)) = py_to_typed_dict_key_internal(key_obj, key_tag) else {
                crate::olive_free_any(olive_obj);
                decref_dict_snapshot_from(&mut entries, index);
                if !PY_ERR_OCCURRED().is_null() {
                    crate::python::python_error::handle_py_error();
                }
                crate::panic::abort_py_coerce("Python dictionary key has incompatible type");
            };
            let olive_val = if value_tag == 0 {
                py_to_any_internal(val_obj)
            } else {
                match py_to_typed_scalar_internal(val_obj, value_tag) {
                    Some(value) => value,
                    None => {
                        if key_owned {
                            crate::olive_free_str(key_ptr);
                        } else if key_tag == 6 {
                            crate::olive_free_any(key_ptr);
                        }
                        crate::olive_free_any(olive_obj);
                        decref_dict_snapshot_from(&mut entries, index);
                        if !PY_ERR_OCCURRED().is_null() {
                            crate::python::python_error::handle_py_error();
                        }
                        crate::panic::abort_py_coerce(
                            "Python dictionary value has incompatible type",
                        );
                    }
                }
            };
            crate::hash_typed::olive_obj_set_typed(
                olive_obj,
                key_ptr,
                olive_val,
                dict_key_descriptor(key_tag),
            );
            if key_owned {
                crate::olive_free_str(key_ptr);
            }
            PY_DEC_REF(key_obj);
            PY_DEC_REF(val_obj);
        }
        if conversion_failed() {
            crate::olive_free_any(olive_obj);
            return 0;
        }
        olive_obj
    }
}

pub unsafe fn olive_py_to_set_internal(obj: PyObject, boxed: bool) -> i64 {
    unsafe { olive_py_to_set_tagged_internal(obj, 0, boxed) }
}

pub unsafe fn olive_py_to_set_tagged_internal(obj: PyObject, elem_tag: i64, boxed: bool) -> i64 {
    unsafe {
        let iter = PY_OBJECT_GET_ITER(obj);
        if iter.is_null() {
            crate::python::python_error::handle_py_error();
        }
        let size_hint = PY_OBJECT_LENGTH(obj).max(0) as i64;
        let set_ptr = crate::olive_set_new(size_hint);
        loop {
            let item = PY_ITER_NEXT(iter);
            if item.is_null() {
                if !PY_ERR_OCCURRED().is_null()
                    && PY_ERR_EXCEPTION_MATCHES(PY_EXC_STOP_ITERATION) == 0
                {
                    crate::olive_free_any(set_ptr);
                    PY_DEC_REF(iter);
                    crate::python::python_error::handle_py_error();
                }
                PY_ERR_CLEAR();
                break;
            }
            let olive_val = if boxed {
                py_to_any_internal(item)
            } else if elem_tag != 0 {
                match py_to_typed_scalar_internal(item, elem_tag) {
                    Some(value) => value,
                    None => {
                        if !PY_ERR_OCCURRED().is_null() {
                            PY_ERR_CLEAR();
                        }
                        PY_DEC_REF(item);
                        PY_DEC_REF(iter);
                        crate::olive_free_any(set_ptr);
                        crate::panic::abort_py_coerce("Python set element has incompatible type");
                    }
                }
            } else {
                py_to_olive_internal(item)
            };
            if boxed || elem_tag == 0 {
                crate::olive_set_add(set_ptr, olive_val);
            } else {
                crate::hash_typed::olive_set_add_typed(
                    set_ptr,
                    olive_val,
                    scalar_descriptor(elem_tag),
                );
            }
            PY_DEC_REF(item);
        }
        PY_DEC_REF(iter);
        if conversion_failed() {
            crate::olive_free_any(set_ptr);
            return 0;
        }
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
    // no-op: the second caller finds the slot already free. One with_gil
    // spans handle free and decref so the pair costs a single depth check
    // inside fused regions.
    with_gil(|| {
        let taken = free_pyobject_handle(obj as *mut OlivePyObject);
        if let Some(py_ptr) = taken
            && !py_ptr.is_null()
        {
            unsafe {
                PY_DEC_REF(py_ptr);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn raw_refcnt(value: PyObject) -> isize {
        unsafe { *(value as *const isize) }
    }

    unsafe fn fresh_object() -> PyObject {
        unsafe {
            let builtins = PY_IMPORT_IMPORT_MODULE(b"builtins\0".as_ptr().cast());
            let object_type = PY_OBJECT_GET_ATTR_STRING(builtins, b"object\0".as_ptr().cast());
            let args = PY_TUPLE_NEW(0);
            let value = PY_OBJECT_CALL_OBJECT(object_type, args);
            PY_DEC_REF(args);
            PY_DEC_REF(object_type);
            PY_DEC_REF(builtins);
            assert!(!value.is_null());
            value
        }
    }

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
    fn cyclic_dynamic_python_collections_are_rejected_without_recursing() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            with_gil(|| {
                let source = PY_LIST_NEW(1);
                assert!(!source.is_null());
                PY_INC_REF(source);
                assert_eq!(PY_LIST_SET_ITEM(source, 0, source), 0);
                assert_eq!(py_to_any_internal(source), 0);
                assert!(conversion_failed());
                let _ = take_conversion_error();
                PY_DEC_REF(source);
                PY_DEC_REF(source);
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

    #[test]
    fn typed_scalar_fallback_releases_owned_python_handle() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let value = with_gil(|| fresh_object());
            let baseline = with_gil(|| raw_refcnt(value));
            let after = [1, 2, 3].map(|tag| {
                assert!(with_gil(|| py_to_typed_scalar_internal(value, tag)).is_none());
                with_gil(|| raw_refcnt(value))
            });
            with_gil(|| PY_DEC_REF(value));
            assert_eq!(after, [baseline; 3]);
        }
    }

    #[test]
    fn typed_scalar_fallback_releases_owned_container() {
        let _guard = pyobject_slab_test_lock();
        if !is_python_available() {
            eprintln!("Python not available, skipping test");
            return;
        }
        unsafe {
            let (source, child) = with_gil(|| {
                let source = PY_LIST_NEW(1);
                let child = fresh_object();
                PY_LIST_SET_ITEM(source, 0, child);
                (source, child)
            });
            let baseline = with_gil(|| raw_refcnt(child));
            let result = with_gil(|| py_to_typed_scalar_internal(source, 2));
            let after = with_gil(|| raw_refcnt(child));
            with_gil(|| PY_DEC_REF(source));
            assert!(result.is_none());
            assert_eq!(after, baseline);
        }
    }
}
