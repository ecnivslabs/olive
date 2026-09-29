//! Cross-product free-confusion test for the unified slab free classifier.
//! Every free entry is called against a live value of every other slab kind,
//! including raw structs with 0, 1, 2, 3, 6, 9, 16, and 20 fields whose
//! header words collide with real kind tags (1 `KIND_LIST`, 2 `KIND_OBJ`,
//! 3 `KIND_ENUM`, 6 `KIND_BYTES`, 9 `KIND_RESULT`, 16 `KIND_STRUCT_BOX`,
//! 20 `KIND_STRBUF`). A mismatched free must be a no-op: the victim stays
//! live, any heap payload stays live, and the victim's own free still
//! reclaims it afterwards. The single designed redirect,
//! `olive_free_list` on a set, frees through dispatch.

use crate::slab::slot_is_live;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Victim {
    List,
    Set,
    Obj,
    Enum,
    Bytes,
    Result,
    StructBox,
    Boxed,
    StrBuf,
    Struct0,
    Struct1,
    Struct2,
    Struct3,
    Struct6,
    Struct9,
    Struct16,
    Struct20,
    FatPtr,
}

const VICTIMS: [Victim; 18] = [
    Victim::List,
    Victim::Set,
    Victim::Obj,
    Victim::Enum,
    Victim::Bytes,
    Victim::Result,
    Victim::StructBox,
    Victim::Boxed,
    Victim::StrBuf,
    Victim::Struct0,
    Victim::Struct1,
    Victim::Struct2,
    Victim::Struct3,
    Victim::Struct6,
    Victim::Struct9,
    Victim::Struct16,
    Victim::Struct20,
    Victim::FatPtr,
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FreeEntry {
    List,
    Set,
    Struct,
    Obj,
    Enum,
    Bytes,
    Result,
    StructBox,
    Boxed,
    StrBuf,
}

const FREES: [FreeEntry; 10] = [
    FreeEntry::List,
    FreeEntry::Set,
    FreeEntry::Struct,
    FreeEntry::Obj,
    FreeEntry::Enum,
    FreeEntry::Bytes,
    FreeEntry::Result,
    FreeEntry::StructBox,
    FreeEntry::Boxed,
    FreeEntry::StrBuf,
];

fn call_free(entry: FreeEntry, ptr: i64) {
    match entry {
        FreeEntry::List => crate::list::olive_free_list(ptr),
        FreeEntry::Set => crate::set::olive_free_set(ptr),
        FreeEntry::Struct => crate::struct_obj::olive_free_struct(ptr),
        FreeEntry::Obj => crate::obj::olive_free_obj(ptr),
        FreeEntry::Enum => crate::enum_obj::olive_free_enum(ptr),
        FreeEntry::Bytes => crate::bytes::olive_buf_free(ptr),
        FreeEntry::Result => crate::result::olive_free_result(ptr),
        FreeEntry::StructBox => crate::struct_box::free_struct_box(ptr),
        FreeEntry::Boxed => crate::boxed::olive_free_boxed(ptr),
        FreeEntry::StrBuf => crate::strbuf::olive_strbuf_free(ptr),
    }
}

fn matching_free(victim: Victim) -> FreeEntry {
    match victim {
        Victim::List => FreeEntry::List,
        Victim::Set => FreeEntry::Set,
        Victim::Obj => FreeEntry::Obj,
        Victim::Enum => FreeEntry::Enum,
        Victim::Bytes => FreeEntry::Bytes,
        Victim::Result => FreeEntry::Result,
        Victim::StructBox => FreeEntry::StructBox,
        Victim::Boxed => FreeEntry::Boxed,
        Victim::StrBuf => FreeEntry::StrBuf,
        Victim::Struct0
        | Victim::Struct1
        | Victim::Struct2
        | Victim::Struct3
        | Victim::Struct6
        | Victim::Struct9
        | Victim::Struct16
        | Victim::Struct20 => FreeEntry::Struct,
        Victim::FatPtr => FreeEntry::Struct,
    }
}

struct Built {
    ptr: i64,
    payload_str: i64,
    payload_gen: i64,
}

fn heap_str(text: &str) -> (i64, i64) {
    let s = crate::olive_str_internal(text);
    let g = crate::string_slab::olive_str_gen_of(s);
    (s, g)
}

fn place_struct_field(ptr: i64, idx: usize, val: i64) {
    unsafe { *((ptr + 8 + 8 * idx as i64) as *mut i64) = val };
}

fn build(victim: Victim) -> Built {
    match victim {
        Victim::List => {
            let (s, g) = heap_str("confusion-list-payload");
            let l = crate::list::olive_list_new(1);
            crate::list::olive_list_set(l, 0, s);
            Built {
                ptr: l,
                payload_str: s,
                payload_gen: g,
            }
        }
        Victim::Set => {
            let (s, g) = heap_str("confusion-set-payload");
            let set = crate::set::olive_set_new(4);
            crate::set::olive_set_add(set, s);
            Built {
                ptr: set,
                payload_str: s,
                payload_gen: g,
            }
        }
        Victim::Obj => {
            let (s, g) = heap_str("confusion-obj-payload");
            let obj = crate::obj::olive_obj_new();
            crate::obj::olive_obj_set(obj, 1, s);
            Built {
                ptr: obj,
                payload_str: s,
                payload_gen: g,
            }
        }
        Victim::Enum => {
            let e = crate::enum_obj::olive_enum_new(1, 0, 1, 0);
            crate::enum_obj::olive_enum_set(e, 0, 42);
            Built {
                ptr: e,
                payload_str: 0,
                payload_gen: 0,
            }
        }
        Victim::Bytes => {
            let b = crate::bytes::new_buf(vec![1, 2, 3, 4]);
            Built {
                ptr: b,
                payload_str: 0,
                payload_gen: 0,
            }
        }
        Victim::Result => {
            let (s, g) = heap_str("confusion-result-payload");
            let r = crate::result::olive_result_ok(s);
            Built {
                ptr: r,
                payload_str: s,
                payload_gen: g,
            }
        }
        Victim::StructBox => {
            let (s, g) = heap_str("confusion-structbox-payload");
            let inner = crate::struct_obj::olive_struct_alloc(1);
            place_struct_field(inner, 0, s);
            let desc = [
                crate::format::D_STRUCT,
                14,
                b'P',
                14,
                14,
                b'x',
                crate::format::D_STR,
            ];
            let b = crate::struct_box::olive_struct_box(inner, desc.as_ptr() as i64);
            Built {
                ptr: b,
                payload_str: s,
                payload_gen: g,
            }
        }
        Victim::Boxed => {
            let b = crate::boxed::olive_box_int(1i64 << 61);
            assert!(slot_is_live(b));
            Built {
                ptr: b,
                payload_str: 0,
                payload_gen: 0,
            }
        }
        Victim::StrBuf => {
            let sb = crate::strbuf::olive_strbuf_new(8);
            let (s, _) = heap_str("confusion-strbuf-copy");
            crate::strbuf::olive_strbuf_push(sb, s);
            crate::olive_free_str(s);
            Built {
                ptr: sb,
                payload_str: 0,
                payload_gen: 0,
            }
        }
        Victim::Struct0 => {
            let st = crate::struct_obj::olive_struct_alloc(0);
            Built {
                ptr: st,
                payload_str: 0,
                payload_gen: 0,
            }
        }
        Victim::Struct1 => {
            let (s, g) = heap_str("confusion-struct1-payload");
            let st = crate::struct_obj::olive_struct_alloc(1);
            place_struct_field(st, 0, s);
            Built {
                ptr: st,
                payload_str: s,
                payload_gen: g,
            }
        }
        Victim::Struct2 => {
            let st = crate::struct_obj::olive_struct_alloc(2);
            place_struct_field(st, 0, 11);
            place_struct_field(st, 1, 22);
            Built {
                ptr: st,
                payload_str: 0,
                payload_gen: 0,
            }
        }
        Victim::Struct3 => {
            let st = crate::struct_obj::olive_struct_alloc(3);
            Built {
                ptr: st,
                payload_str: 0,
                payload_gen: 0,
            }
        }
        Victim::Struct6 => {
            let st = crate::struct_obj::olive_struct_alloc(6);
            Built {
                ptr: st,
                payload_str: 0,
                payload_gen: 0,
            }
        }
        Victim::Struct9 => {
            let st = crate::struct_obj::olive_struct_alloc(9);
            place_struct_field(st, 1, 123);
            Built {
                ptr: st,
                payload_str: 0,
                payload_gen: 0,
            }
        }
        Victim::Struct16 => {
            let st = crate::struct_obj::olive_struct_alloc(16);
            Built {
                ptr: st,
                payload_str: 0,
                payload_gen: 0,
            }
        }
        Victim::Struct20 => {
            let st = crate::struct_obj::olive_struct_alloc(20);
            Built {
                ptr: st,
                payload_str: 0,
                payload_gen: 0,
            }
        }
        Victim::FatPtr => {
            let fp = crate::struct_obj::olive_fatptr_alloc();
            unsafe {
                *((fp + 8) as *mut i64) = 0;
                *((fp + 24) as *mut i64) = 0;
            }
            Built {
                ptr: fp,
                payload_str: 0,
                payload_gen: 0,
            }
        }
    }
}

fn cleanup(victim: Victim, built: Built) {
    match victim {
        Victim::FatPtr => crate::struct_obj::olive_free_fatptr(built.ptr),
        Victim::Struct1 => {
            if built.payload_str != 0 {
                crate::olive_free_str(built.payload_str);
            }
            crate::struct_obj::olive_free_struct(built.ptr);
        }
        _ => call_free(matching_free(victim), built.ptr),
    }
}

fn payload_live(built: &Built) -> bool {
    if built.payload_str == 0 {
        return true;
    }
    crate::string_slab::olive_str_gen_stale(built.payload_str, built.payload_gen) == 0
}

#[test]
fn mismatched_free_is_noop() {
    for &entry in &FREES {
        for &victim in &VICTIMS {
            let designed_redirect = entry == FreeEntry::List && victim == Victim::Set;
            let is_match = matching_free(victim) == entry || designed_redirect;
            // `olive_free_struct` deliberately skips fatptr records; the
            // fatptr free owns them instead.
            let struct_skips_fatptr = entry == FreeEntry::Struct && victim == Victim::FatPtr;
            let built = build(victim);
            assert!(slot_is_live(built.ptr), "{entry:?} x {victim:?}: victim born live");
            call_free(entry, built.ptr);
            if is_match && !struct_skips_fatptr {
                assert!(
                    !slot_is_live(built.ptr),
                    "{entry:?} x {victim:?}: matching free must reclaim"
                );
                if built.payload_str != 0 {
                    if victim == Victim::Struct1 {
                        // The untyped struct free is shallow by design; the
                        // field string stays live for its own owner.
                        assert!(
                            payload_live(&built),
                            "{entry:?} x {victim:?}: shallow struct free keeps field"
                        );
                        crate::olive_free_str(built.payload_str);
                    } else {
                        assert_eq!(
                            crate::string_slab::olive_str_gen_stale(
                                built.payload_str,
                                built.payload_gen
                            ),
                            1,
                            "{entry:?} x {victim:?}: payload must go with owner"
                        );
                    }
                }
            } else {
                assert!(
                    slot_is_live(built.ptr),
                    "{entry:?} x {victim:?}: mismatched free must not reclaim"
                );
                assert!(
                    payload_live(&built),
                    "{entry:?} x {victim:?}: mismatched free must not touch payload"
                );
                let ptr = built.ptr;
                cleanup(victim, built);
                assert!(
                    !slot_is_live(ptr),
                    "{entry:?} x {victim:?}: owner free reclaims after confusion"
                );
            }
        }
    }
}
