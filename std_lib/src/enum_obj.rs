use crate::*;

#[unsafe(no_mangle)]
pub extern "C" fn olive_enum_new(type_id: i64, tag: i64, arg_count: i64) -> i64 {
    let mut payload = vec![0i64; arg_count as usize];
    let payload_ptr = payload.as_mut_ptr();
    let payload_len = payload.len();
    std::mem::forget(payload);
    let res = Box::into_raw(Box::new(OliveEnum {
        kind: KIND_ENUM,
        type_id,
        tag,
        payload_ptr,
        payload_len,
    })) as i64;
    register_object(res);
    res
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
    if ptr != 0 {
        unregister_object(ptr);
        unsafe {
            let e = Box::from_raw(ptr as *mut OliveEnum);
            let _ = Vec::from_raw_parts(e.payload_ptr, e.payload_len, e.payload_len);
        }
    }
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
