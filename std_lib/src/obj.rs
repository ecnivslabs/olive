use crate::*;
use std::cell::UnsafeCell;

const OBJ_POOL_CAP: usize = 131072;

struct ObjPool {
    entries: [*mut OliveObj; OBJ_POOL_CAP],
    count: usize,
}

unsafe impl Send for ObjPool {}

impl ObjPool {
    const fn new() -> Self {
        Self {
            entries: [std::ptr::null_mut(); OBJ_POOL_CAP],
            count: 0,
        }
    }
}

thread_local! {
    static OBJ_POOL: UnsafeCell<ObjPool> = UnsafeCell::new(ObjPool::new());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_new() -> i64 {
    let pooled = OBJ_POOL.with(|p| {
        let p = unsafe { &mut *p.get() };
        if p.count > 0 {
            p.count -= 1;
            p.entries[p.count]
        } else {
            std::ptr::null_mut()
        }
    });

    if !pooled.is_null() {
        let res = pooled as i64;
        return res;
    }

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
    let kind = unsafe { *(obj_ptr as *const i64) };
    if kind == KIND_PYOBJECT {
        return python::olive_py_setattr(obj_ptr as *mut std::ffi::c_void, attr, val) as i64;
    }
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    m.fields.insert(OliveStringKey(attr), val);
    obj_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_get(obj_ptr: i64, attr: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        return 0;
    }
    let kind = unsafe { *(obj_ptr as *const i64) };
    if kind == KIND_PYOBJECT {
        return python::olive_py_getattr(obj_ptr as *mut std::ffi::c_void, attr) as i64;
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    *m.fields.get(&OliveStringKey(attr)).unwrap_or(&0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_remove(obj_ptr: i64, attr: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        return 0;
    }
    let m = unsafe { &mut *(obj_ptr as *mut OliveObj) };
    m.fields.remove(&OliveStringKey(attr)).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_in_obj(key: i64, obj_ptr: i64) -> i64 {
    if obj_ptr == 0 || key == 0 {
        return 0;
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    if m.fields.contains_key(&OliveStringKey(key)) { 1 } else { 0 }
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
    if ptr == 0 {
        return;
    }
    let obj = unsafe { &mut *(ptr as *mut OliveObj) };
    for &val in obj.fields.values() {
        if is_active_object(val) {
            olive_free_any(val);
        }
    }
    obj.fields.clear();

    let returned = OBJ_POOL.with(|p| {
        let p = unsafe { &mut *p.get() };
        if p.count < OBJ_POOL_CAP {
            p.entries[p.count] = ptr as *mut OliveObj;
            p.count += 1;
            true
        } else {
            false
        }
    });

    if !returned {
        unregister_object(ptr);
        unsafe {
            let _ = Box::from_raw(ptr as *mut OliveObj);
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
        let res = Box::into_raw(Box::new(StableVec {
            kind: KIND_LIST,
            ptr: std::ptr::null_mut(),
            cap: 0,
            len: 0,
        })) as i64;
        register_object(res);
        return res;
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let mut ptrs: Vec<i64> = m.fields.keys().map(|k| k.0).collect();
    let ptr = ptrs.as_mut_ptr();
    let cap = ptrs.capacity();
    let len = ptrs.len();
    std::mem::forget(ptrs);
    let res = Box::into_raw(Box::new(StableVec {
        kind: KIND_LIST,
        ptr,
        cap,
        len,
    })) as i64;
    register_object(res);
    res
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_obj_values(obj_ptr: i64) -> i64 {
    if obj_ptr == 0 {
        let res = Box::into_raw(Box::new(StableVec {
            kind: KIND_LIST,
            ptr: std::ptr::null_mut(),
            cap: 0,
            len: 0,
        })) as i64;
        register_object(res);
        return res;
    }
    let m = unsafe { &*(obj_ptr as *const OliveObj) };
    let mut vals: Vec<i64> = m.fields.values().copied().collect();
    let ptr = vals.as_mut_ptr();
    let cap = vals.capacity();
    let len = vals.len();
    std::mem::forget(vals);
    let res = Box::into_raw(Box::new(StableVec {
        kind: KIND_LIST,
        ptr,
        cap,
        len,
    })) as i64;
    register_object(res);
    res
}
