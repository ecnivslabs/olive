use crate::*;

#[unsafe(no_mangle)]
pub extern "C" fn olive_struct_alloc(n_fields: i64) -> i64 {
    let total = (n_fields + 1) * 8;
    let layout = std::alloc::Layout::from_size_align(total as usize, 8).unwrap();
    let ptr = unsafe { std::alloc::alloc(layout) } as i64;
    unsafe { *(ptr as *mut i64) = n_fields };
    register_object(ptr);
    ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_struct(ptr: i64) {
    if ptr == 0 {
        return;
    }
    unregister_object(ptr);
    unsafe {
        let n_fields = *(ptr as *const i64);
        let total = ((n_fields + 1) * 8) as usize;
        let layout = std::alloc::Layout::from_size_align_unchecked(total, 8);
        std::alloc::dealloc(ptr as *mut u8, layout);
    }
}
