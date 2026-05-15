use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use rustc_hash::{FxHashMap as HashMap, FxHashSet};
extern crate libc;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub mod aio;
pub mod bytes;
pub mod compress;
pub mod crypto;
pub mod datetime;
pub mod encoding;
pub mod io;
pub mod json;
pub mod logging;
pub mod math;
pub mod net;
pub mod os;
pub mod random;
pub mod regex;
pub mod requests;
pub mod result;
pub mod sys;
pub mod uuid;
pub mod websocket;
pub mod yaml;

pub(crate) const KIND_LIST: i64 = 1;
pub(crate) const KIND_OBJ: i64 = 2;
pub(crate) const KIND_ENUM: i64 = 3;
pub(crate) const KIND_SET: i64 = 4;
pub(crate) const KIND_BYTES: i64 = 6;

#[repr(C)]
pub struct StableVec {
    pub kind: i64,
    pub ptr: *mut i64,
    pub cap: usize,
    pub len: usize,
}

#[repr(C)]
pub struct OliveObj {
    pub kind: i64,
    pub fields: HashMap<String, i64>,
}

#[repr(C)]
pub struct OliveEnum {
    pub kind: i64,
    pub type_id: i64,
    pub tag: i64,
    pub payload_ptr: *mut i64,
    pub payload_len: usize,
}

#[repr(C)]
pub struct OliveHashSet {
    pub kind: i64,
    pub ptr: *mut i64,
    pub cap: usize,
    pub len: usize,
    pub inner: *mut FxHashSet<i64>,
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_alloc(size: i64) -> *mut u8 {
    let layout = std::alloc::Layout::from_size_align(size as usize, 8).unwrap();
    unsafe { std::alloc::alloc(layout) }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_c_struct(ptr: *mut u8, size: i64) {
    if !ptr.is_null() {
        let layout = std::alloc::Layout::from_size_align(size as usize, 8).unwrap();
        unsafe { std::alloc::dealloc(ptr, layout) }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_vararg_call(
    fn_ptr: i64,
    n_fixed: i64,
    n_total: i64,
    arg_types: *const i64,
    arg_vals: *const i64,
) -> i64 {
    use libffi::middle::{arg, Cif, CodePtr, Type};
    let n = n_total as usize;
    let nf = (n_fixed as usize).max(1).min(n);
    let types: Vec<Type> = (0..n)
        .map(|i| {
            if unsafe { *arg_types.add(i) } == 1 {
                Type::f64()
            } else {
                Type::i64()
            }
        })
        .collect();
    let cif = Cif::new_variadic(types.into_iter(), nf, Type::i64());
    let vals: Vec<i64> = (0..n).map(|i| unsafe { *arg_vals.add(i) }).collect();
    let ffi_args: Vec<_> = vals.iter().map(|v| arg(v)).collect();
    unsafe { cif.call::<i64>(CodePtr(fn_ptr as *mut _), &ffi_args) }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print(val: i64) -> i64 {
    println!("{}", val);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print_float(val: f64) -> i64 {
    println!("{}", val);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print_str(val: i64) -> i64 {
    if val == 0 {
        println!("None");
    } else {
        println!("{}", olive_str_from_ptr(val));
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print_list(ptr: i64) -> i64 {
    if ptr == 0 {
        println!("[]");
        return 0;
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    print!("[");
    for i in 0..v.len {
        if i > 0 {
            print!(", ");
        }
        let elem = unsafe { *v.ptr.add(i) };
        print!("{}", elem);
    }
    println!("]");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print_obj(ptr: i64) -> i64 {
    if ptr == 0 {
        println!("{{}}");
        return 0;
    }
    let m = unsafe { &*(ptr as *const OliveObj) };
    print!("{{");
    for (i, (k, &v)) in m.fields.iter().enumerate() {
        if i > 0 {
            print!(", ");
        }
        print!("'{}': {}", k, v);
    }
    println!("}}");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str(val: i64) -> i64 {
    olive_str_internal(&val.to_string())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_int(val: i64) -> i64 {
    val
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_float(val: i64) -> f64 {
    val as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bool(val: i64) -> i64 {
    if val != 0 { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bool_from_float(val: f64) -> i64 {
    if val != 0.0 { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_float_to_str(val: f64) -> i64 {
    olive_str_internal(&format!("{}", val))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_float_to_int(val: f64) -> i64 {
    val as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_int_to_float(val: i64) -> f64 {
    val as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_to_int(ptr: i64) -> i64 {
    olive_str_from_ptr(ptr).parse::<i64>().unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_to_float(ptr: i64) -> f64 {
    olive_str_from_ptr(ptr).parse::<f64>().unwrap_or(0.0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_concat(l: i64, r: i64) -> i64 {
    let l_bytes = if l == 0 {
        b"" as &[u8]
    } else {
        unsafe { std::ffi::CStr::from_ptr((l & !1) as *const std::ffi::c_char).to_bytes() }
    };
    let r_bytes = if r == 0 {
        b"" as &[u8]
    } else {
        unsafe { std::ffi::CStr::from_ptr((r & !1) as *const std::ffi::c_char).to_bytes() }
    };
    let mut buf = Vec::with_capacity(l_bytes.len() + r_bytes.len() + 1);
    buf.extend_from_slice(l_bytes);
    buf.extend_from_slice(r_bytes);
    let c_str = unsafe { std::ffi::CString::from_vec_unchecked(buf) };
    c_str.into_raw() as i64 | 1
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_eq(l: i64, r: i64) -> i64 {
    if l == r {
        return 1;
    }
    if l == 0 || r == 0 {
        return 0;
    }
    let l_cstr = unsafe { std::ffi::CStr::from_ptr((l & !1) as *const std::ffi::c_char) };
    let r_cstr = unsafe { std::ffi::CStr::from_ptr((r & !1) as *const std::ffi::c_char) };
    if l_cstr == r_cstr { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_copy(ptr: i64) -> i64 {
    olive_str_internal(&olive_str_from_ptr(ptr))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_copy_float(val: f64) -> f64 {
    val
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_new(len: i64) -> i64 {
    let n = len as usize;
    let total = 4 + n; // StableVec header = 4 × i64; data follows
    let layout = unsafe { std::alloc::Layout::from_size_align_unchecked(total * 8, 8) };
    let raw = unsafe { std::alloc::alloc(layout) as *mut i64 };
    if raw.is_null() {
        std::alloc::handle_alloc_error(layout);
    }
    let data_ptr = unsafe { raw.add(4) };
    unsafe {
        let s = &mut *(raw as *mut StableVec);
        s.kind = KIND_LIST;
        s.ptr = data_ptr;
        s.cap = n;
        s.len = n;
    }
    raw as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_set(list_ptr: i64, idx: i64, val: i64) {
    if list_ptr == 0 {
        return;
    }
    let s = unsafe { &mut *(list_ptr as *mut StableVec) };
    if (idx as usize) < s.len {
        unsafe {
            *s.ptr.add(idx as usize) = val;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_get(list_ptr: i64, idx: i64) -> i64 {
    if list_ptr == 0 {
        return 0;
    }
    let s = unsafe { &*(list_ptr as *const StableVec) };
    if (idx as usize) < s.len {
        unsafe { *s.ptr.add(idx as usize) }
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_len(ptr: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    unsafe { (*(ptr as *const StableVec)).len as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_append(list_ptr: i64, val: i64) {
    if list_ptr == 0 {
        return;
    }
    unsafe {
        let s = &mut *(list_ptr as *mut StableVec);
        let inline_data = (list_ptr as *mut i64).add(4);
        let mut v = if s.ptr == inline_data {
            let mut owned = Vec::with_capacity(s.len + 1);
            owned.extend_from_slice(std::slice::from_raw_parts(s.ptr, s.len));
            owned
        } else {
            Vec::from_raw_parts(s.ptr, s.len, s.cap)
        };
        v.push(val);
        s.ptr = v.as_mut_ptr();
        s.cap = v.capacity();
        s.len = v.len();
        std::mem::forget(v);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_concat(l: i64, r: i64) -> i64 {
    if l == 0 {
        return r;
    }
    if r == 0 {
        return l;
    }
    let sl = unsafe { &*(l as *const StableVec) };
    let sr = unsafe { &*(r as *const StableVec) };
    let mut v = Vec::with_capacity(sl.len + sr.len);
    unsafe {
        v.extend_from_slice(std::slice::from_raw_parts(sl.ptr, sl.len));
        v.extend_from_slice(std::slice::from_raw_parts(sr.ptr, sr.len));
    }
    let ptr = v.as_mut_ptr();
    let cap = v.capacity();
    let len = v.len();
    std::mem::forget(v);
    Box::into_raw(Box::new(StableVec {
        kind: KIND_LIST,
        ptr,
        cap,
        len,
    })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_in_list(val: i64, list_ptr: i64) -> i64 {
    if list_ptr == 0 {
        return 0;
    }
    let kind = unsafe { *(list_ptr as *const i64) };
    if kind == KIND_SET {
        let s = unsafe { &*(list_ptr as *const OliveHashSet) };
        return if unsafe { (*s.inner).contains(&val) } { 1 } else { 0 };
    }
    let s = unsafe { &*(list_ptr as *const StableVec) };
    for i in 0..s.len {
        if unsafe { *s.ptr.add(i) } == val {
            return 1;
        }
    }
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_new(capacity: i64) -> i64 {
    let cap = capacity as usize;
    let mut v: Vec<i64> = Vec::with_capacity(cap);
    let ptr = v.as_mut_ptr();
    let v_cap = v.capacity();
    std::mem::forget(v);
    let inner = Box::into_raw(Box::new(FxHashSet::<i64>::default()));
    Box::into_raw(Box::new(OliveHashSet { kind: KIND_SET, ptr, cap: v_cap, len: 0, inner })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_add(set_ptr: i64, val: i64) {
    if set_ptr == 0 {
        return;
    }
    unsafe {
        let s = &mut *(set_ptr as *mut OliveHashSet);
        let hs = &mut *s.inner;
        if hs.insert(val) {
            let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
            v.push(val);
            s.ptr = v.as_mut_ptr();
            s.cap = v.capacity();
            s.len = v.len();
            std::mem::forget(v);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_new() -> i64 {
    Box::into_raw(Box::new(OliveObj {
        kind: KIND_OBJ,
        fields: HashMap::default(),
    })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_set(obj_ptr: i64, attr: i64, val: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        return obj_ptr;
    }
    let attr_str = olive_str_from_ptr(attr);
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    m.fields.insert(attr_str, val);
    obj_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get(obj_ptr: i64, attr: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        return 0;
    }
    let attr_str = olive_str_from_ptr(attr);
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    *m.fields.get(&attr_str).unwrap_or(&0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_in_obj(key: i64, obj_ptr: i64) -> i64 {
    if obj_ptr == 0 || key == 0 {
        return 0;
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    if m.fields.contains_key(&olive_str_from_ptr(key)) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_len(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        return 0;
    }
    unsafe { (*(obj_ptr as *const OliveObj)).fields.len() as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_struct_alloc(n_fields: i64) -> i64 {
    let total = (n_fields + 1) * 8;
    let layout = std::alloc::Layout::from_size_align(total as usize, 8).unwrap();
    let ptr = unsafe { std::alloc::alloc(layout) } as i64;
    unsafe { *(ptr as *mut i64) = n_fields };
    ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_struct(ptr: i64) {
    if ptr == 0 {
        return;
    }
    unsafe {
        let n_fields = *(ptr as *const i64);
        let total = ((n_fields + 1) * 8) as usize;
        let layout = std::alloc::Layout::from_size_align_unchecked(total, 8);
        std::alloc::dealloc(ptr as *mut u8, layout);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_str(ptr: i64) {
    if ptr != 0 && (ptr & 1) == 0 {
        unsafe {
            let _ = std::ffi::CString::from_raw(ptr as *mut std::ffi::c_char);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_list(ptr: i64) {
    if ptr != 0 {
        unsafe {
            let s = &*(ptr as *const StableVec);
            let inline_data = (ptr as *mut i64).add(4);
            if s.ptr == inline_data {
                let total = (4 + s.len) * 8;
                let layout = std::alloc::Layout::from_size_align_unchecked(total, 8);
                std::alloc::dealloc(ptr as *mut u8, layout);
            } else {
                let s = Box::from_raw(ptr as *mut StableVec);
                if !s.ptr.is_null() {
                    let _ = Vec::from_raw_parts(s.ptr, s.len, s.cap);
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_obj(ptr: i64) {
    if ptr != 0 {
        unsafe {
            let _ = Box::from_raw(ptr as *mut OliveObj);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_time_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_time_monotonic() -> f64 {
    static START: OnceLock<SystemTime> = OnceLock::new();
    let start = START.get_or_init(SystemTime::now);
    SystemTime::now()
        .duration_since(*start)
        .unwrap()
        .as_secs_f64()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_time_sleep(secs: f64) {
    thread::sleep(Duration::from_secs_f64(secs));
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_pow(base: i64, exp: i64) -> i64 {
    base.pow(exp as u32)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_pow_float(base: f64, exp: f64) -> f64 {
    base.powf(exp)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_new(type_id: i64, tag: i64, arg_count: i64) -> i64 {
    let mut payload = vec![0i64; arg_count as usize];
    let payload_ptr = payload.as_mut_ptr();
    let payload_len = payload.len();
    std::mem::forget(payload);
    Box::into_raw(Box::new(OliveEnum {
        kind: KIND_ENUM,
        type_id,
        tag,
        payload_ptr,
        payload_len,
    })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_type_id(ptr: i64) -> i64 {
    if ptr == 0 {
        return -1;
    }
    unsafe { (*(ptr as *const OliveEnum)).type_id }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_tag(ptr: i64) -> i64 {
    if ptr == 0 {
        return -1;
    }
    unsafe { (*(ptr as *const OliveEnum)).tag }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_get(ptr: i64, index: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    let e = unsafe { &*(ptr as *const OliveEnum) };
    if (index as usize) < e.payload_len {
        unsafe { *e.payload_ptr.add(index as usize) }
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_set(ptr: i64, index: i64, val: i64) {
    if ptr == 0 {
        return;
    }
    let e = unsafe { &mut *(ptr as *mut OliveEnum) };
    if (index as usize) < e.payload_len {
        unsafe {
            *e.payload_ptr.add(index as usize) = val;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_enum(ptr: i64) {
    if ptr != 0 {
        unsafe {
            let e = Box::from_raw(ptr as *mut OliveEnum);
            let _ = Vec::from_raw_parts(e.payload_ptr, e.payload_len, e.payload_len);
        }
    }
}

struct OliveIter {
    list_ptr: i64,
    index: usize,
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_iter(list_ptr: i64) -> i64 {
    Box::into_raw(Box::new(OliveIter { list_ptr, index: 0 })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_has_next(iter_ptr: i64) -> i64 {
    if iter_ptr == 0 {
        return 0;
    }
    let it = unsafe { &*(iter_ptr as *const OliveIter) };
    if it.list_ptr == 0 {
        return 0;
    }
    let s = unsafe { &*(it.list_ptr as *const StableVec) };
    if it.index < s.len { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_next(iter_ptr: i64) -> i64 {
    if iter_ptr == 0 {
        return 0;
    }
    let it = unsafe { &mut *(iter_ptr as *mut OliveIter) };
    if it.list_ptr == 0 {
        return 0;
    }
    let s = unsafe { &*(it.list_ptr as *const StableVec) };
    if it.index < s.len {
        let val = unsafe { *s.ptr.add(it.index) };
        it.index += 1;
        val
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_len(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    unsafe { std::ffi::CStr::from_ptr((s & !1) as *const std::ffi::c_char).to_bytes().len() as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_get(s: i64, i: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let ptr = (s & !1) as *const u8;
    let byte = unsafe { *ptr.add(i as usize) };
    if byte == 0 {
        return 0;
    }
    let buf = [byte, 0u8];
    let c_str = unsafe { std::ffi::CString::from_vec_unchecked(buf.to_vec()) };
    c_str.into_raw() as i64 | 1
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_char(s: i64, i: i64) -> i64 {
    olive_str_get(s, i)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_slice(s: i64, start: i64, end: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    let start = start as usize;
    let end = end as usize;
    if start <= end && end <= text.len() {
        olive_str_internal(&text[start..end])
    } else {
        0
    }
}

pub fn olive_str_internal(s: &str) -> i64 {
    let c_str = std::ffi::CString::new(s).unwrap_or_else(|_| {
        let safe: String = s.chars().filter(|&c| c != '\0').collect();
        std::ffi::CString::new(safe).unwrap()
    });
    c_str.into_raw() as i64 | 1
}

pub fn olive_str_from_ptr(ptr: i64) -> String {
    if ptr == 0 {
        return String::new();
    }
    let p = ptr & !1;
    unsafe {
        std::ffi::CStr::from_ptr(p as *const std::ffi::c_char)
            .to_string_lossy()
            .into_owned()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_get_index_any(obj: i64, index: i64) -> i64 {
    if obj == 0 {
        return 0;
    }
    if obj & 1 != 0 {
        return olive_str_get(obj, index);
    }
    let kind = unsafe { *(obj as *const i64) };
    match kind {
        KIND_LIST => olive_list_get(obj, index),
        KIND_OBJ => olive_obj_get(obj, index),
        KIND_ENUM => olive_enum_get(obj, index),
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_index_any(obj: i64, index: i64, val: i64) {
    if obj == 0 || (obj & 1 != 0) {
        return;
    }
    let kind = unsafe { *(obj as *const i64) };
    match kind {
        KIND_LIST => olive_list_set(obj, index, val),
        KIND_OBJ => {
            olive_obj_set(obj, index, val);
        }
        _ => {}
    }
}

fn olive_free_set(ptr: i64) {
    if ptr != 0 {
        unsafe {
            let s = Box::from_raw(ptr as *mut OliveHashSet);
            if !s.ptr.is_null() {
                let _ = Vec::from_raw_parts(s.ptr, s.len, s.cap);
            }
            if !s.inner.is_null() {
                let _ = Box::from_raw(s.inner);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_any(ptr: i64) {
    if ptr == 0 {
        return;
    }
    if ptr & 1 != 0 {
        olive_free_str(ptr);
        return;
    }
    let kind = unsafe { *(ptr as *const i64) };
    match kind {
        KIND_LIST => olive_free_list(ptr),
        KIND_SET => olive_free_set(ptr),
        KIND_OBJ => olive_free_obj(ptr),
        KIND_ENUM => olive_free_enum(ptr),
        _ => {}
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    olive_str_internal(olive_str_from_ptr(s).trim())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim_start(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    olive_str_internal(olive_str_from_ptr(s).trim_start())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_trim_end(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    olive_str_internal(olive_str_from_ptr(s).trim_end())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_upper(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    olive_str_internal(&olive_str_from_ptr(s).to_uppercase())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_lower(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    olive_str_internal(&olive_str_from_ptr(s).to_lowercase())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_replace(s: i64, from: i64, to: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = olive_str_from_ptr(s);
    let from_str = olive_str_from_ptr(from);
    let to_str = olive_str_from_ptr(to);
    olive_str_internal(&text.replace(&from_str, &to_str))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_find(s: i64, needle: i64) -> i64 {
    if s == 0 || needle == 0 {
        return -1;
    }
    let text = olive_str_from_ptr(s);
    let pat = olive_str_from_ptr(needle);
    match text.find(&pat) {
        Some(i) => i as i64,
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_contains(s: i64, needle: i64) -> i64 {
    if s == 0 || needle == 0 {
        return 0;
    }
    if olive_str_from_ptr(s).contains(&olive_str_from_ptr(needle)) { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_starts_with(s: i64, prefix: i64) -> i64 {
    if s == 0 || prefix == 0 {
        return 0;
    }
    if olive_str_from_ptr(s).starts_with(&olive_str_from_ptr(prefix)) { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_ends_with(s: i64, suffix: i64) -> i64 {
    if s == 0 || suffix == 0 {
        return 0;
    }
    if olive_str_from_ptr(s).ends_with(&olive_str_from_ptr(suffix)) { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_repeat(s: i64, n: i64) -> i64 {
    if s == 0 || n <= 0 {
        return olive_str_internal("");
    }
    olive_str_internal(&olive_str_from_ptr(s).repeat(n as usize))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_split(s: i64, sep: i64) -> i64 {
    let text = if s == 0 { String::new() } else { olive_str_from_ptr(s) };
    let parts: Vec<i64> = if sep == 0 {
        text.split_whitespace().map(olive_str_internal).collect()
    } else {
        let sep_str = olive_str_from_ptr(sep);
        text.split(&sep_str).map(olive_str_internal).collect()
    };
    let mut v = parts;
    let ptr = v.as_mut_ptr();
    let cap = v.capacity();
    let len = v.len();
    std::mem::forget(v);
    Box::into_raw(Box::new(StableVec { kind: KIND_LIST, ptr, cap, len })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_join(list_ptr: i64, sep: i64) -> i64 {
    if list_ptr == 0 {
        return olive_str_internal("");
    }
    let s = unsafe { &*(list_ptr as *const StableVec) };
    let sep_str = if sep == 0 { String::new() } else { olive_str_from_ptr(sep) };
    let parts: Vec<String> = (0..s.len)
        .map(|i| olive_str_from_ptr(unsafe { *s.ptr.add(i) }))
        .collect();
    olive_str_internal(&parts.join(&sep_str))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_fmt(template: i64, args: i64) -> i64 {
    if template == 0 {
        return olive_str_internal("");
    }
    let tmpl = olive_str_from_ptr(template);
    let arg_strs: Vec<String> = if args == 0 {
        vec![]
    } else {
        let sv = unsafe { &*(args as *const StableVec) };
        (0..sv.len).map(|i| olive_str_from_ptr(unsafe { *sv.ptr.add(i) })).collect()
    };
    let mut result = String::with_capacity(tmpl.len());
    let mut arg_idx = 0;
    let mut chars = tmpl.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'}') {
            chars.next();
            if arg_idx < arg_strs.len() {
                result.push_str(&arg_strs[arg_idx]);
                arg_idx += 1;
            }
        } else {
            result.push(c);
        }
    }
    olive_str_internal(&result)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_char_count(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    olive_str_from_ptr(s).chars().count() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_is_ascii(s: i64) -> i64 {
    if s == 0 {
        return 1;
    }
    if olive_str_from_ptr(s).is_ascii() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_grapheme_count(s: i64) -> i64 {
    use unicode_segmentation::UnicodeSegmentation;
    if s == 0 {
        return 0;
    }
    olive_str_from_ptr(s).graphemes(true).count() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_graphemes(s: i64) -> i64 {
    use unicode_segmentation::UnicodeSegmentation;
    if s == 0 {
        let v = Box::new(StableVec { kind: KIND_LIST, ptr: std::ptr::null_mut(), cap: 0, len: 0 });
        return Box::into_raw(v) as i64;
    }
    let text = olive_str_from_ptr(s);
    let mut ptrs: Vec<i64> = text.graphemes(true).map(olive_str_internal).collect();
    let ptr = ptrs.as_mut_ptr();
    let cap = ptrs.capacity();
    let len = ptrs.len();
    std::mem::forget(ptrs);
    Box::into_raw(Box::new(StableVec { kind: KIND_LIST, ptr, cap, len })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_panic(msg: i64) -> i64 {
    let text = if msg == 0 {
        "panic".to_string()
    } else {
        olive_str_from_ptr(msg)
    };
    if let Some(hooks) = EXIT_HOOKS.get()
        && let Ok(list) = hooks.lock() {
            for &fn_ptr in list.iter() {
                let f: extern "C" fn() = unsafe { std::mem::transmute(fn_ptr as usize) };
                f();
            }
        }
    eprintln!("panic: {text}");
    std::process::exit(1);
}

static EXIT_HOOKS: OnceLock<Mutex<Vec<i64>>> = OnceLock::new();

fn exit_hooks() -> &'static Mutex<Vec<i64>> {
    EXIT_HOOKS.get_or_init(|| Mutex::new(Vec::new()))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_atexit(fn_ptr: i64) {
    if fn_ptr != 0 {
        exit_hooks().lock().unwrap().push(fn_ptr);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_run_exit_hooks() {
    if let Ok(list) = exit_hooks().lock() {
        for &fn_ptr in list.iter() {
            let f: extern "C" fn() = unsafe { std::mem::transmute(fn_ptr as usize) };
            f();
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_is_null(val: i64) -> i64 {
    if val == 0 { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_is_str(val: i64) -> i64 {
    if val != 0 && (val & 1) != 0 { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_is_list(val: i64) -> i64 {
    if val == 0 || (val & 1) != 0 {
        return 0;
    }
    let kind = unsafe { *(val as *const i64) };
    if kind == KIND_LIST { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_is_obj(val: i64) -> i64 {
    if val == 0 || (val & 1) != 0 {
        return 0;
    }
    let kind = unsafe { *(val as *const i64) };
    if kind == KIND_OBJ { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_is_bytes(val: i64) -> i64 {
    if val == 0 || (val & 1) != 0 {
        return 0;
    }
    let kind = unsafe { *(val as *const i64) };
    if kind == KIND_BYTES { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_typeof_str(val: i64) -> i64 {
    if val == 0 {
        return olive_str_internal("null");
    }
    if (val & 1) != 0 {
        return olive_str_internal("str");
    }
    let kind = unsafe { *(val as *const i64) };
    let name = match kind {
        KIND_LIST => "list",
        KIND_OBJ => "obj",
        KIND_ENUM => "enum",
        KIND_SET => "set",
        KIND_BYTES => "bytes",
        _ => "int",
    };
    olive_str_internal(name)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_keys(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        let v = Box::new(StableVec { kind: KIND_LIST, ptr: std::ptr::null_mut(), cap: 0, len: 0 });
        return Box::into_raw(v) as i64;
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let mut ptrs: Vec<i64> = m.fields.keys().map(|k| olive_str_internal(k)).collect();
    let ptr = ptrs.as_mut_ptr();
    let cap = ptrs.capacity();
    let len = ptrs.len();
    std::mem::forget(ptrs);
    Box::into_raw(Box::new(StableVec { kind: KIND_LIST, ptr, cap, len })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_values(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        let v = Box::new(StableVec { kind: KIND_LIST, ptr: std::ptr::null_mut(), cap: 0, len: 0 });
        return Box::into_raw(v) as i64;
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let mut vals: Vec<i64> = m.fields.values().copied().collect();
    let ptr = vals.as_mut_ptr();
    let cap = vals.capacity();
    let len = vals.len();
    std::mem::forget(vals);
    Box::into_raw(Box::new(StableVec { kind: KIND_LIST, ptr, cap, len })) as i64
}

pub fn unix_to_ymd_hms(ts: i64) -> (i64, i64, i64, i64, i64, i64) {
    let mut d = ts / 86400;
    let sec = ts.rem_euclid(86400);
    let h = sec / 3600;
    let m = (sec % 3600) / 60;
    let s = sec % 60;
    if ts < 0 && (ts % 86400) != 0 {
        d -= 1;
    }
    d += 719468;
    let era = if d >= 0 { d } else { d - 146096 } / 146097;
    let doe = d - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day, h, m, s)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_time_format(ts: f64, fmt: i64) -> i64 {
    let (year, month, day, h, m, s) = unix_to_ymd_hms(ts as i64);
    let fmt_str = if fmt == 0 {
        "%Y-%m-%dT%H:%M:%S".to_string()
    } else {
        olive_str_from_ptr(fmt)
    };
    let mut out = String::with_capacity(fmt_str.len() + 8);
    let mut chars = fmt_str.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.next() {
                Some('Y') => out.push_str(&format!("{:04}", year)),
                Some('m') => out.push_str(&format!("{:02}", month)),
                Some('d') => out.push_str(&format!("{:02}", day)),
                Some('H') => out.push_str(&format!("{:02}", h)),
                Some('M') => out.push_str(&format!("{:02}", m)),
                Some('S') => out.push_str(&format!("{:02}", s)),
                Some('%') => out.push('%'),
                Some(x) => { out.push('%'); out.push(x); }
                None => out.push('%'),
            }
        } else {
            out.push(c);
        }
    }
    olive_str_internal(&out)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_cache_has(cache_ptr: i64, key: i64) -> i64 {
    if cache_ptr == 0 {
        return 0;
    }
    let cache = unsafe { &*(cache_ptr as *const HashMap<i64, i64>) };
    if cache.contains_key(&key) { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_cache_get(cache_ptr: i64, key: i64) -> i64 {
    if cache_ptr == 0 {
        return 0;
    }
    let cache = unsafe { &*(cache_ptr as *const HashMap<i64, i64>) };
    *cache.get(&key).unwrap_or(&0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_cache_set(cache_ptr: i64, key: i64, val: i64) -> i64 {
    if cache_ptr == 0 {
        return 0;
    }
    let cache = unsafe { &mut *(cache_ptr as *mut HashMap<i64, i64>) };
    cache.insert(key, val);
    cache_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_memo_get(name_ptr: i64, is_tuple: i64) -> i64 {
    static GLOBAL_CACHES: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    static GLOBAL_CACHES_TUPLE: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();

    let name = olive_str_from_ptr(name_ptr);
    if is_tuple == 0 {
        let mut caches = GLOBAL_CACHES
            .get_or_init(|| Mutex::new(HashMap::default()))
            .lock()
            .unwrap();
        if let Some(&c) = caches.get(&name) {
            c
        } else {
            let new_cache = Box::into_raw(Box::new(HashMap::<i64, i64>::default())) as i64;
            caches.insert(name, new_cache);
            new_cache
        }
    } else {
        let mut caches = GLOBAL_CACHES_TUPLE
            .get_or_init(|| Mutex::new(HashMap::default()))
            .lock()
            .unwrap();
        if let Some(&c) = caches.get(&name) {
            c
        } else {
            let new_cache = Box::into_raw(Box::new(HashMap::<Vec<i64>, i64>::default())) as i64;
            caches.insert(name, new_cache);
            new_cache
        }
    }
}

fn read_tuple(ptr: i64) -> Vec<i64> {
    unsafe {
        let p = ptr as *const i64;
        let len = *p as usize;
        let mut v = Vec::with_capacity(len);
        for i in 0..len {
            v.push(*(p.add(i + 1)));
        }
        v
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_cache_has_tuple(cache_ptr: i64, key_ptr: i64) -> i64 {
    if cache_ptr == 0 {
        return 0;
    }
    let cache = unsafe { &*(cache_ptr as *const HashMap<Vec<i64>, i64>) };
    let v = read_tuple(key_ptr);
    if cache.contains_key(&v) { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_cache_get_tuple(cache_ptr: i64, key_ptr: i64) -> i64 {
    if cache_ptr == 0 {
        return 0;
    }
    let cache = unsafe { &*(cache_ptr as *const HashMap<Vec<i64>, i64>) };
    let v = read_tuple(key_ptr);
    *cache.get(&v).unwrap_or(&0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_cache_set_tuple(cache_ptr: i64, key_ptr: i64, val: i64) -> i64 {
    if cache_ptr == 0 {
        return 0;
    }
    let cache = unsafe { &mut *(cache_ptr as *mut HashMap<Vec<i64>, i64>) };
    let v = read_tuple(key_ptr);
    cache.insert(v, val);
    cache_ptr
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> i64 {
        olive_str_internal(text)
    }

    fn from_ptr(ptr: i64) -> String {
        olive_str_from_ptr(ptr)
    }

    #[test]
    fn str_trim() {
        assert_eq!(from_ptr(olive_str_trim(s("  hello  "))), "hello");
        assert_eq!(from_ptr(olive_str_trim(s("no spaces"))), "no spaces");
        assert_eq!(olive_str_trim(0), 0);
    }

    #[test]
    fn str_upper_lower() {
        assert_eq!(from_ptr(olive_str_upper(s("hello"))), "HELLO");
        assert_eq!(from_ptr(olive_str_lower(s("WORLD"))), "world");
    }

    #[test]
    fn str_replace() {
        assert_eq!(from_ptr(olive_str_replace(s("hello world"), s("world"), s("olive"))), "hello olive");
        assert_eq!(from_ptr(olive_str_replace(s("aaa"), s("a"), s("b"))), "bbb");
    }

    #[test]
    fn str_find() {
        assert_eq!(olive_str_find(s("hello"), s("ell")), 1);
        assert_eq!(olive_str_find(s("hello"), s("xyz")), -1);
        assert_eq!(olive_str_find(0, s("x")), -1);
    }

    #[test]
    fn str_contains() {
        assert_eq!(olive_str_contains(s("hello world"), s("world")), 1);
        assert_eq!(olive_str_contains(s("hello"), s("xyz")), 0);
    }

    #[test]
    fn str_starts_ends_with() {
        assert_eq!(olive_str_starts_with(s("hello"), s("hel")), 1);
        assert_eq!(olive_str_starts_with(s("hello"), s("llo")), 0);
        assert_eq!(olive_str_ends_with(s("hello"), s("llo")), 1);
        assert_eq!(olive_str_ends_with(s("hello"), s("hel")), 0);
    }

    #[test]
    fn str_repeat() {
        assert_eq!(from_ptr(olive_str_repeat(s("ab"), 3)), "ababab");
        assert_eq!(from_ptr(olive_str_repeat(s("x"), 0)), "");
    }

    #[test]
    fn str_split_by_sep() {
        let ptr = olive_str_split(s("a,b,c"), s(","));
        let list = unsafe { &*(ptr as *const StableVec) };
        assert_eq!(list.len, 3);
        assert_eq!(from_ptr(unsafe { *list.ptr }), "a");
        assert_eq!(from_ptr(unsafe { *list.ptr.add(1) }), "b");
        assert_eq!(from_ptr(unsafe { *list.ptr.add(2) }), "c");
    }

    #[test]
    fn str_split_whitespace() {
        let ptr = olive_str_split(s("foo bar baz"), 0);
        let list = unsafe { &*(ptr as *const StableVec) };
        assert_eq!(list.len, 3);
    }

    #[test]
    fn str_join() {
        let list_ptr = olive_str_split(s("a,b,c"), s(","));
        let joined = olive_str_join(list_ptr, s("-"));
        assert_eq!(from_ptr(joined), "a-b-c");
    }

    #[test]
    fn set_add_contains_o1() {
        let set = olive_set_new(4);
        olive_set_add(set, 10);
        olive_set_add(set, 20);
        olive_set_add(set, 10);
        let s = unsafe { &*(set as *const OliveHashSet) };
        assert_eq!(s.len, 2);
        assert_eq!(olive_in_list(10, set), 1);
        assert_eq!(olive_in_list(20, set), 1);
        assert_eq!(olive_in_list(99, set), 0);
    }

    #[test]
    fn set_len_via_list_len() {
        let set = olive_set_new(0);
        olive_set_add(set, 1);
        olive_set_add(set, 2);
        olive_set_add(set, 3);
        assert_eq!(olive_list_len(set), 3);
    }

    #[test]
    fn set_iteration_order_stable() {
        let set = olive_set_new(0);
        for i in [5i64, 3, 7, 1, 9] {
            olive_set_add(set, i);
        }
        assert_eq!(olive_list_len(set), 5);
        let sv = unsafe { &*(set as *const OliveHashSet) };
        let items: Vec<i64> = (0..sv.len).map(|i| unsafe { *sv.ptr.add(i) }).collect();
        assert!(items.contains(&5));
        assert!(items.contains(&1));
    }

    #[test]
    fn obj_keys_values() {
        let obj = olive_obj_new();
        olive_obj_set(obj, s("a"), 10);
        olive_obj_set(obj, s("b"), 20);
        let keys_ptr = olive_obj_keys(obj);
        let vals_ptr = olive_obj_values(obj);
        assert_eq!(olive_list_len(keys_ptr), 2);
        assert_eq!(olive_list_len(vals_ptr), 2);
    }

    #[test]
    fn time_format_epoch() {
        let result = from_ptr(olive_time_format(0.0, 0));
        assert_eq!(result, "1970-01-01T00:00:00");
    }

    #[test]
    fn time_format_known_date() {
        let result = from_ptr(olive_time_format(1705319445.0, 0));
        assert_eq!(result, "2024-01-15T11:50:45");
    }

    #[test]
    fn time_format_custom() {
        let fmt = s("%Y/%m/%d");
        let result = from_ptr(olive_time_format(1705319445.0, fmt));
        assert_eq!(result, "2024/01/15");
    }

    #[test]
    fn list_append_and_get() {
        let list = olive_list_new(0);
        olive_list_append(list, 42);
        olive_list_append(list, 99);
        assert_eq!(olive_list_len(list), 2);
        assert_eq!(olive_list_get(list, 0), 42);
        assert_eq!(olive_list_get(list, 1), 99);
    }

    #[test]
    fn obj_set_get() {
        let obj = olive_obj_new();
        olive_obj_set(obj, s("key"), 777);
        assert_eq!(olive_obj_get(obj, s("key")), 777);
        assert_eq!(olive_obj_get(obj, s("missing")), 0);
    }

    #[test]
    fn str_concat_and_eq() {
        let a = s("hello ");
        let b = s("world");
        let c = olive_str_concat(a, b);
        assert_eq!(from_ptr(c), "hello world");
        assert_eq!(olive_str_eq(c, s("hello world")), 1);
        assert_eq!(olive_str_eq(c, s("other")), 0);
    }

    #[test]
    fn str_len_and_slice() {
        let text = s("hello");
        assert_eq!(olive_str_len(text), 5);
        assert_eq!(from_ptr(olive_str_slice(text, 1, 4)), "ell");
    }

    #[test]
    fn time_now_positive() {
        assert!(olive_time_now() > 0.0);
    }

    #[test]
    fn str_fmt_basic() {
        let tmpl = s("hello {}!");
        let mut args_ptrs = vec![s("world")];
        let args = Box::into_raw(Box::new(StableVec {
            kind: KIND_LIST,
            ptr: args_ptrs.as_mut_ptr(),
            cap: args_ptrs.capacity(),
            len: args_ptrs.len(),
        })) as i64;
        std::mem::forget(args_ptrs);
        let result = from_ptr(olive_str_fmt(tmpl, args));
        assert_eq!(result, "hello world!");
    }

    #[test]
    fn str_fmt_multiple_args() {
        let tmpl = s("{} + {} = {}");
        let mut args_ptrs = vec![s("1"), s("2"), s("3")];
        let args = Box::into_raw(Box::new(StableVec {
            kind: KIND_LIST,
            ptr: args_ptrs.as_mut_ptr(),
            cap: args_ptrs.capacity(),
            len: args_ptrs.len(),
        })) as i64;
        std::mem::forget(args_ptrs);
        let result = from_ptr(olive_str_fmt(tmpl, args));
        assert_eq!(result, "1 + 2 = 3");
    }

    #[test]
    fn str_fmt_no_placeholders() {
        let tmpl = s("no placeholders");
        let result = from_ptr(olive_str_fmt(tmpl, 0));
        assert_eq!(result, "no placeholders");
    }

    #[test]
    fn str_char_count_ascii() {
        assert_eq!(olive_str_char_count(s("hello")), 5);
    }

    #[test]
    fn str_char_count_unicode() {
        let emoji = s("café");
        assert_eq!(olive_str_char_count(emoji), 4);
    }

    #[test]
    fn str_char_count_null() {
        assert_eq!(olive_str_char_count(0), 0);
    }
}
