use crate::slab::GenSlab;
use crate::*;
use std::cell::UnsafeCell;

thread_local! {
    static ENUM_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::with_cleanup(std::mem::size_of::<OliveEnum>(), release_enum_storage)) };
}

/// Whether `v` lives in an enum slab. Distinguishes real enums (`KIND_ENUM`)
/// from raw structs whose field count collides with it: a 3-field struct
/// header reads as 3 without this gate, and its fields would then be read as
/// enum payload and descriptor words past the slot.
pub(crate) fn owns_enum(v: i64) -> bool {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            if (*active).enum_slab.owns_addr(v as usize) {
                return true;
            }
            if crate::slab::active_slab_is_global() {
                return ENUM_SLAB.with(|sl| (*sl.get()).owns_addr(v as usize));
            }
            return crate::slab::global_enum_owns_addr(v as usize);
        }
        ENUM_SLAB.with(|sl| (*sl.get()).owns_addr(v as usize))
            || crate::slab::global_enum_owns_addr(v as usize)
    }
}

pub(crate) unsafe fn release_enum_storage(body: *mut u8) {
    let e = unsafe { &mut *(body as *mut OliveEnum) };
    let ptr = std::mem::replace(&mut e.payload_ptr, std::ptr::null_mut());
    let len = std::mem::take(&mut e.payload_len);
    if !ptr.is_null() {
        drop(unsafe { Vec::from_raw_parts(ptr, len, len) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_new(type_id: i64, tag: i64, arg_count: i64, desc: i64) -> i64 {
    // `desc` is the raw `D_ENUM` descriptor pointer (passed untagged like
    // every other typed-free descriptor), stored for descriptor-less frees.
    let mut payload = vec![0i64; arg_count as usize];
    let payload_ptr = payload.as_mut_ptr();
    let payload_len = payload.len();
    std::mem::forget(payload);
    let slab_alloc = |sl: &mut GenSlab| {
        let (body, _) = sl.alloc();
        unsafe {
            std::ptr::write(
                body as *mut OliveEnum,
                OliveEnum {
                    kind: KIND_ENUM,
                    type_id,
                    tag,
                    payload_ptr,
                    payload_len,
                    desc,
                },
            );
        }
        body as i64
    };
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            slab_alloc(&mut (*active).enum_slab)
        } else {
            ENUM_SLAB.with(|sl| slab_alloc(&mut *sl.get()))
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_type_id(ptr: i64) -> i64 {
    if !crate::is_active_object(ptr) {
        return -1;
    }
    let kind = unsafe { *(ptr as *const i64) };
    if kind == KIND_ENUM {
        unsafe { (*(ptr as *const OliveEnum)).type_id }
    } else {
        -1
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_tag(ptr: i64) -> i64 {
    if !crate::is_active_object(ptr) {
        return -1;
    }
    let kind = unsafe { *(ptr as *const i64) };
    if kind == KIND_ENUM {
        unsafe { (*(ptr as *const OliveEnum)).tag }
    } else {
        -1
    }
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

/// Indexed payload store for enums with heap-owning payloads (`e[i] = v`):
/// releases the displaced payload word through the enum descriptor before
/// storing, mirroring the tuple replacing store. `desc` is the whole enum's
/// descriptor; the walk matches `free_enum` exactly, including its
/// self-assignment guard.
#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_set_typed(ptr: i64, index: i64, val: i64, desc: i64) {
    if ptr == 0 {
        return;
    }
    let (tag, pptr, plen) = unsafe {
        let e = &*(ptr as *const OliveEnum);
        (e.tag as usize, e.payload_ptr, e.payload_len)
    };
    if pptr.is_null() || (index as usize) >= plen {
        return;
    }
    let desc_ptr = desc as *const u8;
    let mut elem_pos = 0usize;
    let mut found = false;
    unsafe {
        // `free_val` consumes the `D_ENUM` tag before delegating; start past
        // it, then walk exactly like `free_enum`.
        if crate::format::byte(desc_ptr, 0) == crate::format::D_ENUM {
            let mut pos = 1usize;
            crate::free_typed::skip_lp(desc_ptr, &mut pos);
            let n = crate::format::byte(desc_ptr, pos) as usize - 13;
            pos += 1;
            'outer: for i in 0..n {
                crate::free_typed::skip_lp(desc_ptr, &mut pos);
                let np = crate::format::byte(desc_ptr, pos) as usize - 13;
                pos += 1;
                for j in 0..np {
                    if i == tag && j == index as usize {
                        elem_pos = pos;
                        found = true;
                        break 'outer;
                    }
                    crate::format::skip(desc_ptr, &mut pos);
                }
            }
        }
    }
    let slot = unsafe { &mut *pptr.add(index as usize) };
    let old = std::mem::replace(slot, val);
    if found && old != val {
        crate::free_typed::free_val(old, desc_ptr, &mut elem_pos);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_enum(ptr: i64) {
    if ptr == 0 {
        return;
    }
    let Some(is_global) = crate::slab::slab_membership(ptr) else {
        return;
    };
    // Same arena-aware ownership scan as `olive_free_obj`: an enum received
    // over a channel lives in the global escape arena and a purely local
    // owns_addr check would leak it.
    let is_ours = if is_global {
        crate::slab::with_escape_arena(|| enum_slab_owns(ptr))
    } else {
        enum_slab_owns(ptr)
    };
    if !is_ours {
        return;
    }
    // Descriptor-carrying enums free exactly like a typed enum free: the
    // payload walk is descriptor-driven, so struct payloads (whose header
    // word is a field count, not a kind) are read precisely rather than
    // kind-dispatched. Descriptor-less values keep the legacy behavior of
    // releasing storage alone.
    let desc = unsafe { (*(ptr as *const OliveEnum)).desc };
    if desc != 0 {
        crate::free_typed::olive_free_typed(ptr, desc);
        return;
    }
    if crate::slab::slot_is_live(ptr) {
        unsafe { release_enum_storage(ptr as *mut u8) };
    }
    free_enum_slot_raw_with(ptr, Some(is_global));
}

fn enum_slab_owns(ptr: i64) -> bool {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            (*active).enum_slab.owns_addr(ptr as usize)
        } else {
            ENUM_SLAB.with(|sl| (*sl.get()).owns_addr(ptr as usize))
        }
    }
}

pub(crate) fn free_enum_slot_raw(ptr: i64) {
    free_enum_slot_raw_with(ptr, None);
}

/// `known_global` skips the chunk lookup when the caller already classified
/// `ptr` a moment ago (e.g. `olive_free_enum`'s own span check).
pub(crate) fn free_enum_slot_raw_with(ptr: i64, known_global: Option<bool>) {
    if !crate::slab::slot_is_live(ptr) {
        return;
    }
    unsafe {
        let e = &mut *(ptr as *mut OliveEnum);
        e.payload_ptr = std::ptr::null_mut();
        e.payload_len = 0;
    }
    let is_global = known_global.unwrap_or_else(|| crate::slab::chunk_is_global(ptr as usize));
    if is_global {
        crate::slab::with_escape_arena(|| free_enum_slot_raw_local(ptr));
    } else {
        free_enum_slot_raw_local(ptr);
    }
}

fn free_enum_slot_raw_local(ptr: i64) {
    unsafe {
        let active = crate::slab::ACTIVE_SLABS.get();
        if !active.is_null() {
            (*active).enum_slab.free(ptr as *mut u8);
        } else {
            ENUM_SLAB.with(|sl| {
                (&mut *sl.get()).free(ptr as *mut u8);
            });
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_new_reuse(
    old_ptr: i64,
    type_id: i64,
    tag: i64,
    arg_count: i64,
    bump: i64,
    desc: i64,
) -> i64 {
    if old_ptr == 0 {
        return olive_enum_new(type_id, tag, arg_count, desc);
    }
    if bump != 0 {
        unsafe {
            let gen_ptr = (old_ptr as *mut std::sync::atomic::AtomicU64).sub(1);
            let g = crate::slab::advance_generation(
                (*gen_ptr).load(std::sync::atomic::Ordering::Relaxed),
                2,
            );
            (*gen_ptr).store(g, std::sync::atomic::Ordering::Release);
        }
    }
    let n = arg_count as usize;
    let e = unsafe { &mut *(old_ptr as *mut OliveEnum) };
    // A reuse normally follows `__olive_clear_typed` at the `Drop` site,
    // which already freed the old payloads and nulled the buffer. A
    // non-cleared slot still owns live words here, and overwriting them
    // would strand those payloads. Drain through the old descriptor first;
    // the clear path is idempotent, so a cleared slot is a no-op.
    if !e.payload_ptr.is_null() && e.payload_len != 0 && e.desc != 0 {
        crate::free_typed::olive_clear_typed(old_ptr, e.desc);
    }
    unsafe {
        // The header stores no separate capacity, so its payload allocation
        // must have exactly the recorded length, including after shrinking.
        if e.payload_ptr.is_null() || e.payload_len != n {
            if !e.payload_ptr.is_null() {
                let _ = Vec::from_raw_parts(e.payload_ptr, e.payload_len, e.payload_len);
            }
            let mut payload = vec![0i64; n];
            e.payload_ptr = payload.as_mut_ptr();
            e.payload_len = payload.len();
            std::mem::forget(payload);
        } else {
            std::ptr::write_bytes(e.payload_ptr, 0, n);
            e.payload_len = n;
        }
        e.type_id = type_id;
        e.tag = tag;
        e.desc = desc;
    }
    old_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_print_enum(ptr: i64) -> i64 {
    if ptr == 0 {
        println!("<null enum>");
        return 0;
    }
    let e = unsafe { &*(ptr as *const OliveEnum) };
    print!("Enum(type_id={}, tag={}", e.type_id, e.tag);
    if e.payload_len > 0 {
        print!(", payload=[");
        for i in 0..e.payload_len {
            if i > 0 {
                print!(", ");
            }
            let val = unsafe { *e.payload_ptr.add(i) };
            print!("{}", crate::format_list_elem(val));
        }
        print!("]");
    }
    println!(")");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_enum_basic() {
        let e = olive_enum_new(1, 0, 0, 0);
        assert_ne!(e, 0);
        assert_eq!(olive_enum_type_id(e), 1);
        assert_eq!(olive_enum_tag(e), 0);
    }

    #[test]
    fn enum_with_payload() {
        let e = olive_enum_new(1, 2, 3, 0);
        olive_enum_set(e, 0, 10);
        olive_enum_set(e, 1, 20);
        olive_enum_set(e, 2, 30);
        assert_eq!(olive_enum_get(e, 0), 10);
        assert_eq!(olive_enum_get(e, 1), 20);
        assert_eq!(olive_enum_get(e, 2), 30);
    }

    #[test]
    fn enum_get_out_of_bounds() {
        let e = olive_enum_new(0, 0, 1, 0);
        assert_eq!(olive_enum_get(e, 10), 0);
    }

    #[test]
    fn enum_set_out_of_bounds_no_panic() {
        let e = olive_enum_new(0, 0, 1, 0);
        olive_enum_set(e, 100, 42);
    }

    #[test]
    fn enum_type_id_multiple() {
        let e1 = olive_enum_new(42, 0, 0, 0);
        let e2 = olive_enum_new(99, 0, 0, 0);
        assert_eq!(olive_enum_type_id(e1), 42);
        assert_eq!(olive_enum_type_id(e2), 99);
    }

    #[test]
    fn free_enum_no_panic() {
        let e = olive_enum_new(0, 0, 3, 0);
        olive_free_enum(e);
    }

    #[test]
    fn free_enum_with_desc_frees_heap_payload() {
        use crate::format::{D_ENUM, D_STR};
        // [D_ENUM, lp("E"), 1 variant, lp("V"), 1 payload, D_STR].
        let desc = [D_ENUM, 14, b'E', 14, 14, b'V', 14, D_STR];
        let desc_ptr = desc.as_ptr() as i64;
        let e = olive_enum_new(1, 0, 1, desc_ptr);
        let s = crate::olive_str_internal("payload-string");
        let g = crate::string_slab::olive_str_gen_of(s);
        olive_enum_set(e, 0, s);
        olive_free_enum(e);
        assert_eq!(crate::string_slab::olive_str_gen_stale(s, g), 1);
    }

    #[test]
    fn free_enum_with_desc_walks_struct_payload() {
        use crate::format::{D_ENUM, D_STR, D_STRUCT};
        // Variant `V` holding a struct `P` with one string field: the walk
        // must reach through the struct precisely (a kind dispatch would
        // misread the struct header as a kind tag).
        let desc = [
            D_ENUM, 14, b'E', 14, 14, b'V', 14, D_STRUCT, 14, b'P', 14, 14, b'x', D_STR,
        ];
        let desc_ptr = desc.as_ptr() as i64;
        let e = olive_enum_new(1, 0, 1, desc_ptr);
        let st = crate::struct_obj::olive_struct_alloc(1);
        let s = crate::olive_str_internal("nested-string");
        let g = crate::string_slab::olive_str_gen_of(s);
        unsafe { *((st + 8) as *mut i64) = s };
        olive_enum_set(e, 0, st);
        olive_free_enum(e);
        assert_eq!(crate::string_slab::olive_str_gen_stale(s, g), 1);
    }

    #[test]
    fn set_typed_releases_displaced_str_payload() {
        use crate::format::{D_ENUM, D_STR};
        // [D_ENUM, lp("E"), 1 variant, lp("V"), 1 payload, D_STR].
        let desc = [D_ENUM, 14, b'E', 14, 14, b'V', 14, D_STR];
        let desc_ptr = desc.as_ptr() as i64;
        let e = olive_enum_new(1, 0, 1, 0);
        let old = crate::olive_str_internal("old-payload");
        let gold = crate::string_slab::olive_str_gen_of(old);
        olive_enum_set(e, 0, old);
        let new = crate::olive_str_internal("new-payload");
        let gnew = crate::string_slab::olive_str_gen_of(new);
        olive_enum_set_typed(e, 0, new, desc_ptr);
        assert_eq!(olive_enum_get(e, 0), new);
        assert_eq!(crate::string_slab::olive_str_gen_stale(old, gold), 1);
        crate::olive_free_str(new);
        assert_eq!(crate::string_slab::olive_str_gen_stale(new, gnew), 1);
        olive_free_enum(e);
    }

    #[test]
    fn set_typed_self_assignment_keeps_payload() {
        use crate::format::{D_ENUM, D_STR};
        let desc = [D_ENUM, 14, b'E', 14, 14, b'V', 14, D_STR];
        let desc_ptr = desc.as_ptr() as i64;
        let e = olive_enum_new(1, 0, 1, 0);
        let a = crate::olive_str_internal("same-payload");
        let g = crate::string_slab::olive_str_gen_of(a);
        olive_enum_set(e, 0, a);
        olive_enum_set_typed(e, 0, a, desc_ptr);
        assert_eq!(olive_enum_get(e, 0), a);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, g), 0);
        crate::olive_free_str(a);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, g), 1);
        olive_free_enum(e);
    }
}
