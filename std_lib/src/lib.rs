#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::manual_c_str_literals)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::missing_transmute_annotations)]
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

use rustc_hash::{FxHashMap as HashMap, FxHashSet};
extern crate libc;
use std::sync::{Mutex, OnceLock};

pub mod aio;
pub mod boxed;
pub mod bytes;
pub mod compress;
pub mod crypto;
pub mod datetime;
pub mod encoding;
pub mod format;
pub mod io;
pub mod json;
pub mod logging;
pub mod math;
pub mod net;
pub mod os;
pub mod panic;
pub mod python;
pub mod random;
pub mod regex;
pub mod requests;
pub mod result;
pub mod sys;
pub mod sys_signal;
mod tracking;
pub mod uuid;
pub mod websocket;
pub mod yaml;

pub(crate) const KIND_LIST: i64 = 1;
// List whose elements are inline Any-tagged (TAG_INT/BOOL/NULL or heap ptr); list proxy
// uses olive_any_to_py for elements instead of the raw olive_to_py path.
pub(crate) const KIND_ANY_LIST: i64 = 15;
pub(crate) const KIND_OBJ: i64 = 2;
pub(crate) const KIND_ENUM: i64 = 3;
pub(crate) const KIND_SET: i64 = 4;
pub(crate) const KIND_BYTES: i64 = 6;
pub(crate) const KIND_PYOBJECT: i64 = 7;
pub(crate) const KIND_ITER: i64 = 8;
pub(crate) const KIND_FLOAT: i64 = 11;
pub(crate) const KIND_BOOL: i64 = 12;
pub(crate) const KIND_NULL: i64 = 13;
pub(crate) const KIND_INT: i64 = 14;

pub use tracking::{active_objects_count, is_active_object, register_object, unregister_object};

#[repr(C)]
pub struct StableVec {
    pub kind: i64,
    pub ptr: *mut i64,
    pub cap: usize,
    pub len: usize,
}

#[derive(Clone, Copy)]
pub struct OliveStringKey(pub i64);

/// How a dict/set key word is identified. A scalar is hashed and compared by
/// its value so that the same key reaches the same entry whether it arrives raw
/// (a bare int from a literal) or boxed (an `Any`-typed variable carrying a
/// `KIND_INT` box); a boxed scalar is never matched by its heap address.
enum KeyClass {
    Str(&'static str),
    Scalar(i64, i64),
    Raw(i64),
}

fn classify_key(v: i64) -> KeyClass {
    // Inline immediates are checked before the string heuristic so a large odd
    // integer is never misread as a tagged pointer.
    match v & boxed::TAG_MASK {
        boxed::TAG_INT => return KeyClass::Scalar(KIND_INT, v >> 3),
        boxed::TAG_BOOL => return KeyClass::Scalar(KIND_BOOL, v >> 3),
        boxed::TAG_NULL => return KeyClass::Scalar(KIND_NULL, 0),
        _ => {}
    }
    if v & 1 == 1 && (v & !1) > 0x10000 {
        return KeyClass::Str(olive_str_as_str(v).unwrap_or(""));
    }
    if is_active_object(v) {
        let kind = unsafe { *(v as *const i64) };
        if matches!(kind, KIND_INT | KIND_FLOAT) {
            // A heap-boxed (out-of-range) int collapses to the same class as an
            // inline int of equal value, so equal keys hash and compare alike.
            let b = unsafe { &*(v as *const boxed::OliveBoxed) };
            return KeyClass::Scalar(kind, b.bits);
        }
        return KeyClass::Raw(v);
    }
    // A bare non-pointer word is a raw integer (a concrete int key or `0`);
    // normalize it to the int class so it matches its inline form.
    KeyClass::Scalar(KIND_INT, v)
}

impl PartialEq for OliveStringKey {
    fn eq(&self, other: &Self) -> bool {
        if self.0 == other.0 {
            return true;
        }
        match (classify_key(self.0), classify_key(other.0)) {
            (KeyClass::Str(a), KeyClass::Str(b)) => a == b,
            (KeyClass::Scalar(ka, va), KeyClass::Scalar(kb, vb)) => ka == kb && va == vb,
            _ => false,
        }
    }
}
impl Eq for OliveStringKey {}
impl std::hash::Hash for OliveStringKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match classify_key(self.0) {
            KeyClass::Str(s) => s.hash(state),
            KeyClass::Scalar(kind, bits) => {
                kind.hash(state);
                bits.hash(state);
            }
            KeyClass::Raw(v) => v.hash(state),
        }
    }
}

#[repr(C)]
pub struct OliveObj {
    pub kind: i64,
    pub fields: HashMap<OliveStringKey, i64>,
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

/// Address of the thread-local `errno`, or null on platforms without one.
fn errno_location() -> *mut i32 {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        libc::__errno_location()
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
        libc::__error()
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
    std::ptr::null_mut()
}

thread_local! {
    /// `errno` captured the instant the most recent FFI call returned. Reading
    /// `errno` directly is unsafe in Olive because the runtime may allocate
    /// (boxing a result, building a string) between the FFI call and the user's
    /// `ffi_errno()` read, and `malloc` clobbers `errno`. The compiler emits a
    /// snapshot immediately after every FFI call so the value is preserved.
    static FFI_ERRNO_SNAPSHOT: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// Captures `errno` into the snapshot. Emitted by codegen right after a foreign
/// call, before any runtime allocation can overwrite `errno`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_ffi_snapshot_errno() {
    let loc = errno_location();
    if !loc.is_null() {
        let v = unsafe { *loc };
        FFI_ERRNO_SNAPSHOT.with(|c| c.set(v));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_ffi_errno() -> i64 {
    FFI_ERRNO_SNAPSHOT.with(|c| c.get()) as i64
}

/// Resets both the live `errno` and the snapshot to 0 so the value read after a
/// failing FFI call is known to belong to that call and not a stale value.
#[unsafe(no_mangle)]
pub extern "C" fn olive_ffi_clear_errno() {
    let loc = errno_location();
    if !loc.is_null() {
        unsafe { *loc = 0 };
    }
    FFI_ERRNO_SNAPSHOT.with(|c| c.set(0));
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_ffi_errmsg(fn_name: i64, errno: i64) -> i64 {
    let name = crate::olive_str_from_ptr(fn_name);
    let msg = if errno == 0 {
        format!("{name}: call failed")
    } else {
        let desc = std::io::Error::from_raw_os_error(errno as i32).to_string();
        format!("{name}: {desc}")
    };
    crate::olive_str_internal(&msg)
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
    let cif = Cif::new_variadic(types, nf, Type::i64());
    let vals: Vec<i64> = (0..n).map(|i| unsafe { *arg_vals.add(i) }).collect();
    let ffi_args: Vec<_> = vals.iter().map(|v| arg(v)).collect();
    unsafe { cif.call::<i64>(CodePtr(fn_ptr as *mut _), &ffi_args) }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print_bool(val: i64) -> i64 {
    if val == 0 {
        println!("False");
    } else {
        println!("True");
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print(val: i64) -> i64 {
    println!("{}", val);
    0
}

/// Formats a float the way Python's `repr` does: a finite value with no
/// fractional or exponent part still shows a trailing `.0` so it reads as a
/// float, not an int.
pub(crate) fn fmt_float(val: f64) -> String {
    if val.is_nan() {
        return "nan".to_string();
    }
    if val.is_infinite() {
        return if val < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    let s = format!("{val}");
    if s.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        format!("{s}.0")
    } else {
        s
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print_float(val: f64) -> i64 {
    println!("{}", fmt_float(val));
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
pub extern "C" fn olive_print_py(val: i64) -> i64 {
    if val == 0 {
        println!("None");
        return 0;
    }
    let str_ptr = python::olive_py_to_str(val as python::PyObject);
    if str_ptr != 0 {
        println!("{}", olive_str_from_ptr(str_ptr));
        olive_free_str(str_ptr);
    } else {
        println!("<PyObject>");
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
            fmt_float(f64::from_bits(bits as u64))
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

fn looks_like_float(val: i64) -> bool {
    let f = f64::from_bits(val as u64);
    if f.is_nan() || f.is_infinite() || f.is_subnormal() {
        return false;
    }
    let abs_f = f.abs();
    abs_f > 1e-100 && abs_f < 1e100
}

pub(crate) fn format_list_elem(val: i64) -> String {
    match val & boxed::TAG_MASK {
        boxed::TAG_INT => return format!("{}", val >> 3),
        boxed::TAG_BOOL => return if val >> 3 != 0 { "True" } else { "False" }.to_string(),
        boxed::TAG_NULL => return "None".to_string(),
        _ => {}
    }
    if val & 1 == 1 {
        let untagged = val & !1;
        if untagged > 0x10000 {
            return format!("\"{}\"", olive_str_from_ptr(val));
        }
    }
    if is_active_object(val) {
        let kind = unsafe { *(val as *const i64) };
        match kind {
            KIND_FLOAT => {
                let b = unsafe { &*(val as *const boxed::OliveBoxed) };
                return fmt_float(f64::from_bits(b.bits as u64));
            }
            KIND_INT => {
                let b = unsafe { &*(val as *const boxed::OliveBoxed) };
                return format!("{}", b.bits);
            }
            KIND_LIST => return format_list(val),
            KIND_OBJ => {
                let m = unsafe { &*(val as *const OliveObj) };
                let mut parts = Vec::with_capacity(m.fields.len());
                for (k, &v) in &m.fields {
                    let k_str = olive_str_as_str(k.0).unwrap_or("");
                    parts.push(format!("'{}': {}", k_str, format_list_elem(v)));
                }
                return format!("{{{}}}", parts.join(", "));
            }
            KIND_PYOBJECT => {
                let str_ptr = python::olive_py_to_str(val as python::PyObject);
                if str_ptr != 0 {
                    let s = olive_str_from_ptr(str_ptr);
                    olive_free_str(str_ptr);
                    return s;
                }
                return "<PyObject>".to_string();
            }
            KIND_ENUM => {
                let e = unsafe { &*(val as *const OliveEnum) };
                let mut parts = Vec::with_capacity(e.payload_len);
                for i in 0..e.payload_len {
                    let pval = unsafe { *e.payload_ptr.add(i) };
                    parts.push(format_list_elem(pval));
                }
                return format!("Enum(tag={}, payload=[{}])", e.tag, parts.join(", "));
            }
            crate::result::KIND_RESULT => {
                let res = unsafe { &*(val as *const crate::result::OliveResult) };
                if res.tag == 1 {
                    return format!("Ok({})", format_list_elem(res.payload));
                } else {
                    return format!("Err({})", format_list_elem(res.payload));
                }
            }
            _ => {}
        }
    }
    if looks_like_float(val) {
        fmt_float(f64::from_bits(val as u64))
    } else {
        format!("{}", val)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print_any(val: i64) -> i64 {
    println!("{}", format_list_elem(val));
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print_obj(ptr: i64) -> i64 {
    if ptr == 0 {
        println!("{{}}");
        return 0;
    }
    let kind = unsafe { *(ptr as *const i64) };
    if kind == crate::result::KIND_RESULT {
        let res = unsafe { &*(ptr as *const crate::result::OliveResult) };
        if res.tag == 1 {
            println!("Ok({})", format_list_elem(res.payload));
        } else {
            println!("Err({})", format_list_elem(res.payload));
        }
        return 0;
    }
    let m = unsafe { &*(ptr as *const OliveObj) };
    print!("{{");
    for (i, (k, &v)) in m.fields.iter().enumerate() {
        if i > 0 {
            print!(", ");
        }
        let k_str = olive_str_as_str(k.0).unwrap_or("");
        print!("'{}': {}", k_str, format_list_elem(v));
    }
    println!("}}");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_str(val: i64) -> i64 {
    olive_str_internal(&val.to_string())
}

/// `str()` of an `Any`: renders the value's content (boxed float/bool, string,
/// container), strings unquoted.
#[unsafe(no_mangle)]
pub extern "C" fn olive_none_to_str(_val: i64) -> i64 {
    olive_str_internal("None")
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_bool_to_str(val: i64) -> i64 {
    olive_str_internal(if val != 0 { "True" } else { "False" })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_any_to_str(val: i64) -> i64 {
    match val & boxed::TAG_MASK {
        boxed::TAG_INT => return olive_str_internal(&format!("{}", val >> 3)),
        boxed::TAG_BOOL => {
            return olive_str_internal(if val >> 3 != 0 { "True" } else { "False" });
        }
        boxed::TAG_NULL => return olive_str_internal("None"),
        _ => {}
    }
    if val & 1 == 1 && (val & !1) > 0x10000 {
        return olive_str_internal(&olive_str_from_ptr(val));
    }
    if is_active_object(val) {
        let kind = unsafe { *(val as *const i64) };
        match kind {
            KIND_FLOAT => {
                let b = unsafe { &*(val as *const boxed::OliveBoxed) };
                return olive_str_internal(&fmt_float(f64::from_bits(b.bits as u64)));
            }
            KIND_INT => {
                let b = unsafe { &*(val as *const boxed::OliveBoxed) };
                return olive_str_internal(&format!("{}", b.bits));
            }
            KIND_PYOBJECT => {
                let p = python::olive_py_to_str(val as python::PyObject);
                return if p != 0 {
                    p
                } else {
                    olive_str_internal("<PyObject>")
                };
            }
            KIND_LIST | KIND_OBJ | KIND_ENUM | KIND_SET => {
                return olive_str_internal(&format_list_elem(val));
            }
            _ => {}
        }
    }
    olive_str_internal(&val.to_string())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_int(val: i64) -> i64 {
    if val & 1 == 1 {
        let untagged = val & !1;
        if untagged > 0x10000 {
            return olive_str_to_int(val);
        }
    }
    if is_active_object(val) {
        let kind = unsafe { *(val as *const i64) };
        if kind == KIND_PYOBJECT {
            return python::olive_py_to_int(val as python::PyObject);
        }
    }
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
    olive_str_internal(&fmt_float(val))
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

/// `+` on operands whose static type is `Any`: dispatch on the runtime kind so
/// strings concatenate, floats and ints add, and lists join, rather than blindly
/// adding the two words as integers. Every scalar reaching here is boxed (the
/// compiler boxes a concrete operand before an `Any` op), so a bare low-bit
/// pointer is unambiguously a string. The integer result is reboxed because the
/// expression's static type is `Any`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_any_add(a: i64, b: i64) -> i64 {
    if any_is_str(a) || any_is_str(b) {
        return olive_str_concat(a, b);
    }
    if any_is_float(a) || any_is_float(b) {
        return boxed::olive_box_float(boxed::olive_unbox_float(a) + boxed::olive_unbox_float(b));
    }
    if any_is_list(a) && any_is_list(b) {
        return crate::list::olive_list_concat(a, b);
    }
    // wrapping_add, not `+`: plain `+` panics on overflow in debug builds.
    boxed::olive_box_int(boxed::olive_unbox_int(a).wrapping_add(boxed::olive_unbox_int(b)))
}

/// A call site's kind-history byte, one of:
/// - `0..ANY_SITE_SAMPLE_WINDOW`: still sampling, N calls observed so far, all
///   two plain (non-str/float/list) ints -- keep recording.
/// - `ANY_SITE_SAMPLE_WINDOW`: graduated all-int -- `ANY_SITE_SAMPLE_WINDOW`
///   consecutive calls were all plain ints, stop touching this cell.
/// - `ANY_SITE_MIXED`: at least one call had a non-int operand -- terminal,
///   never specializable.
///
/// Recording unconditionally on every call measured at ~58% overhead on an
/// Any-add-only microbenchmark (3M-iteration loop, release build) -- pure cost
/// for a call this project's whole point is to make cheap. A fixed sample
/// window bounds that cost to a handful of calls per site instead of the
/// entire program lifetime (the same tradeoff inline-cache/feedback-vector
/// designs make: a few observations are enough to trust monomorphic
/// behavior, further verification isn't worth its own cost).
// Swept 4/8/32 on a monomorphic no-retier workload; 8 measured fastest in
// both runs (see benchmark/results/tier_sweep.md) though the effect size
// doesn't cleanly attribute to the recording mechanism itself -- kept as
// the empirically-best of the tested candidates either way.
const ANY_SITE_SAMPLE_WINDOW: u8 = 8;
const ANY_SITE_MIXED: u8 = 254;

fn is_plain_int_any(v: i64) -> bool {
    !any_is_str(v) && !any_is_float(v) && !any_is_list(v)
}

/// Delegates to the underlying dispatcher, plus recording whether both
/// operands were plain ints into `site_history` for specialization.
macro_rules! any_profiled {
    ($name:ident, $inner:ident) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(a: i64, b: i64, site_history: *mut u8) -> i64 {
            if !site_history.is_null() {
                let cur = unsafe { *site_history };
                if cur < ANY_SITE_SAMPLE_WINDOW {
                    let both_int = is_plain_int_any(a) && is_plain_int_any(b);
                    let next = if both_int { cur + 1 } else { ANY_SITE_MIXED };
                    unsafe {
                        *site_history = next;
                    }
                }
            }
            $inner(a, b)
        }
    };
}
any_profiled!(olive_any_add_profiled, olive_any_add);

fn any_is_str(v: i64) -> bool {
    v & 1 == 1 && (v & !1) > 0x10000
}

fn any_is_float(v: i64) -> bool {
    is_active_object(v) && unsafe { *(v as *const i64) } == KIND_FLOAT
}

fn any_is_list(v: i64) -> bool {
    is_active_object(v) && unsafe { *(v as *const i64) } == KIND_LIST
}

/// Arithmetic on `Any` operands: a float on either side promotes to float,
/// otherwise both are integers and the result is reboxed as an `Any` int.
/// Wrapping on the int path -- plain `-`/`*` panic on overflow in debug builds.
macro_rules! any_arith {
    ($name:ident, $op:tt, $wrapping:ident) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(a: i64, b: i64) -> i64 {
            if any_is_float(a) || any_is_float(b) {
                boxed::olive_box_float(boxed::olive_unbox_float(a) $op boxed::olive_unbox_float(b))
            } else {
                boxed::olive_box_int(boxed::olive_unbox_int(a).$wrapping(boxed::olive_unbox_int(b)))
            }
        }
    };
}
any_arith!(olive_any_sub, -, wrapping_sub);
any_arith!(olive_any_mul, *, wrapping_mul);
any_profiled!(olive_any_sub_profiled, olive_any_sub);
any_profiled!(olive_any_mul_profiled, olive_any_mul);

#[unsafe(no_mangle)]
pub extern "C" fn olive_any_div(a: i64, b: i64) -> i64 {
    if any_is_float(a) || any_is_float(b) {
        boxed::olive_box_float(boxed::olive_unbox_float(a) / boxed::olive_unbox_float(b))
    } else {
        let d = boxed::olive_unbox_int(b);
        boxed::olive_box_int(if d == 0 {
            0
        } else {
            // i64::MIN / -1 is a hardware trap (SIGFPE); wrapping_div avoids it.
            boxed::olive_unbox_int(a).wrapping_div(d)
        })
    }
}
any_profiled!(olive_any_div_profiled, olive_any_div);

#[unsafe(no_mangle)]
pub extern "C" fn olive_any_mod(a: i64, b: i64) -> i64 {
    if any_is_float(a) || any_is_float(b) {
        boxed::olive_box_float(boxed::olive_unbox_float(a) % boxed::olive_unbox_float(b))
    } else {
        let d = boxed::olive_unbox_int(b);
        boxed::olive_box_int(if d == 0 {
            0
        } else {
            // Same i64::MIN / -1 trap as olive_any_div.
            boxed::olive_unbox_int(a).wrapping_rem(d)
        })
    }
}
any_profiled!(olive_any_mod_profiled, olive_any_mod);

/// Comparison on `Any` operands: two strings compare lexicographically, a float
/// on either side compares as float, otherwise as integers. The boolean result
/// is a concrete `Bool`, so it stays a bare word.
macro_rules! any_cmp {
    ($name:ident, $op:tt) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(a: i64, b: i64) -> i64 {
            let r = if any_is_str(a) && any_is_str(b) {
                olive_str_from_ptr(a) $op olive_str_from_ptr(b)
            } else if any_is_float(a) || any_is_float(b) {
                boxed::olive_unbox_float(a) $op boxed::olive_unbox_float(b)
            } else {
                boxed::olive_unbox_int(a) $op boxed::olive_unbox_int(b)
            };
            r as i64
        }
    };
}
any_cmp!(olive_any_lt, <);
any_cmp!(olive_any_le, <=);
any_cmp!(olive_any_gt, >);
any_cmp!(olive_any_ge, >=);
any_cmp!(olive_any_eq, ==);
any_cmp!(olive_any_ne, !=);
any_profiled!(olive_any_lt_profiled, olive_any_lt);
any_profiled!(olive_any_le_profiled, olive_any_le);
any_profiled!(olive_any_gt_profiled, olive_any_gt);
any_profiled!(olive_any_ge_profiled, olive_any_ge);
any_profiled!(olive_any_eq_profiled, olive_any_eq);
any_profiled!(olive_any_ne_profiled, olive_any_ne);

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
pub extern "C" fn olive_get_index_any(obj: i64, index: i64, loc: i64) -> i64 {
    if obj == 0 {
        panic::olive_nil_index_fail(loc);
    }
    if obj & 1 != 0 {
        return string::olive_str_get_checked(obj, index, loc);
    }
    let kind = unsafe { *(obj as *const i64) };
    match kind {
        KIND_LIST => {
            let len = olive_list_len(obj);
            if index < 0 || index >= len {
                panic::olive_bounds_fail(index, len, loc);
            }
            olive_list_get(obj, index)
        }
        KIND_OBJ => olive_obj_get(obj, index),
        KIND_ENUM => olive_enum_get(obj, index),
        KIND_BYTES => {
            let len = bytes::olive_buf_len(obj);
            if index < 0 || index >= len {
                panic::olive_bounds_fail(index, len, loc);
            }
            bytes::olive_buf_get(obj, index)
        }
        KIND_PYOBJECT => {
            let key_obj = if index > 0x10000 && index & 1 != 0 {
                python::olive_py_from_str(index)
            } else {
                python::olive_py_from_int(index)
            };
            let py_res = python::olive_py_getitem(obj as *mut std::ffi::c_void, key_obj);
            python::olive_py_decref(key_obj);
            // getitem returns a wrapped arena handle; unwrap before converting (py_to_olive reads ob_type).
            let raw_res = unsafe { python::olive_py_unwrap(py_res) };
            let olive_res = python::olive_py_conv_to_olive(raw_res);
            python::olive_py_decref(py_res);
            olive_res
        }
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_index_any(obj: i64, index: i64, val: i64, loc: i64) {
    if obj == 0 {
        panic::olive_nil_index_fail(loc);
    }
    if obj & 1 != 0 {
        return;
    }
    let kind = unsafe { *(obj as *const i64) };
    match kind {
        KIND_LIST => {
            let len = olive_list_len(obj);
            if index < 0 || index >= len {
                panic::olive_bounds_fail(index, len, loc);
            }
            olive_list_set(obj, index, val)
        }
        KIND_BYTES => {
            let len = bytes::olive_buf_len(obj);
            if index < 0 || index >= len {
                panic::olive_bounds_fail(index, len, loc);
            }
            bytes::olive_buf_set(obj, index, val)
        }
        KIND_OBJ => {
            olive_obj_set(obj, index, val);
        }
        KIND_PYOBJECT => {
            let key_obj = if index > 0x10000 && index & 1 != 0 {
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
    // An inline immediate (`TAG_INT`/`TAG_BOOL`/`TAG_NULL`) owns nothing; only
    // 8-aligned heap pointers carry a kind header.
    if ptr & boxed::TAG_MASK != 0 {
        return;
    }
    if ptr < 0x1000 {
        return;
    }
    let kind = unsafe { *(ptr as *const i64) };
    match kind {
        KIND_LIST => olive_free_list(ptr),
        KIND_SET => olive_free_set(ptr),
        KIND_OBJ => olive_free_obj(ptr),
        KIND_ENUM => olive_free_enum(ptr),
        KIND_BYTES => bytes::olive_buf_free(ptr),
        KIND_FLOAT | KIND_INT => boxed::olive_free_boxed(ptr),
        crate::result::KIND_RESULT => crate::result::olive_free_result(ptr),
        KIND_PYOBJECT => python::olive_py_decref(ptr as *mut std::os::raw::c_void),
        KIND_ITER => olive_free_iter(ptr),
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
    panic::abort(&text, None)
}

/// Runs every registered `atexit` hook once, in registration order. Shared by
/// the normal exit path and every panic path so resources are released before
/// the process tears down.
pub(crate) fn run_exit_hooks() {
    if let Some(hooks) = EXIT_HOOKS.get()
        && let Ok(list) = hooks.lock()
    {
        for &fn_ptr in list.iter() {
            let f: extern "C" fn() = unsafe { std::mem::transmute(fn_ptr as usize) };
            f();
        }
    }
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
    if val > 0x10000 && (val & 1) != 0 {
        1
    } else {
        0
    }
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
    match val & boxed::TAG_MASK {
        boxed::TAG_INT => return olive_str_internal("int"),
        boxed::TAG_BOOL => return olive_str_internal("bool"),
        boxed::TAG_NULL => return olive_str_internal("None"),
        _ => {}
    }
    // A bare `0` is int; an inline `null` is handled above.
    if val > 0x10000 && (val & 1) != 0 {
        return olive_str_internal("str");
    }
    if !is_active_object(val) {
        return olive_str_internal("int");
    }
    let kind = unsafe { *(val as *const i64) };
    let name = match kind {
        KIND_LIST => "list",
        KIND_OBJ => "dict",
        KIND_ENUM => "enum",
        KIND_SET => "set",
        KIND_BYTES => "bytes",
        KIND_FLOAT => "float",
        KIND_PYOBJECT => "PyObject",
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

pub mod python_proxy;
#[cfg(test)]
mod tests;
