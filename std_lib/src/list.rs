use crate::*;

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_new(len: i64) -> i64 {
    let n = len as usize;
    let mut v = Vec::with_capacity(n);
    unsafe {
        v.set_len(n);
    }
    for i in 0..n {
        v[i] = 0;
    }
    let ptr = v.as_mut_ptr();
    let cap = v.capacity();
    let len = v.len();
    std::mem::forget(v);

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
pub extern "C" fn olive_list_set(list_ptr: i64, idx: i64, val: i64) {
    if list_ptr == 0 {
        return;
    }
    let s = unsafe { &mut *(list_ptr as *mut StableVec) };
    if (idx as usize) < s.len {
        unsafe {
            *s.ptr.add(idx as usize) = val;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_get(list_ptr: i64, idx: i64) -> i64 {
    if list_ptr == 0 {
        return 0;
    }
    let s = unsafe { &*(list_ptr as *const StableVec) };
    if (idx as usize) < s.len {
        unsafe { *s.ptr.add(idx as usize) }
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_len(ptr: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    unsafe {
        let raw_ptr = ptr as *const libc::c_void;
        if python::is_readable_ptr(raw_ptr) {
            let kind = *(ptr as *const i64);
            if kind == KIND_PYOBJECT {
                return python::olive_py_len(ptr as *mut libc::c_void);
            }
        }
        (*(ptr as *const StableVec)).len as i64
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_insert(list_ptr: i64, idx: i64, val: i64) {
    if list_ptr == 0 {
        return;
    }
    unsafe {
        let s = &mut *(list_ptr as *mut StableVec);
        let idx = idx as usize;
        let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
        if idx <= v.len() {
            v.insert(idx, val);
        }
        s.ptr = v.as_mut_ptr();
        s.cap = v.capacity();
        s.len = v.len();
        std::mem::forget(v);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_remove(list_ptr: i64, idx: i64) -> i64 {
    if list_ptr == 0 {
        return 0;
    }
    unsafe {
        let s = &mut *(list_ptr as *mut StableVec);
        let idx = idx as usize;
        if idx >= s.len {
            return 0;
        }
        let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
        let val = v.remove(idx);
        s.ptr = v.as_mut_ptr();
        s.cap = v.capacity();
        s.len = v.len();
        std::mem::forget(v);
        val
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_list_concat(l: i64, r: i64) -> i64 {
    if l == 0 {
        return r;
    }
    if r == 0 {
        return l;
    }
    let sl = unsafe { &*(l as *const StableVec) };
    let sr = unsafe { &*(r as *const StableVec) };
    let mut v = Vec::with_capacity(sl.len + sr.len);
    unsafe {
        v.extend_from_slice(std::slice::from_raw_parts(sl.ptr, sl.len));
        v.extend_from_slice(std::slice::from_raw_parts(sr.ptr, sr.len));
    }
    let ptr = v.as_mut_ptr();
    let cap = v.capacity();
    let len = v.len();
    std::mem::forget(v);
    Box::into_raw(Box::new(StableVec {
        kind: KIND_LIST,
        ptr,
        cap,
        len,
    })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_list(ptr: i64) {
    if ptr != 0 {
        unregister_object(ptr);
        unsafe {
            let s = Box::from_raw(ptr as *mut StableVec);
            for i in 0..s.len {
                let elem = *s.ptr.add(i);
                if is_active_object(elem) {
                    olive_free_any(elem);
                }
            }
            if !s.ptr.is_null() {
                let _ = Vec::from_raw_parts(s.ptr, s.len, s.cap);
            }
        }
    }
}

struct OliveIter {
    list_ptr: i64,
    index: usize,
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_iter(list_ptr: i64) -> i64 {
    Box::into_raw(Box::new(OliveIter { list_ptr, index: 0 })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_has_next(iter_ptr: i64) -> i64 {
    if iter_ptr == 0 {
        return 0;
    }
    let it = unsafe { &*(iter_ptr as *const OliveIter) };
    if it.list_ptr == 0 {
        return 0;
    }
    let s = unsafe { &*(it.list_ptr as *const StableVec) };
    if it.index < s.len { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_next(iter_ptr: i64) -> i64 {
    if iter_ptr == 0 {
        return 0;
    }
    let it = unsafe { &mut *(iter_ptr as *mut OliveIter) };
    if it.list_ptr == 0 {
        return 0;
    }
    let s = unsafe { &*(it.list_ptr as *const StableVec) };
    if it.index < s.len {
        let val = unsafe { *s.ptr.add(it.index) };
        it.index += 1;
        val
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_is_list(val: i64) -> i64 {
    if val == 0 || (val & 1) != 0 {
        return 0;
    }
    let kind = unsafe { *(val as *const i64) };
    if kind == KIND_LIST { 1 } else { 0 }
}
