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
    D_ANY, D_BACKREF, D_BYTES, D_DICT, D_ENUM, D_LIST, D_NULLABLE, D_SET, D_STR, D_STRUCT,
    D_STRUCT_SHARED, D_TUPLE, byte, skip,
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

/// Whether a key descriptor carries no static key type: zero, or an `Any`
/// descriptor (a set dispatched through `Any`, which threads `D_ANY` where a
/// typed op would thread the element type). Descriptor words arrive tagged
/// when they originate as `Str` constants; `str_body` strips the tag (a
/// no-op for raw codegen pointers).
pub(crate) fn is_untyped_desc(desc: i64) -> bool {
    if desc == 0 {
        return true;
    }
    let raw = crate::string_slab::str_body(desc);
    unsafe { crate::format::byte(raw as *const u8, 0) == crate::format::D_ANY }
}

struct ActiveKeyDescriptorGuard {
    previous: i64,
}

impl Drop for ActiveKeyDescriptorGuard {
    fn drop(&mut self) {
        ACTIVE_KEY_DESC.with(|d| d.set(self.previous));
    }
}

/// Installs `desc` for the duration of `f`. Runtime helpers that compose
/// typed entry points (an `update` that inserts through `olive_obj_set`)
/// use this so the inner op keeps classifying keys by the same type.
pub(crate) fn with_key_descriptor<R>(desc: i64, f: impl FnOnce() -> R) -> R {
    let previous = ACTIVE_KEY_DESC.with(|d| d.replace(desc));
    let _guard = ActiveKeyDescriptorGuard { previous };
    f()
}

/// Materializes an owned descriptor rooted at `start`. The selected subtree
/// is copied with all back-references rebased and any reachable definitions
/// outside the subtree appended. A raw child pointer is not a valid
/// descriptor: its offsets still address the original root, and a byte-string
/// child can also be unaligned.
pub(crate) fn owned_sub_descriptor(
    desc: *const u8,
    start: usize,
) -> crate::format::OwnedDescriptor {
    crate::format::owned_subdescriptor(desc, start)
}

/// Runs `f` with an owned, aligned descriptor rooted at `start`. The
/// descriptor is borrowed only for the callback, so callers must not retain
/// the integer word after `f` returns.
pub(crate) fn with_owned_sub_descriptor<R>(
    desc: *const u8,
    start: usize,
    f: impl FnOnce(i64) -> R,
) -> R {
    let owned = owned_sub_descriptor(desc, start);
    f(owned.as_i64())
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_set_typed(obj_ptr: i64, attr: i64, val: i64, key_desc: i64) -> i64 {
    with_key_descriptor(key_desc, || crate::obj::olive_obj_set(obj_ptr, attr, val))
}

/// `d.remove(k)` under a key descriptor: `olive_obj_remove` hashes and
/// compares the key, so a concrete scalar key needs the descriptor to be
/// classified by type instead of the magnitude heuristic.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_remove_typed(obj_ptr: i64, attr: i64, key_desc: i64) -> i64 {
    with_key_descriptor(key_desc, || {
        crate::obj::olive_obj_remove_inner(obj_ptr, attr, Some(key_desc))
    })
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

/// Structural-key `.get(k, d)` whose result feeds a tag-encoded slot: the
/// hit is boxed like `olive_obj_get_default_boxed`, the caller-passed
/// default arrives already boxed.
#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get_default_boxed_typed(
    obj_ptr: i64,
    attr: i64,
    default: i64,
    key_desc: i64,
    value_desc: i64,
) -> i64 {
    with_key_descriptor(key_desc, || {
        crate::obj::olive_obj_get_default_boxed(obj_ptr, attr, default, value_desc)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_add_typed(set_ptr: i64, val: i64, key_desc: i64) {
    let inserted = with_key_descriptor(key_desc, || crate::set::set_try_add(set_ptr, val));
    if !inserted {
        // Covers duplicates and the null-set path: `add` takes ownership,
        // so a rejected value must be released either way.
        crate::free_typed::olive_free_typed(val, key_desc);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_contains_typed(set_ptr: i64, val: i64, key_desc: i64) -> i64 {
    with_key_descriptor(key_desc, || crate::set::olive_set_contains(set_ptr, val))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_remove_typed(set_ptr: i64, val: i64, key_desc: i64) -> i64 {
    with_key_descriptor(key_desc, || {
        crate::set::olive_set_remove_inner(set_ptr, val, Some(key_desc))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_remove_checked_typed(
    set_ptr: i64,
    val: i64,
    loc: i64,
    key_desc: i64,
) -> i64 {
    with_key_descriptor(key_desc, || {
        crate::set::olive_set_remove_checked_inner(set_ptr, val, loc, Some(key_desc))
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
        crate::obj::olive_obj_pop_checked_inner(obj_ptr, attr, loc, Some(key_desc))
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
        crate::obj::olive_obj_pop_default_inner(obj_ptr, attr, default, Some(key_desc))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_setdefault_typed(
    obj_ptr: i64,
    attr: i64,
    default: i64,
    key_desc: i64,
    val_desc: i64,
) -> i64 {
    with_key_descriptor(key_desc, || {
        crate::obj::olive_obj_setdefault(obj_ptr, attr, default, val_desc)
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

/// Structural hash for a `Raw`-classified key, given the active key
/// descriptor. Boxes, enums, sequences, sets, and dicts carry their own
/// shape, so all hash structurally even with no static key type (an untyped
/// `Any`-keyed container or an `Any`-descriptor set op); otherwise falls back
/// to pointer identity (unchanged from before this existed). Typed containers
/// skip these checks and keep their exact fast path.
pub(crate) fn hash_key(v: i64) -> u64 {
    let desc = active_key_descriptor();
    if is_untyped_desc(desc) {
        if let Some(h) = hash_struct_box_key(v) {
            return h;
        }
        if let Some(h) = hash_enum_key(v) {
            return h;
        }
        if let Some(h) = hash_seq_key(v) {
            return h;
        }
        if let Some(h) = hash_set_key(v) {
            return h;
        }
        if let Some(h) = hash_dict_key(v) {
            return h;
        }
        return v as u64;
    }
    let mut visited = FxHashSet::default();
    let mut pos = 0usize;
    hash_val(v, desc as *const u8, &mut pos, &mut visited)
}

/// Whether `v` is a live struct box, whose embedded descriptor lets an
/// `Any`-keyed dict hash and compare it structurally without an active key
/// descriptor. Boxes are 8-aligned heap pointers; inline immediates, tagged
/// strings, and small words reject on bits alone with no slab lookup. The
/// slab gate comes before the kind read so a raw 16-field struct (whose
/// header collides with the box kind) never classifies as a box.
pub(crate) fn is_struct_box_key(v: i64) -> bool {
    if v == 0 || v & 7 != 0 || v < 0x1000 {
        return false;
    }
    if !crate::struct_box::owns_struct_box(v) {
        return false;
    }
    crate::is_active_object(v)
        && unsafe { *(v as *const i64) } == crate::struct_box::KIND_STRUCT_BOX
}

/// Structural hash for a struct box through its embedded descriptor, or
/// `None` when `v` is not a live struct box. Lets two distinct boxes holding
/// equal structs hash identically in an `Any`-keyed dict, the same rule
/// `==` derives through `eq_typed`.
pub(crate) fn hash_struct_box_key(v: i64) -> Option<u64> {
    if v == 0 || v & 7 != 0 || v < 0x1000 {
        return None;
    }
    if !crate::struct_box::owns_struct_box(v) || !crate::is_active_object(v) {
        return None;
    }
    let kind = unsafe { *(v as *const i64) };
    if kind != crate::struct_box::KIND_STRUCT_BOX {
        return None;
    }
    let (desc, inner) = unsafe {
        let b = &*(v as *const crate::struct_box::OliveStructBox);
        (b.desc, b.ptr)
    };
    if desc == 0 {
        return Some(one(v as u64));
    }
    let mut visited = FxHashSet::default();
    let mut pos = 0usize;
    Some(hash_val(inner, desc as *const u8, &mut pos, &mut visited))
}

/// Structural hash for a raw enum through its embedded descriptor, or `None`
/// when `v` is not a descriptor-carrying enum. Lets two distinct enums of
/// equal tag and payload hash identically in an `Any`-keyed dict. The slab
/// gate comes before the kind read so a raw 3-field struct (whose header
/// collides with the enum kind) never reads past its slot for payload and
/// descriptor words.
pub(crate) fn hash_enum_key(v: i64) -> Option<u64> {
    if v == 0 || v & 7 != 0 || v < 0x1000 {
        return None;
    }
    if !crate::enum_obj::owns_enum(v) || !crate::is_active_object(v) {
        return None;
    }
    if unsafe { *(v as *const i64) } != crate::KIND_ENUM {
        return None;
    }
    let desc = unsafe { (*(v as *const crate::OliveEnum)).desc };
    if desc == 0 {
        return None;
    }
    let mut visited = FxHashSet::default();
    let mut pos = 0usize;
    Some(hash_val(v, desc as *const u8, &mut pos, &mut visited))
}

/// Structural hash for an `Any` word without a descriptor, for sequence
/// elements in untyped keys. Inline immediates and bare scalars hash by word
/// (deterministic encodings make equal values identical words); strings by
/// content; boxes and enums structurally through their embedded descriptors;
/// sequences recurse; anything else falls back to word identity (a safe miss
/// for distinct instances, never a misread).
fn hash_any_word(v: i64, visited: &mut FxHashSet<i64>) -> u64 {
    if v == 0 {
        return one(0);
    }
    if v & 1 == 1 {
        if (v & !1) > 0x10000 {
            return hash_str(v);
        }
        return one(v as u64);
    }
    if v & 7 != 0 {
        return one(v as u64);
    }
    if v < 0x1000 || !crate::is_active_object(v) {
        return one(v as u64);
    }
    if !visited.insert(v) {
        return 0;
    }
    let kind = unsafe { *(v as *const i64) };
    if kind == crate::struct_box::KIND_STRUCT_BOX {
        if let Some(h) = hash_struct_box_key(v) {
            return h;
        }
        return one(v as u64);
    }
    if kind == crate::KIND_ENUM {
        if let Some(h) = hash_enum_key(v) {
            return h;
        }
        return one(v as u64);
    }
    if (kind == crate::KIND_LIST || kind == crate::KIND_ANY_LIST) && crate::list::owns_list(v) {
        let (eptr, elen) = unsafe {
            let s = &*(v as *const StableVec);
            (s.ptr, s.len)
        };
        let parts = (0..elen).map(|i| hash_any_word(unsafe { *eptr.add(i) }, visited));
        return seq(parts);
    }
    if kind == crate::KIND_SET && crate::set::owns_set(v) {
        let (eptr, elen) = unsafe {
            let s = &*(v as *const crate::OliveHashSet);
            (s.ptr, s.len)
        };
        let parts = (0..elen).map(|i| hash_any_word(unsafe { *eptr.add(i) }, visited));
        return commutative(parts);
    }
    if kind == crate::KIND_OBJ && crate::obj::owns_obj(v) {
        let obj = unsafe { &*(v as *const crate::OliveObj) };
        let parts = obj.fields.iter().map(|(k, &val)| {
            let kh = hash_any_word(k.0, visited);
            let vh = hash_any_word(val, visited);
            seq([kh, vh])
        });
        return commutative(parts);
    }
    if kind == crate::KIND_BYTES {
        let bytes = unsafe { &*(v as *const crate::bytes::OliveBytes) }.as_slice();
        return one(hash_bytes(bytes));
    }
    if kind == crate::KIND_FLOAT || kind == crate::KIND_INT || kind == crate::KIND_U64 {
        let b = unsafe { &*(v as *const crate::boxed::OliveBoxed) };
        return seq([kind as u64, b.bits as u64]);
    }
    one(v as u64)
}

/// Whether `v` is a live sequence in a list slab, whose elements hash by
/// content in an untyped key. The slab gate keeps raw structs (whose headers
/// collide with the list kind) on pointer identity.
pub(crate) fn is_seq_key(v: i64) -> bool {
    if v == 0 || v & 7 != 0 || v < 0x1000 {
        return false;
    }
    if !crate::list::owns_list(v) || !crate::is_active_object(v) {
        return false;
    }
    let kind = unsafe { *(v as *const i64) };
    kind == crate::KIND_LIST || kind == crate::KIND_ANY_LIST
}

/// Structural hash for a list or tuple key through its elements, or `None`
/// when `v` is not a live sequence in a list slab. Lets `(1, 2)` and `[1, 2]`
/// style keys hash by content in an `Any`-keyed dict. The slab gate keeps raw
/// structs (whose headers collide with the list kind) on pointer identity.
pub(crate) fn hash_seq_key(v: i64) -> Option<u64> {
    if v == 0 || v & 7 != 0 || v < 0x1000 {
        return None;
    }
    if !crate::list::owns_list(v) || !crate::is_active_object(v) {
        return None;
    }
    let kind = unsafe { *(v as *const i64) };
    if kind != crate::KIND_LIST && kind != crate::KIND_ANY_LIST {
        return None;
    }
    let (eptr, elen) = unsafe {
        let s = &*(v as *const StableVec);
        (s.ptr, s.len)
    };
    let mut visited = FxHashSet::default();
    visited.insert(v);
    let parts = (0..elen).map(|i| hash_any_word(unsafe { *eptr.add(i) }, &mut visited));
    Some(seq(parts))
}

/// Whether `v` is a live set in a set slab. Gates set key reads so raw
/// structs (whose headers collide with the set kind) never read past slots.
pub(crate) fn is_set_key(v: i64) -> bool {
    if v == 0 || v & 7 != 0 || v < 0x1000 {
        return false;
    }
    if !crate::set::owns_set(v) || !crate::is_active_object(v) {
        return false;
    }
    let kind = unsafe { *(v as *const i64) };
    kind == crate::KIND_SET
}

/// Whether `v` is a live dict in an object slab. Gates dict key reads so raw
/// structs (whose headers collide with the object kind) never read past slots.
pub(crate) fn is_dict_key(v: i64) -> bool {
    if v == 0 || v & 7 != 0 || v < 0x1000 {
        return false;
    }
    if !crate::obj::owns_obj(v) || !crate::is_active_object(v) {
        return false;
    }
    let kind = unsafe { *(v as *const i64) };
    kind == crate::KIND_OBJ
}

/// Structural hash for a set key through its members, or `None` when `v` is
/// not a live set. Commutative so insertion order never matters.
pub(crate) fn hash_set_key(v: i64) -> Option<u64> {
    if v == 0 || v & 7 != 0 || v < 0x1000 {
        return None;
    }
    if !crate::set::owns_set(v) || !crate::is_active_object(v) {
        return None;
    }
    if unsafe { *(v as *const i64) } != crate::KIND_SET {
        return None;
    }
    let (eptr, elen) = unsafe {
        let s = &*(v as *const crate::OliveHashSet);
        (s.ptr, s.len)
    };
    let mut visited = FxHashSet::default();
    visited.insert(v);
    let parts = (0..elen).map(|i| hash_any_word(unsafe { *eptr.add(i) }, &mut visited));
    Some(commutative(parts))
}

/// Structural hash for a dict key through its entries, or `None` when `v` is
/// not a live dict. Commutative so entry order never matters.
pub(crate) fn hash_dict_key(v: i64) -> Option<u64> {
    if v == 0 || v & 7 != 0 || v < 0x1000 {
        return None;
    }
    if !crate::obj::owns_obj(v) || !crate::is_active_object(v) {
        return None;
    }
    if unsafe { *(v as *const i64) } != crate::KIND_OBJ {
        return None;
    }
    let obj = unsafe { &*(v as *const crate::OliveObj) };
    let mut visited = FxHashSet::default();
    visited.insert(v);
    let parts = obj.fields.iter().map(|(k, &val)| {
        let kh = hash_any_word(k.0, &mut visited);
        let vh = hash_any_word(val, &mut visited);
        seq([kh, vh])
    });
    Some(commutative(parts))
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
        D_NULLABLE => {
            if val == 0 {
                skip(desc, pos);
                one(0)
            } else {
                hash_val(val, desc, pos, visited)
            }
        }
        D_ANY | D_BYTES => hash_any(val),
        D_LIST => hash_list(val, desc, pos, visited),
        D_SET => hash_set(val, desc, pos, visited),
        D_TUPLE => hash_tuple(val, desc, pos, visited),
        D_DICT => hash_dict(val, desc, pos, visited),
        D_STRUCT | D_STRUCT_SHARED => hash_struct(val, desc, pos, visited),
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

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = FxHasher::default();
    hasher.write(bytes);
    hasher.finish()
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
