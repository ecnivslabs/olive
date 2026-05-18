use crate::*;
use rustc_hash::FxHashSet;

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_new(capacity: i64) -> i64 {
    let cap = capacity as usize;
    let mut v: Vec<i64> = Vec::with_capacity(cap);
    let ptr = v.as_mut_ptr();
    let v_cap = v.capacity();
    std::mem::forget(v);
    let inner = Box::into_raw(Box::new(FxHashSet::<i64>::default()));
    let res = Box::into_raw(Box::new(OliveHashSet {
        kind: KIND_SET,
        ptr,
        cap: v_cap,
        len: 0,
        inner,
    })) as i64;
    register_object(res);
    res
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_set_add(set_ptr: i64, val: i64) {
    if set_ptr == 0 {
        return;
    }
    unsafe {
        let s = &mut *(set_ptr as *mut OliveHashSet);
        let hs = &mut *s.inner;
        if hs.insert(val) {
            let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
            v.push(val);
            s.ptr = v.as_mut_ptr();
            s.cap = v.capacity();
            s.len = v.len();
            std::mem::forget(v);
        }
    }
}
