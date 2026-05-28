//! Descriptor-driven deep equality, the read-only sibling of `copy_typed`.
//! Walks the same byte descriptor structs/enums/tuples/lists/sets/dicts get
//! for copy and free, so `==` on an aggregate compares values field-by-field
//! (matching Python's `[1,2] == [1,2]`) instead of pointer identity.
//!
//! Sets and dicts have no fixed element order, so their equality is a
//! mutual-containment match rather than a zip: every element of one side
//! must pair with a distinct, structurally-equal element of the other.
//! Greedy first-match pairing is exact for this (equality is transitive),
//! at the cost of being O(n^2) instead of hash-bucketed -- a hashed
//! comparison needs collections to hash aggregate elements structurally,
//! which is a separate piece of work.
//!
//! Cycles: a self-referential value pair already on the call stack is
//! assumed equal rather than walked again, the same call `visited` plays
//! for `copy_typed`'s pointer-remap map, just keyed by the compared pair.

use crate::format::{
    D_ANY, D_BACKREF, D_BYTES, D_DICT, D_ENUM, D_LIST, D_SET, D_STR, D_STRUCT, D_TUPLE, byte, skip,
};
use crate::slab::slot_is_live;
use crate::{OliveEnum, OliveHashSet, OliveObj, StableVec};
use rustc_hash::FxHashSet;

#[unsafe(no_mangle)]
pub extern "C" fn olive_eq_typed(a: i64, b: i64, desc: i64) -> i64 {
    let mut visited = FxHashSet::default();
    let mut pos = 0usize;
    eq_val(a, b, desc as *const u8, &mut pos, &mut visited) as i64
}

/// Skips a length-prefixed name; length byte is biased by 13.
pub(crate) fn skip_lp(desc: *const u8, pos: &mut usize) {
    let len = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1 + len;
}

/// Structural equality entry point for a dict/set key comparison, given the
/// container's active key descriptor (see `hash_typed`). Zero-position start.
pub(crate) fn eq_key(a: i64, b: i64, desc: i64) -> bool {
    let mut visited = FxHashSet::default();
    let mut pos = 0usize;
    eq_val(a, b, desc as *const u8, &mut pos, &mut visited)
}

pub(crate) fn eq_val(
    a: i64,
    b: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashSet<(i64, i64)>,
) -> bool {
    if a == b {
        skip(desc, pos);
        return true;
    }
    if a != 0
        && b != 0
        && crate::is_active_object(a)
        && crate::is_active_object(b)
        && visited.contains(&(a, b))
    {
        skip(desc, pos);
        return true;
    }
    let tag = unsafe { byte(desc, *pos) };
    *pos += 1;
    match tag {
        D_STR => crate::olive_str_eq(a, b) != 0,
        D_ANY | D_BYTES => crate::olive_any_eq(a, b) != 0,
        D_LIST => eq_list(a, b, desc, pos, visited),
        D_SET => eq_set(a, b, desc, pos, visited),
        D_TUPLE => eq_tuple(a, b, desc, pos, visited),
        D_DICT => eq_dict(a, b, desc, pos, visited),
        D_STRUCT => eq_struct(a, b, desc, pos, visited),
        D_ENUM => eq_enum(a, b, desc, pos, visited),
        D_BACKREF => {
            // Purely a descriptor-size bound for a recursive *type*
            // (`Node.next: Node`), not evidence the *values* cycle -- most
            // backref traversals are just a longer, still-finite chain,
            // so this must not mark `(a, b)` as already-equal. Real value
            // cycles are still caught: each aggregate helper inserts into
            // `visited` once it starts comparing that pair's fields.
            let hi = unsafe { byte(desc, *pos) } as usize;
            let lo = unsafe { byte(desc, *pos + 1) } as usize;
            *pos += 2;
            let mut target_pos = (hi << 8) | lo;
            eq_val(a, b, desc, &mut target_pos, visited)
        }
        // Scalars, PyObject, and the recursion-cut `D_OBJ` word compare raw;
        // the `a == b` shortcut above already covers the equal case, so
        // reaching here on a scalar tag means unequal.
        _ => false,
    }
}

fn eq_list(
    a: i64,
    b: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashSet<(i64, i64)>,
) -> bool {
    let inner_start = *pos;
    skip(desc, pos);
    let a_live = a != 0 && slot_is_live(a);
    let b_live = b != 0 && slot_is_live(b);
    if !a_live || !b_live {
        return a == b;
    }
    let (aptr, alen) = unsafe {
        let s = &*(a as *const StableVec);
        (s.ptr, s.len)
    };
    let (bptr, blen) = unsafe {
        let s = &*(b as *const StableVec);
        (s.ptr, s.len)
    };
    if alen != blen {
        return false;
    }
    visited.insert((a, b));
    let mut equal = true;
    for i in 0..alen {
        let av = unsafe { *aptr.add(i) };
        let bv = unsafe { *bptr.add(i) };
        let mut p = inner_start;
        if !eq_val(av, bv, desc, &mut p, visited) {
            equal = false;
        }
    }
    equal
}

fn eq_set(
    a: i64,
    b: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashSet<(i64, i64)>,
) -> bool {
    let inner_start = *pos;
    skip(desc, pos);
    let a_live = a != 0 && slot_is_live(a);
    let b_live = b != 0 && slot_is_live(b);
    if !a_live || !b_live {
        return a == b;
    }
    let (aptr, alen) = unsafe {
        let s = &*(a as *const OliveHashSet);
        (s.ptr, s.len)
    };
    let (bptr, blen) = unsafe {
        let s = &*(b as *const OliveHashSet);
        (s.ptr, s.len)
    };
    if alen != blen {
        return false;
    }
    visited.insert((a, b));
    let mut used = vec![false; blen];
    'outer: for i in 0..alen {
        let av = unsafe { *aptr.add(i) };
        for (j, u) in used.iter_mut().enumerate() {
            if *u {
                continue;
            }
            let bv = unsafe { *bptr.add(j) };
            let mut p = inner_start;
            if eq_val(av, bv, desc, &mut p, visited) {
                *u = true;
                continue 'outer;
            }
        }
        return false;
    }
    true
}

fn eq_tuple(
    a: i64,
    b: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashSet<(i64, i64)>,
) -> bool {
    let n = unsafe { byte(desc, *pos) } as usize - 1;
    *pos += 1;
    let a_live = a != 0 && slot_is_live(a);
    let b_live = b != 0 && slot_is_live(b);
    if !a_live || !b_live {
        for _ in 0..n {
            skip(desc, pos);
        }
        return a == b;
    }
    let (aptr, alen) = unsafe {
        let s = &*(a as *const StableVec);
        (s.ptr, s.len)
    };
    let (bptr, blen) = unsafe {
        let s = &*(b as *const StableVec);
        (s.ptr, s.len)
    };
    visited.insert((a, b));
    let mut equal = true;
    for i in 0..n {
        let av = if i < alen { unsafe { *aptr.add(i) } } else { 0 };
        let bv = if i < blen { unsafe { *bptr.add(i) } } else { 0 };
        if !eq_val(av, bv, desc, pos, visited) {
            equal = false;
        }
    }
    equal
}

fn eq_dict(
    a: i64,
    b: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashSet<(i64, i64)>,
) -> bool {
    let key_start = *pos;
    skip(desc, pos);
    let val_start = *pos;
    skip(desc, pos);
    let a_live = a != 0 && slot_is_live(a);
    let b_live = b != 0 && slot_is_live(b);
    if !a_live || !b_live {
        return a == b;
    }
    let a_obj = unsafe { &*(a as *const OliveObj) };
    let b_obj = unsafe { &*(b as *const OliveObj) };
    if a_obj.fields.len() != b_obj.fields.len() {
        return false;
    }
    visited.insert((a, b));
    let b_entries: Vec<(i64, i64)> = b_obj.fields.iter().map(|(k, &v)| (k.0, v)).collect();
    let mut used = vec![false; b_entries.len()];
    'outer: for (ak, &av) in a_obj.fields.iter() {
        for (j, u) in used.iter_mut().enumerate() {
            if *u {
                continue;
            }
            let (bk, bv) = b_entries[j];
            let mut kp = key_start;
            if !eq_val(ak.0, bk, desc, &mut kp, visited) {
                continue;
            }
            let mut vp = val_start;
            if eq_val(av, bv, desc, &mut vp, visited) {
                *u = true;
                continue 'outer;
            }
        }
        return false;
    }
    true
}

fn eq_struct(
    a: i64,
    b: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashSet<(i64, i64)>,
) -> bool {
    skip_lp(desc, pos);
    let n = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1;
    let a_live = a != 0 && slot_is_live(a);
    let b_live = b != 0 && slot_is_live(b);
    if !a_live || !b_live {
        for _ in 0..n {
            skip_lp(desc, pos);
            skip(desc, pos);
        }
        return a == b;
    }
    let a_n = unsafe { *(a as *const i64) };
    let b_n = unsafe { *(b as *const i64) };
    visited.insert((a, b));
    let mut equal = true;
    for i in 0..n {
        skip_lp(desc, pos);
        let af = if (i as i64) < a_n {
            unsafe { *((a + 8 + 8 * i as i64) as *const i64) }
        } else {
            0
        };
        let bf = if (i as i64) < b_n {
            unsafe { *((b + 8 + 8 * i as i64) as *const i64) }
        } else {
            0
        };
        if !eq_val(af, bf, desc, pos, visited) {
            equal = false;
        }
    }
    equal
}

fn eq_enum(
    a: i64,
    b: i64,
    desc: *const u8,
    pos: &mut usize,
    visited: &mut FxHashSet<(i64, i64)>,
) -> bool {
    skip_lp(desc, pos);
    let n = unsafe { byte(desc, *pos) } as usize - 13;
    *pos += 1;
    let a_live = a != 0 && slot_is_live(a);
    let b_live = b != 0 && slot_is_live(b);
    if !a_live || !b_live {
        for _ in 0..n {
            skip_lp(desc, pos);
            let np = unsafe { byte(desc, *pos) } as usize - 13;
            *pos += 1;
            for _ in 0..np {
                skip(desc, pos);
            }
        }
        return a == b;
    }
    let (a_tag, a_pptr, a_plen) = unsafe {
        let e = &*(a as *const OliveEnum);
        (e.tag as usize, e.payload_ptr, e.payload_len)
    };
    let (b_tag, b_pptr, b_plen) = unsafe {
        let e = &*(b as *const OliveEnum);
        (e.tag as usize, e.payload_ptr, e.payload_len)
    };
    visited.insert((a, b));
    let tags_match = a_tag == b_tag;
    let mut payload_equal = true;
    for i in 0..n {
        skip_lp(desc, pos);
        let np = unsafe { byte(desc, *pos) } as usize - 13;
        *pos += 1;
        for j in 0..np {
            if i == a_tag && tags_match && j < a_plen && j < b_plen {
                let av = unsafe { *a_pptr.add(j) };
                let bv = unsafe { *b_pptr.add(j) };
                if !eq_val(av, bv, desc, pos, visited) {
                    payload_equal = false;
                }
            } else {
                skip(desc, pos);
            }
        }
    }
    tags_match && payload_equal
}
