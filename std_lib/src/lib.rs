pub mod enum_obj;
pub mod list;
pub mod obj;
pub mod set;
pub mod string;
pub mod struct_obj;
pub mod time;

pub use enum_obj::*;
pub use list::*;
pub use obj::*;
pub use set::*;
pub use string::*;
pub use struct_obj::*;
pub use time::*;

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use rustc_hash::{FxHashMap as HashMap, FxHashSet};
extern crate libc;
use std::sync::{Mutex, OnceLock};

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
pub mod python;
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
pub(crate) const KIND_PYOBJECT: i64 = 7;

const SHARDS: usize = 16;
static ACTIVE_OBJECTS: OnceLock<[std::sync::RwLock<FxHashSet<i64>>; SHARDS]> = OnceLock::new();

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

static IS_MULTITHREADED: AtomicBool = AtomicBool::new(false);
static MAIN_THREAD_ID: OnceLock<std::thread::ThreadId> = OnceLock::new();

#[inline]
fn check_multithreaded() -> bool {
    if IS_MULTITHREADED.load(Ordering::Relaxed) {
        return true;
    }
    let main_id = MAIN_THREAD_ID.get_or_init(|| std::thread::current().id());
    if std::thread::current().id() != *main_id {
        IS_MULTITHREADED.store(true, Ordering::Relaxed);
        true
    } else {
        false
    }
}

thread_local! {
    static LOCAL_ACTIVE_OBJECTS: RefCell<FxHashSet<i64>> = RefCell::new(FxHashSet::default());
}

#[inline]
fn get_shard(ptr: i64) -> usize {
    (ptr as usize >> 4) % SHARDS
}

pub fn register_object(ptr: i64) {
    if ptr != 0 {
        LOCAL_ACTIVE_OBJECTS.with(|cache| cache.borrow_mut().insert(ptr));
        if check_multithreaded() {
            let shards = ACTIVE_OBJECTS.get_or_init(|| {
                std::array::from_fn(|_| std::sync::RwLock::new(FxHashSet::default()))
            });
            shards[get_shard(ptr)].write().unwrap().insert(ptr);
        }
    }
}

pub fn unregister_object(ptr: i64) {
    if ptr != 0 {
        LOCAL_ACTIVE_OBJECTS.with(|cache| cache.borrow_mut().remove(&ptr));
        if check_multithreaded() {
            if let Some(shards) = ACTIVE_OBJECTS.get() {
                shards[get_shard(ptr)].write().unwrap().remove(&ptr);
            }
        }
    }
}

pub fn is_active_object(ptr: i64) -> bool {
    if ptr == 0 {
        return false;
    }

    // Fast path: Check thread-local cache first
    let in_local = LOCAL_ACTIVE_OBJECTS.with(|cache| cache.borrow().contains(&ptr));
    if in_local {
        return true;
    }

    // Slow path: Check global shards (only if multithreaded)
    if check_multithreaded() {
        if let Some(shards) = ACTIVE_OBJECTS.get() {
            shards[get_shard(ptr)].read().unwrap().contains(&ptr)
        } else {
            false
        }
    } else {
        false
    }
}

pub fn active_objects_count() -> usize {
    if check_multithreaded() {
        if let Some(shards) = ACTIVE_OBJECTS.get() {
            shards.iter().map(|shard| shard.read().unwrap().len()).sum()
        } else {
            0
        }
    } else {
        LOCAL_ACTIVE_OBJECTS.with(|cache| cache.borrow().len())
    }
}

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
pub extern "C" fn olive_ffi_errno() -> i64 {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        *libc::__errno_location() as i64
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    unsafe {
        *libc::__error() as i64
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    )))]
    0
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
    use libffi::middle::{Cif, CodePtr, Type, arg};
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
    println!("{}", format_list(ptr));
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print_list_float(ptr: i64) -> i64 {
    if ptr == 0 {
        println!("[]");
        return 0;
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    let parts: Vec<String> = (0..v.len)
        .map(|i| {
            let bits = unsafe { *v.ptr.add(i) };
            format!("{}", f64::from_bits(bits as u64))
        })
        .collect();
    println!("[{}]", parts.join(", "));
    0
}

fn format_list(ptr: i64) -> String {
    if ptr == 0 {
        return "[]".to_string();
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    let mut parts = Vec::with_capacity(v.len);
    for i in 0..v.len {
        let elem = unsafe { *v.ptr.add(i) };
        parts.push(format_list_elem(elem));
    }
    format!("[{}]", parts.join(", "))
}

fn format_list_elem(val: i64) -> String {
    // Olive strings are tagged with LSB = 1
    if val & 1 == 1 {
        let untagged = val & !1;
        if untagged != 0 {
            return format!("\"{}\"", olive_str_from_ptr(val));
        }
    }
    // Nested list: val is a valid live Olive heap object
    if is_active_object(val) {
        return format_list(val);
    }
    // Plain integer
    format!("{}", val)
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
    let s = olive_str_from_ptr(ptr);
    s.trim().parse::<i64>().unwrap_or_else(|_| {
        let msg = olive_str_internal(&format!("int() argument must be an integer, got '{}'", s));
        olive_panic(msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str_to_float(ptr: i64) -> f64 {
    let s = olive_str_from_ptr(ptr);
    s.trim().parse::<f64>().unwrap_or_else(|_| {
        let msg = olive_str_internal(&format!("float() argument must be a number, got '{}'", s));
        olive_panic(msg);
        #[allow(unreachable_code)]
        0.0
    })
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
pub extern "C" fn olive_in_list(val: i64, list_ptr: i64) -> i64 {
    if list_ptr == 0 {
        return 0;
    }
    let kind = unsafe { *(list_ptr as *const i64) };
    if kind == KIND_SET {
        let s = unsafe { &*(list_ptr as *const OliveHashSet) };
        return if unsafe { (*s.inner).contains(&val) } {
            1
        } else {
            0
        };
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
pub extern "C" fn olive_free_str(ptr: i64) {
    if ptr != 0 && (ptr & 1) == 0 {
        unsafe {
            let _ = std::ffi::CString::from_raw(ptr as *mut std::ffi::c_char);
        }
    }
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
        KIND_PYOBJECT => {
            let key_obj = if index & 1 != 0 {
                python::olive_py_from_str(index)
            } else {
                python::olive_py_from_int(index)
            };
            let py_res = python::olive_py_getitem(obj as *mut std::ffi::c_void, key_obj);
            python::olive_py_decref(key_obj);
            let olive_res = python::olive_py_conv_to_olive(py_res);
            python::olive_py_decref(py_res);
            olive_res
        }
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
        KIND_PYOBJECT => {
            let key_obj = if index & 1 != 0 {
                python::olive_py_from_str(index)
            } else {
                python::olive_py_from_int(index)
            };
            let py_val = python::olive_py_conv_to_py(val);
            python::olive_py_setitem(obj as *mut std::ffi::c_void, key_obj, py_val);
            python::olive_py_decref(key_obj);
            python::olive_py_decref(py_val);
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
        crate::result::KIND_RESULT => crate::result::olive_free_result(ptr),
        KIND_PYOBJECT => python::olive_py_decref(ptr as *mut std::os::raw::c_void),
        _ => {}
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_panic(msg: i64) -> i64 {
    let text = if msg == 0 {
        "panic".to_string()
    } else {
        olive_str_from_ptr(msg)
    };
    if let Some(hooks) = EXIT_HOOKS.get()
        && let Ok(list) = hooks.lock()
    {
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
                Some(x) => {
                    out.push('%');
                    out.push(x);
                }
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
        assert_eq!(
            from_ptr(olive_str_replace(s("hello world"), s("world"), s("olive"))),
            "hello olive"
        );
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
pub mod python_proxy;
