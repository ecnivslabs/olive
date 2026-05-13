use rustc_hash::FxHashMap as HashMap;
#[allow(unused_imports)]
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub mod aio;
pub mod http;
pub mod io;
pub mod math;
pub mod net;
pub mod random;

const KIND_LIST: i64 = 1;
const KIND_OBJ: i64 = 2;
const KIND_ENUM: i64 = 3;
const KIND_SET: i64 = 4;

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
    pub tag: i64,
    pub payload_ptr: *mut i64,
    pub payload_len: usize,
}

// Memory Allocation
#[unsafe(no_mangle)]
pub extern "C" fn olive_alloc(size: i64) -> *mut u8 {
    let layout = std::alloc::Layout::from_size_align(size as usize, 8).unwrap();
    unsafe { std::alloc::alloc(layout) }
}

// Print functions
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

// Conversion
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
    olive_str_internal(&format!(
        "{}{}",
        olive_str_from_ptr(l),
        olive_str_from_ptr(r)
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_eq(l: i64, r: i64) -> i64 {
    if l == r {
        return 1;
    }
    if l == 0 || r == 0 {
        return 0;
    }
    if olive_str_from_ptr(l) == olive_str_from_ptr(r) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_copy(ptr: i64) -> i64 {
    olive_str_internal(&olive_str_from_ptr(ptr))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_copy_float(val: f64) -> f64 {
    val
}

// List operations
#[unsafe(no_mangle)]
pub extern "C" fn olive_list_new(len: i64) -> i64 {
    let mut v = vec![0i64; len as usize];
    let ptr = v.as_mut_ptr();
    let cap = v.capacity();
    let length = v.len();
    std::mem::forget(v);
    Box::into_raw(Box::new(StableVec {
        kind: KIND_LIST,
        ptr,
        cap,
        len: length,
    })) as i64
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
        let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
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
    let s = unsafe { &*(list_ptr as *const StableVec) };
    for i in 0..s.len {
        if unsafe { *s.ptr.add(i) } == val {
            return 1;
        }
    }
    0
}
// Set operations
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_new(len: i64) -> i64 {
    let mut v = Vec::with_capacity(len as usize);
    let ptr = v.as_mut_ptr();
    let cap = v.capacity();
    let length = v.len();
    std::mem::forget(v);
    Box::into_raw(Box::new(StableVec {
        kind: KIND_SET,
        ptr,
        cap,
        len: length,
    })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_add(set_ptr: i64, val: i64) {
    if set_ptr == 0 {
        return;
    }
    unsafe {
        let s = &mut *(set_ptr as *mut StableVec);
        // Simple list-based set: check for existence first
        for i in 0..s.len {
            if *s.ptr.add(i) == val {
                return;
            }
        }
        let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
        v.push(val);
        s.ptr = v.as_mut_ptr();
        s.cap = v.capacity();
        s.len = v.len();
        std::mem::forget(v);
    }
}

// Object operations
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
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    m.fields.insert(olive_str_from_ptr(attr), val);
    obj_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get(obj_ptr: i64, attr: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        return 0;
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    *m.fields.get(&olive_str_from_ptr(attr)).unwrap_or(&0)
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

// Free memory
#[unsafe(no_mangle)]
pub extern "C" fn olive_free_str(ptr: i64) {
    if ptr != 0 && (ptr & 1) == 0 {
        unsafe {
            let _ = std::ffi::CString::from_raw(ptr as *mut i8);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_list(ptr: i64) {
    if ptr != 0 {
        unsafe {
            let s = Box::from_raw(ptr as *mut StableVec);
            if !s.ptr.is_null() {
                let _ = Vec::from_raw_parts(s.ptr, s.len, s.cap);
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

// Time functions
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

// Math helpers (non-module)
#[unsafe(no_mangle)]
pub extern "C" fn olive_pow(base: i64, exp: i64) -> i64 {
    base.pow(exp as u32)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_pow_float(base: f64, exp: f64) -> f64 {
    base.powf(exp)
}

// Internal helpers
// Enum operations
#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_new(tag: i64, arg_count: i64) -> i64 {
    let mut payload = vec![0i64; arg_count as usize];
    let payload_ptr = payload.as_mut_ptr();
    let payload_len = payload.len();
    std::mem::forget(payload);
    Box::into_raw(Box::new(OliveEnum {
        kind: KIND_ENUM,
        tag,
        payload_ptr,
        payload_len,
    })) as i64
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

// Iterator operations
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

// String indexing
#[unsafe(no_mangle)]
pub extern "C" fn olive_str_len(s: i64) -> i64 {
    olive_str_from_ptr(s).len() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_get(s: i64, i: i64) -> i64 {
    let text = olive_str_from_ptr(s);
    if let Some(c) = text.chars().nth(i as usize) {
        olive_str_internal(&c.to_string())
    } else {
        0
    }
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

// Internal helpers
pub fn olive_str_internal(s: &str) -> i64 {
    let c_str = std::ffi::CString::new(s).unwrap();
    c_str.into_raw() as i64 | 1
}

pub fn olive_str_from_ptr(ptr: i64) -> String {
    if ptr == 0 {
        return String::new();
    }
    let p = ptr & !1;
    unsafe {
        std::ffi::CStr::from_ptr(p as *const i8)
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
        KIND_LIST | KIND_SET => olive_free_list(ptr),
        KIND_OBJ => olive_free_obj(ptr),
        KIND_ENUM => olive_free_enum(ptr),
        _ => {}
    }
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

// Tuple cache helpers
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
