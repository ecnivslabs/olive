use crate::slab::GenSlab;
use crate::*;
use std::cell::UnsafeCell;

thread_local! {
    static ENUM_SLAB: UnsafeCell<GenSlab> =
        const { UnsafeCell::new(GenSlab::new(std::mem::size_of::<OliveEnum>())) };
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_new(type_id: i64, tag: i64, arg_count: i64) -> i64 {
    let mut payload = vec![0i64; arg_count as usize];
    let payload_ptr = payload.as_mut_ptr();
    let payload_len = payload.len();
    std::mem::forget(payload);
    ENUM_SLAB.with(|sl| {
        let sl = unsafe { &mut *sl.get() };
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
                },
            );
        }
        body as i64
    })
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

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_enum(ptr: i64) {
    if ptr == 0 || !crate::slab::ptr_in_slab_span(ptr) {
        return;
    }
    if crate::slab::slot_is_live(ptr) {
        unsafe {
            let e = &*(ptr as *const OliveEnum);
            let _ = Vec::from_raw_parts(e.payload_ptr, e.payload_len, e.payload_len);
        }
    }
    free_enum_slot_raw(ptr);
}

pub(crate) fn free_enum_slot_raw(ptr: i64) {
    ENUM_SLAB.with(|sl| {
        unsafe { &mut *sl.get() }.free(ptr as *mut u8);
    });
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
        let e = olive_enum_new(1, 0, 0);
        assert_ne!(e, 0);
        assert_eq!(olive_enum_type_id(e), 1);
        assert_eq!(olive_enum_tag(e), 0);
    }

    #[test]
    fn enum_with_payload() {
        let e = olive_enum_new(1, 2, 3);
        olive_enum_set(e, 0, 10);
        olive_enum_set(e, 1, 20);
        olive_enum_set(e, 2, 30);
        assert_eq!(olive_enum_get(e, 0), 10);
        assert_eq!(olive_enum_get(e, 1), 20);
        assert_eq!(olive_enum_get(e, 2), 30);
    }

    #[test]
    fn enum_get_out_of_bounds() {
        let e = olive_enum_new(0, 0, 1);
        assert_eq!(olive_enum_get(e, 10), 0);
    }

    #[test]
    fn enum_set_out_of_bounds_no_panic() {
        let e = olive_enum_new(0, 0, 1);
        olive_enum_set(e, 100, 42);
    }

    #[test]
    fn enum_type_id_multiple() {
        let e1 = olive_enum_new(42, 0, 0);
        let e2 = olive_enum_new(99, 0, 0);
        assert_eq!(olive_enum_type_id(e1), 42);
        assert_eq!(olive_enum_type_id(e2), 99);
    }

    #[test]
    fn free_enum_no_panic() {
        let e = olive_enum_new(0, 0, 3);
        olive_free_enum(e);
    }
}
