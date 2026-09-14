use crate::boxed::{TAG_BOOL, TAG_INT, TAG_MASK, TAG_NULL};
use crate::format::{
    D_ANY, D_BACKREF, D_BOOL, D_BYTES, D_DICT, D_ENUM, D_F32, D_FATPTR, D_FLOAT, D_INT, D_LIST,
    D_NULL, D_SET, D_STR, D_STRUCT, D_STRUCT_SHARED, D_TUPLE, D_U64, byte, skip,
};
use crate::{
    KIND_ANY_LIST, KIND_BYTES, KIND_ENUM, KIND_FLOAT, KIND_INT, KIND_LIST, KIND_OBJ, KIND_SET,
    KIND_U64, OliveHashSet, OliveObj, OliveStringKey, StableVec,
};
use rustc_hash::{FxHashMap, FxHashSet};

fn fault(expected: &str) -> ! {
    crate::panic::abort_unbox(&format!(
        "narrowed union holds a value of another type where {expected} was expected"
    ))
}

fn is_tagged_str(v: i64) -> bool {
    v & 1 == 1 && (v & !1) > 0x10000
}

fn struct_tag_name(desc: *const u8, pos: usize) -> Option<(u8, Vec<u8>)> {
    let tag = unsafe { byte(desc, pos) };
    if tag != D_STRUCT && tag != D_STRUCT_SHARED {
        return None;
    }
    let len = unsafe { byte(desc, pos + 1) } as usize - 13;
    if len > 512 {
        return None;
    }
    let name = unsafe { std::slice::from_raw_parts(desc.add(pos + 2), len) }.to_vec();
    Some((tag, name))
}

fn box_struct_name(box_desc: i64) -> Option<(u8, Vec<u8>)> {
    if box_desc == 0 {
        return None;
    }
    let p = box_desc as *const u8;
    let tag = unsafe { *p };
    if tag != D_STRUCT && tag != D_STRUCT_SHARED {
        return None;
    }
    let len = unsafe { *p.add(1) } as usize - 13;
    if len > 512 {
        return None;
    }
    let name = unsafe { std::slice::from_raw_parts(p.add(2), len) }.to_vec();
    Some((tag, name))
}

fn check_struct_desc(box_desc: i64, target: *const u8, target_pos: usize) {
    let (btag, bname) = box_struct_name(box_desc).unwrap_or_else(|| fault("a struct"));
    let (ttag, tname) = struct_tag_name(target, target_pos).unwrap_or_else(|| fault("a struct"));
    if btag != ttag || bname != tname {
        fault("a struct");
    }
}

/// Mirrors `any_needs_erase` over descriptor bytes: whether values of the
/// encoded type are boxed (or rebuilt erased) before crossing into an
/// `Any`-shaped slot. Dict keys consult it because erasure boxes
/// struct-ish keys but keeps scalars raw, and the two need different
/// unerase paths (unboxing versus copying). Revisits (recursive types
/// via back-references) answer false: reaching one means no struct was
/// met on the way down, and only scalars can cycle without one.
fn erase_needed(desc: *const u8, pos: usize, seen: &mut FxHashSet<usize>) -> bool {
    if !seen.insert(pos) {
        return false;
    }
    match unsafe { byte(desc, pos) } {
        D_STRUCT | D_STRUCT_SHARED | D_INT | D_U64 | D_FLOAT | D_F32 | D_NULL => true,
        D_LIST | D_SET => erase_needed(desc, pos + 1, seen),
        D_DICT => {
            let mut p = pos + 1;
            skip(desc, &mut p);
            erase_needed(desc, pos + 1, seen) || erase_needed(desc, p, seen)
        }
        D_TUPLE => {
            let n = unsafe { byte(desc, pos + 1) } as usize - 1;
            let mut p = pos + 2;
            for _ in 0..n {
                if erase_needed(desc, p, seen) {
                    return true;
                }
                skip(desc, &mut p);
            }
            false
        }
        D_BACKREF => {
            let hi = unsafe { byte(desc, pos + 1) } as usize;
            let lo = unsafe { byte(desc, pos + 2) } as usize;
            erase_needed(desc, (hi << 8) | lo, seen)
        }
        _ => false,
    }
}

fn live_kind(v: i64) -> Option<i64> {
    if v == 0 || !crate::slab::slot_is_live(v) {
        return None;
    }
    // SAFETY: liveness above proves a live slot body, so the header read is valid.
    Some(unsafe { *(v as *const i64) })
}

pub(crate) fn unerase_scalar(any: i64, tag: u8) -> i64 {
    match tag {
        D_INT | D_U64 => {
            if any & TAG_MASK == TAG_INT {
                return any >> 3;
            }
            match live_kind(any) {
                Some(KIND_INT | KIND_U64) => {
                    // SAFETY: kind verified above on a live slot.
                    unsafe { (*(any as *const crate::boxed::OliveBoxed)).bits }
                }
                _ => fault("an integer"),
            }
        }
        D_FLOAT => match live_kind(any) {
            Some(KIND_FLOAT) => {
                // SAFETY: kind verified above on a live slot.
                unsafe { (*(any as *const crate::boxed::OliveBoxed)).bits }
            }
            _ => fault("a float"),
        },
        D_F32 => match live_kind(any) {
            Some(KIND_FLOAT) => {
                // SAFETY: kind verified above on a live slot.
                let bits = unsafe { (*(any as *const crate::boxed::OliveBoxed)).bits };
                (f64::from_bits(bits as u64) as f32).to_bits() as i64
            }
            _ => fault("a float"),
        },
        D_BOOL => {
            if any & TAG_MASK == TAG_BOOL {
                return any >> 3;
            }
            fault("a boolean")
        }
        D_NULL => {
            if any == TAG_NULL {
                return 0;
            }
            fault("null")
        }
        _ => fault("a scalar"),
    }
}

pub(crate) fn unerase_any(
    any: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashMap<i64, i64>,
) -> i64 {
    let tag_pos = *pos;
    let tag = unsafe { byte(desc, *pos) };
    *pos += 1;
    match tag {
        D_STR => {
            if !is_tagged_str(any) {
                fault("a string");
            }
            crate::copy_typed::copy_any(any, visited)
        }
        D_INT | D_U64 | D_FLOAT | D_F32 | D_BOOL | D_NULL => unerase_scalar(any, tag),
        D_ANY => crate::copy_typed::copy_any(any, visited),
        D_LIST => unerase_list_inner(any, desc, pos, visited),
        D_SET => unerase_set_inner(any, desc, pos, visited),
        D_DICT => unerase_dict_inner(any, desc, pos, visited),
        D_TUPLE => unerase_tuple_inner(any, desc, pos, visited),
        D_STRUCT | D_STRUCT_SHARED => {
            if any == 0 {
                return 0;
            }
            if crate::slab::slot_is_live(any) {
                // SAFETY: live slot, kind read valid.
                let kind = unsafe { *(any as *const i64) };
                if kind == crate::struct_box::KIND_STRUCT_BOX {
                    // SAFETY: kind verified, box header valid.
                    let (bdesc, inner) = unsafe {
                        let b = &*(any as *const crate::struct_box::OliveStructBox);
                        (b.desc, b.ptr)
                    };
                    check_struct_desc(bdesc, desc, tag_pos);
                    let mut sub = tag_pos;
                    let out = crate::copy_typed::copy_val(inner, desc, &mut sub, visited);
                    *pos = sub;
                    return out;
                }
            }
            fault("a struct")
        }
        D_ENUM => match live_kind(any) {
            Some(KIND_ENUM) => {
                let mut sub = tag_pos;
                let out = crate::copy_typed::copy_val(any, desc, &mut sub, visited);
                *pos = sub;
                out
            }
            _ => fault("an enum"),
        },
        D_BYTES => match live_kind(any) {
            Some(KIND_BYTES) => {
                let mut sub = tag_pos;
                let out = crate::copy_typed::copy_val(any, desc, &mut sub, visited);
                *pos = sub;
                out
            }
            _ => fault("bytes"),
        },
        D_FATPTR => {
            let mut sub = tag_pos;
            let out = crate::copy_typed::copy_val(any, desc, &mut sub, visited);
            *pos = sub;
            out
        }
        D_BACKREF => {
            let target = (unsafe { byte(desc, *pos) } as usize) << 8
                | unsafe { byte(desc, *pos + 1) } as usize;
            *pos += 2;
            let mut target_pos = target;
            unerase_any(any, desc, &mut target_pos, visited)
        }
        _ => fault("a value of the expected type"),
    }
}

fn unerase_list_inner(
    erased: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashMap<i64, i64>,
) -> i64 {
    let inner_start = *pos;
    skip(desc, pos);
    if let Some(hit) = visited.get(&erased).copied() {
        return hit;
    }
    let (eptr, elen) = match live_kind(erased) {
        Some(KIND_LIST) | Some(KIND_ANY_LIST) => {
            // SAFETY: kind verified list like, header and buffer walk bounded by length.
            unsafe {
                let s = &*(erased as *const StableVec);
                (s.ptr, s.len)
            }
        }
        _ => fault("a list"),
    };
    let new = crate::list::olive_list_new(elen as i64);
    visited.insert(erased, new);
    for i in 0..elen {
        // SAFETY: buffer walk bounded by the checked header length above.
        let elem = unsafe { *eptr.add(i) };
        let mut p = inner_start;
        let c = unerase_any(elem, desc, &mut p, visited);
        crate::list::olive_list_set(new, i as i64, c);
    }
    new
}

fn unerase_set_inner(
    erased: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashMap<i64, i64>,
) -> i64 {
    let inner_start = *pos;
    skip(desc, pos);
    if let Some(hit) = visited.get(&erased).copied() {
        return hit;
    }
    let (eptr, elen) = match live_kind(erased) {
        Some(KIND_SET) => {
            // SAFETY: kind verified set, header walk bounded by length.
            unsafe {
                let s = &*(erased as *const OliveHashSet);
                (s.ptr, s.len)
            }
        }
        _ => fault("a set"),
    };
    let new = crate::set::olive_set_new(elen as i64);
    visited.insert(erased, new);
    // Owned copy: a mid-string sub-pointer is generally unaligned and tag
    // stripping would corrupt it (see `owned_sub_descriptor`).
    let elem_owned = crate::hash_typed::owned_sub_descriptor(desc, inner_start);
    let elem_desc = elem_owned.as_ptr() as i64;
    for i in 0..elen {
        // SAFETY: buffer walk bounded by the checked header length above.
        let elem = unsafe { *eptr.add(i) };
        let mut p = inner_start;
        let c = unerase_any(elem, desc, &mut p, visited);
        crate::hash_typed::olive_set_add_typed(new, c, elem_desc);
    }
    new
}

fn unerase_dict_inner(
    erased: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashMap<i64, i64>,
) -> i64 {
    let key_start = *pos;
    skip(desc, pos);
    let val_start = *pos;
    skip(desc, pos);
    if let Some(hit) = visited.get(&erased).copied() {
        return hit;
    }
    match live_kind(erased) {
        Some(KIND_OBJ) => {}
        _ => fault("a dict"),
    }
    // SAFETY: kind verified dict, field walk stays inside the map.
    let obj = unsafe { &*(erased as *const OliveObj) };
    let new = crate::obj::olive_obj_new();
    visited.insert(erased, new);
    let mut fields = FxHashMap::default();
    // Owned copy: a mid-string sub-pointer is generally unaligned and tag
    // stripping would corrupt it (see `owned_sub_descriptor`).
    let key_owned = crate::hash_typed::owned_sub_descriptor(desc, key_start);
    let key_desc = key_owned.as_ptr() as i64;
    // Keys mirror erasure exactly: struct-ish keys arrive boxed and must
    // unbox through the key type, while scalars arrive raw and copy. The
    // predicate reads the key encoding, the same rule `box_into_any`
    // applies when the dict crosses into the union.
    let keys_erased = erase_needed(desc, key_start, &mut FxHashSet::default());
    for (k, &v) in obj.fields.iter() {
        let kc = if keys_erased {
            let mut kp = key_start;
            unerase_any(k.0, desc, &mut kp, visited)
        } else {
            let mut kp = key_start;
            crate::copy_typed::copy_val(k.0, desc, &mut kp, visited)
        };
        let mut vp = val_start;
        let vc = unerase_any(v, desc, &mut vp, visited);
        crate::hash_typed::with_key_descriptor(key_desc, || {
            fields.insert(OliveStringKey(kc), vc);
        });
    }
    unsafe { (*(new as *mut OliveObj)).fields = fields };
    new
}

fn unerase_tuple_inner(
    erased: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashMap<i64, i64>,
) -> i64 {
    let n = unsafe { byte(desc, *pos) } as usize - 1;
    *pos += 1;
    if let Some(hit) = visited.get(&erased).copied() {
        for _ in 0..n {
            skip(desc, pos);
        }
        return hit;
    }
    let (eptr, elen) = match live_kind(erased) {
        Some(KIND_LIST) | Some(KIND_ANY_LIST) => {
            // SAFETY: kind verified list like tuple storage, walk bounded by length.
            unsafe {
                let s = &*(erased as *const StableVec);
                (s.ptr, s.len)
            }
        }
        _ => fault("a tuple"),
    };
    if elen != n {
        fault("a tuple");
    }
    let new = crate::list::olive_list_new(n as i64);
    visited.insert(erased, new);
    for i in 0..n {
        // SAFETY: length equality verified above, indexed walk stays inside.
        let elem = unsafe { *eptr.add(i) };
        let c = unerase_any(elem, desc, pos, visited);
        crate::list::olive_list_set(new, i as i64, c);
    }
    new
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_unerase(erased: i64, target_desc: i64) -> i64 {
    if erased == 0 {
        fault("a list");
    }
    let desc = crate::string_slab::str_body(target_desc) as *const u8;
    if unsafe { byte(desc, 0) } != D_LIST {
        fault("a list");
    }
    let mut visited = FxHashMap::default();
    let mut pos = 1usize;
    unerase_list_inner(erased, desc, &mut pos, &mut visited)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_tuple_unerase(erased: i64, target_desc: i64) -> i64 {
    if erased == 0 {
        fault("a tuple");
    }
    let desc = crate::string_slab::str_body(target_desc) as *const u8;
    if unsafe { byte(desc, 0) } != D_TUPLE {
        fault("a tuple");
    }
    let mut visited = FxHashMap::default();
    let mut pos = 1usize;
    unerase_tuple_inner(erased, desc, &mut pos, &mut visited)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_unerase(erased: i64, target_desc: i64) -> i64 {
    if erased == 0 {
        fault("a dict");
    }
    let desc = crate::string_slab::str_body(target_desc) as *const u8;
    if unsafe { byte(desc, 0) } != D_DICT {
        fault("a dict");
    }
    let mut visited = FxHashMap::default();
    let mut pos = 1usize;
    unerase_dict_inner(erased, desc, &mut pos, &mut visited)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_unerase(erased: i64, target_desc: i64) -> i64 {
    if erased == 0 {
        fault("a set");
    }
    let desc = crate::string_slab::str_body(target_desc) as *const u8;
    if unsafe { byte(desc, 0) } != D_SET {
        fault("a set");
    }
    let mut visited = FxHashMap::default();
    let mut pos = 1usize;
    unerase_set_inner(erased, desc, &mut pos, &mut visited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustc_hash::FxHashSet;

    fn en(desc: &[u8], pos: usize) -> bool {
        erase_needed(desc.as_ptr(), pos, &mut FxHashSet::default())
    }

    #[test]
    fn struct_needs_erase_int_does_not() {
        assert!(en(&[D_STRUCT], 0));
        assert!(en(&[D_STRUCT_SHARED], 0));
        assert!(en(&[D_INT], 0));
        assert!(en(&[D_FLOAT], 0));
        assert!(en(&[D_F32], 0));
        assert!(en(&[D_NULL], 0));
        assert!(!en(&[D_BOOL], 0));
        assert!(!en(&[D_STR], 0));
        assert!(!en(&[D_ANY], 0));
    }

    #[test]
    fn containers_recurse() {
        assert!(en(&[D_LIST, D_STRUCT], 0));
        assert!(en(&[D_LIST, D_INT], 0));
        assert!(en(&[D_DICT, D_INT, D_STRUCT], 0));
        assert!(en(&[D_DICT, D_INT, D_INT], 0));
        assert!(en(&[D_TUPLE, 3, D_INT, D_STRUCT], 0));
        assert!(en(&[D_TUPLE, 3, D_INT, D_INT], 0));
    }

    #[test]
    fn tuple_unerase_backref_advances_only_three_bytes() {
        let d = vec![
            D_TUPLE, 4, D_STRUCT, 14, b'O', 14, 14, b'v', D_STR, D_BACKREF, 0, 2, D_INT, 0,
        ];
        let desc = d.as_ptr() as i64;
        let sub_desc = crate::index_any::intern_sub_descriptor(d.as_ptr(), 2);
        let first = crate::olive_struct_alloc(1);
        let second = crate::olive_struct_alloc(1);
        unsafe {
            *((first + 8) as *mut i64) = crate::olive_str_internal("first");
            *((second + 8) as *mut i64) = crate::olive_str_internal("second");
        }
        let erased = crate::list::list_from_vec(vec![
            crate::struct_box::olive_struct_box(first, sub_desc),
            crate::struct_box::olive_struct_box(second, sub_desc),
            crate::boxed::olive_box_int(7),
        ]);
        unsafe { (*(erased as *mut StableVec)).kind = KIND_ANY_LIST };
        let result = olive_tuple_unerase(erased, desc);
        assert!(crate::list::olive_list_get(result, 0) != 0);
        assert_eq!(
            crate::olive_str_from_ptr(unsafe {
                *((crate::list::olive_list_get(result, 0) + 8) as *const i64)
            }),
            "first"
        );
        assert_eq!(
            crate::olive_str_from_ptr(unsafe {
                *((crate::list::olive_list_get(result, 1) + 8) as *const i64)
            }),
            "second"
        );
        assert_eq!(crate::list::olive_list_get(result, 2), 7);
        crate::olive_free_any(erased);
        crate::free_typed::olive_free_typed(result, desc);
    }
}
