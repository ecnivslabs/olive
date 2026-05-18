use crate::*;

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_new() -> i64 {
    let res = Box::into_raw(Box::new(OliveObj {
        kind: KIND_OBJ,
        fields: HashMap::default(),
    })) as i64;
    register_object(res);
    res
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_set(obj_ptr: i64, attr: i64, val: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        return obj_ptr;
    }
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    if let Some(attr_str) = olive_str_as_str(attr) {
        if let Some(val_ref) = m.fields.get_mut(attr_str) {
            *val_ref = val;
        } else {
            m.fields.insert(attr_str.to_string(), val);
        }
    }
    obj_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get(obj_ptr: i64, attr: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        return 0;
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    if let Some(attr_str) = olive_str_as_str(attr) {
        *m.fields.get(attr_str).unwrap_or(&0)
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_remove(obj_ptr: i64, attr: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        return 0;
    }
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    if let Some(attr_str) = olive_str_as_str(attr) {
        m.fields.remove(attr_str).unwrap_or(0)
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_in_obj(key: i64, obj_ptr: i64) -> i64 {
    if obj_ptr == 0 || key == 0 {
        return 0;
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    if let Some(key_str) = olive_str_as_str(key) {
        if m.fields.contains_key(key_str) { 1 } else { 0 }
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_len(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        return 0;
    }
    unsafe { (*(obj_ptr as *const OliveObj)).fields.len() as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_obj(ptr: i64) {
    if ptr != 0 {
        unregister_object(ptr);
        unsafe {
            let obj = Box::from_raw(ptr as *mut OliveObj);
            for &val in obj.fields.values() {
                if is_active_object(val) {
                    olive_free_any(val);
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_is_obj(val: i64) -> i64 {
    if val == 0 || (val & 1) != 0 {
        return 0;
    }
    let kind = unsafe { *(val as *const i64) };
    if kind == KIND_OBJ { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_keys(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        let v = Box::new(StableVec {
            kind: KIND_LIST,
            ptr: std::ptr::null_mut(),
            cap: 0,
            len: 0,
        });
        return Box::into_raw(v) as i64;
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let mut ptrs: Vec<i64> = m.fields.keys().map(|k| olive_str_internal(k)).collect();
    let ptr = ptrs.as_mut_ptr();
    let cap = ptrs.capacity();
    let len = ptrs.len();
    std::mem::forget(ptrs);
    Box::into_raw(Box::new(StableVec {
        kind: KIND_LIST,
        ptr,
        cap,
        len,
    })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_values(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        let v = Box::new(StableVec {
            kind: KIND_LIST,
            ptr: std::ptr::null_mut(),
            cap: 0,
            len: 0,
        });
        return Box::into_raw(v) as i64;
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let mut vals: Vec<i64> = m.fields.values().copied().collect();
    let ptr = vals.as_mut_ptr();
    let cap = vals.capacity();
    let len = vals.len();
    std::mem::forget(vals);
    Box::into_raw(Box::new(StableVec {
        kind: KIND_LIST,
        ptr,
        cap,
        len,
    })) as i64
}
