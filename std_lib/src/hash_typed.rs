//! Descriptor-driven structural hash for struct/enum dict/set keys, paired
//! with `eq_typed` so a key hashes and compares by the same rule `==`
//! derives. `classify_key` (in `lib.rs`) has no per-call type context --
//! dict/set lookups reach it through the plain `std::hash::Hash`/`PartialEq`
//! traits, called generically by `HashMap`/`HashSet` with no descriptor
//! parameter to pass through. The bridge is a thread-local "active key
//! descriptor": a `_typed` entry point sets it, delegates to the existing
//! untyped dict/set op (so the fast str/int/float key path is untouched),
//! and clears it. `classify_key`'s `Raw` case (a live struct/enum pointer)
//! checks the thread-local and, if set, hashes/compares structurally
//! instead of by pointer identity.
//!
//! Sets and dicts have no fixed element order, so their contribution to a
//! containing hash must be commutative (XOR of each element's own hash, not
//! a sequential mix) -- otherwise `{1, 2}` and `{2, 1}` would hash
//! differently despite `==` (correctly) calling them equal.

use crate::format::{
    D_ANY, D_BACKREF, D_BYTES, D_DICT, D_ENUM, D_LIST, D_SET, D_STR, D_STRUCT, D_TUPLE, byte, skip,
};
use crate::slab::slot_is_live;
use crate::{OliveEnum, OliveHashSet, OliveObj, StableVec};
use rustc_hash::{FxHashSet, FxHasher};
use std::cell::Cell;
use std::hash::Hasher;

thread_local! {
    /// Descriptor byte-pointer for the dict/set key type currently being
    /// hashed or compared, or 0 when no typed key operation is in flight.
    static ACTIVE_KEY_DESC: Cell<i64> = const { Cell::new(0) };
}

pub(crate) fn active_key_descriptor() -> i64 {
    ACTIVE_KEY_DESC.with(|d| d.get())
}

fn with_key_descriptor<R>(desc: i64, f: impl FnOnce() -> R) -> R {
    let prev = ACTIVE_KEY_DESC.with(|d| d.replace(desc));
    let r = f();
    ACTIVE_KEY_DESC.with(|d| d.set(prev));
    r
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_set_typed(obj_ptr: i64, attr: i64, val: i64, key_desc: i64) -> i64 {
    with_key_descriptor(key_desc, || crate::obj::olive_obj_set(obj_ptr, attr, val))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get_typed(obj_ptr: i64, attr: i64, key_desc: i64) -> i64 {
    with_key_descriptor(key_desc, || crate::obj::olive_obj_get(obj_ptr, attr))
}

/// The `d[k]` indexing path (faults on a missing key), not the non-faulting
/// `.get()` method (`olive_obj_get_typed` above).
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get_checked_typed(
    obj_ptr: i64,
    attr: i64,
    loc: i64,
    key_desc: i64,
) -> i64 {
    with_key_descriptor(key_desc, || {
        crate::obj::olive_obj_get_checked(obj_ptr, attr, loc)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get_default_typed(
    obj_ptr: i64,
    attr: i64,
    default: i64,
    key_desc: i64,
) -> i64 {
    with_key_descriptor(key_desc, || {
        crate::obj::olive_obj_get_default(obj_ptr, attr, default)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_add_typed(set_ptr: i64, val: i64, key_desc: i64) {
    with_key_descriptor(key_desc, || crate::set::olive_set_add(set_ptr, val))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_contains_typed(set_ptr: i64, val: i64, key_desc: i64) -> i64 {
    with_key_descriptor(key_desc, || crate::set::olive_set_contains(set_ptr, val))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_remove_typed(set_ptr: i64, val: i64, key_desc: i64) -> i64 {
    with_key_descriptor(key_desc, || crate::set::olive_set_remove(set_ptr, val))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_remove_checked_typed(
    set_ptr: i64,
    val: i64,
    loc: i64,
    key_desc: i64,
) -> i64 {
    with_key_descriptor(key_desc, || {
        crate::set::olive_set_remove_checked(set_ptr, val, loc)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_pop_checked_typed(
    obj_ptr: i64,
    attr: i64,
    loc: i64,
    key_desc: i64,
) -> i64 {
    with_key_descriptor(key_desc, || {
        crate::obj::olive_obj_pop_checked(obj_ptr, attr, loc)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_pop_default_typed(
    obj_ptr: i64,
    attr: i64,
    default: i64,
    key_desc: i64,
) -> i64 {
    with_key_descriptor(key_desc, || {
        crate::obj::olive_obj_pop_default(obj_ptr, attr, default)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_setdefault_typed(
    obj_ptr: i64,
    attr: i64,
    default: i64,
    key_desc: i64,
) -> i64 {
    with_key_descriptor(key_desc, || {
        crate::obj::olive_obj_setdefault(obj_ptr, attr, default)
    })
}

/// `key in dict`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_in_obj_typed(key: i64, obj_ptr: i64, key_desc: i64) -> i64 {
    with_key_descriptor(key_desc, || crate::obj::olive_in_obj(key, obj_ptr))
}

/// `val in set`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_in_list_typed(val: i64, list_ptr: i64, key_desc: i64) -> i64 {
    with_key_descriptor(key_desc, || crate::olive_in_list(val, list_ptr))
}

/// Structural hash for a `Raw`-classified key (a live struct/enum pointer),
/// given the active key descriptor. Falls back to pointer identity when no
/// descriptor is active (an untyped `Any`-keyed container, unchanged from
/// before this existed).
pub(crate) fn hash_key(v: i64) -> u64 {
    let desc = active_key_descriptor();
    if desc == 0 {
        return v as u64;
    }
    let mut visited = FxHashSet::default();
    let mut pos = 0usize;
    hash_val(v, desc as *const u8, &mut pos, &mut visited)
}

fn hash_val(val: i64, desc: *const u8, pos: &mut usize, visited: &mut FxHashSet<i64>) -> u64 {
    if val != 0 && crate::is_active_object(val) && !visited.insert(val) {
        skip(desc, pos);
        return 0;
    }
    let tag = unsafe { byte(desc, *pos) };
    *pos += 1;
    match tag {
        D_STR => hash_str(val),
        D_ANY | D_BYTES => hash_any(val),
        D_LIST => hash_list(val, desc, pos, visited),
        D_SET => hash_set(val, desc, pos, visited),
        D_TUPLE => hash_tuple(val, desc, pos, visited),
        D_DICT => hash_dict(val, desc, pos, visited),
        D_STRUCT => hash_struct(val, desc, pos, visited),
        D_ENUM => hash_enum(val, desc, pos, visited),
        D_BACKREF => {
            let hi = unsafe { byte(desc, *pos) } as usize;
            let lo = unsafe { byte(desc, *pos + 1) } as usize;
            *pos += 2;
            let mut target_pos = (hi << 8) | lo;
            hash_val(val, desc, &mut target_pos, visited)
        }
        _ => one(val as u64),
    }
}

fn one(v: u64) -> u64 {
    let mut h = FxHasher::default();
    h.write_u64(v);
    h.finish()
}

/// Combines a sequence of hashes where order is representationally fixed
/// (struct fields, tuple/list elements): each feeds into one running hasher.
fn seq(parts: impl IntoIterator<Item = u64>) -> u64 {
    let mut h = FxHasher::default();
    for p in parts {
        h.write_u64(p);
    }
    h.finish()
}

/// Combines a sequence of hashes where order is not fixed (set elements,
/// dict entries): XOR is commutative, so pairing order never matters.
fn commutative(parts: impl IntoIterator<Item = u64>) -> u64 {
    parts.into_iter().fold(0u64, |acc, p| acc ^ p)
}

fn hash_str(val: i64) -> u64 {
    if val == 0 {
        return one(val as u64);
    }
    let mut h = FxHasher::default();
    h.write(crate::olive_str_to_bytes(val));
    h.finish()
}

fn hash_any(val: i64) -> u64 {
    // No static element type to walk; the runtime kind tag plus raw word is
    // the best available discriminator without re-deriving `classify_key`'s
    // full dispatch here.
    one(val as u64)
}

fn hash_list(val: i64, desc: *const u8, pos: &mut usize, visited: &mut FxHashSet<i64>) -> u64 {
    let inner_start = *pos;
    skip(desc, pos);
    if val == 0 || !slot_is_live(val) {
        return one(0);
    }
    let (eptr, elen) = unsafe {
        let s = &*(val as *const StableVec);
        (s.ptr, s.len)
    };
    let parts = (0..elen).map(|i| {
        let mut p = inner_start;
        hash_val(unsafe { *eptr.add(i) }, desc, &mut p, visited)
    });
    seq(parts)
}

fn hash_set(val: i64, desc: *const u8, pos: &mut usize, visited: &mut FxHashSet<i64>) -> u64 {
    let inner_start = *pos;
    skip(desc, pos);
    if val == 0 || !slot_is_live(val) {
        return one(0);
    }
    let (eptr, elen) = unsafe {
        let s = &*(val as *const OliveHashSet);
        (s.ptr, s.len)
    };
    let parts = (0..elen).map(|i| {
        let mut p = inner_start;
        hash_val(unsafe { *eptr.add(i) }, desc, &mut p, visited)
    });
    commutative(parts)
}

fn hash_tuple(val: i64, desc: *const u8, pos: &mut usize, visited: &mut FxHashSet<i64>) -> u64 {
    let n = unsafe { byte(desc, *pos) } as usize - 1;
    *pos += 1;
    if val == 0 || !slot_is_live(val) {
        for _ in 0..n {
            skip(desc, pos);
        }
        return one(0);
    }
    let (eptr, elen) = unsafe {
        let s = &*(val as *const StableVec);
        (s.ptr, s.len)
    };
    let mut parts = Vec::with_capacity(n);
    for i in 0..n {
        let elem = if i < elen { unsafe { *eptr.add(i) } } else { 0 };
        parts.push(hash_val(elem, desc, pos, visited));
    }
    seq(parts)
}

fn hash_dict(val: i64, desc: *const u8, pos: &mut usize, visited: &mut FxHashSet<i64>) -> u64 {
    let key_start = *pos;
    skip(desc, pos);
    let val_start = *pos;
    skip(desc, pos);
    if val == 0 || !slot_is_live(val) {
        return one(0);
    }
    let obj = unsafe { &*(val as *const OliveObj) };
    let parts = obj.fields.iter().map(|(k, &v)| {
        let mut kp = key_start;
        let kh = hash_val(k.0, desc, &mut kp, visited);
        let mut vp = val_start;
        let vh = hash_val(v, desc, &mut vp, visited);
        seq([kh, vh])
    });
    commutative(parts)
}

fn hash_struct(val: i64, desc: *const u8, pos: &mut usize, visited: &mut FxHashSet<i64>) -> u64 {
    crate::eq_typed::skip_lp(desc, pos);
    let n = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1;
    if val == 0 || !slot_is_live(val) {
        for _ in 0..n {
            crate::eq_typed::skip_lp(desc, pos);
            skip(desc, pos);
        }
        return one(0);
    }
    let n_fields = unsafe { *(val as *const i64) };
    let mut parts = Vec::with_capacity(n);
    for i in 0..n {
        crate::eq_typed::skip_lp(desc, pos);
        let field = if (i as i64) < n_fields {
            unsafe { *((val + 8 + 8 * i as i64) as *const i64) }
        } else {
            0
        };
        parts.push(hash_val(field, desc, pos, visited));
    }
    seq(parts)
}

fn hash_enum(val: i64, desc: *const u8, pos: &mut usize, visited: &mut FxHashSet<i64>) -> u64 {
    crate::eq_typed::skip_lp(desc, pos);
    let n = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1;
    let live = val != 0 && slot_is_live(val);
    let (tag, pptr, plen) = if live {
        let e = unsafe { &*(val as *const OliveEnum) };
        (e.tag as usize, e.payload_ptr, e.payload_len)
    } else {
        (usize::MAX, std::ptr::null_mut(), 0)
    };
    let mut parts = vec![one(tag as u64)];
    for i in 0..n {
        crate::eq_typed::skip_lp(desc, pos);
        let np = unsafe { byte(desc, *pos) } as usize - 13;
        *pos += 1;
        for j in 0..np {
            if live && i == tag && j < plen {
                parts.push(hash_val(unsafe { *pptr.add(j) }, desc, pos, visited));
            } else {
                skip(desc, pos);
            }
        }
    }
    seq(parts)
}
