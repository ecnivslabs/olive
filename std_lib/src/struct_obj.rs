use std::cell::UnsafeCell;

const MAX_CACHED_FIELDS: usize = 8;
const FREE_LIST_CAP: usize = 256;

struct FreeList {
    heads: [*mut u8; MAX_CACHED_FIELDS],
    counts: [usize; MAX_CACHED_FIELDS],
}

impl FreeList {
    const fn new() -> Self {
        Self {
            heads: [std::ptr::null_mut(); MAX_CACHED_FIELDS],
            counts: [0usize; MAX_CACHED_FIELDS],
        }
    }
}

thread_local! {
    static STRUCT_FREE_LIST: UnsafeCell<FreeList> = UnsafeCell::new(FreeList::new());
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_struct_alloc(n_fields: i64) -> i64 {
    let idx = (n_fields as usize).wrapping_sub(1);
    if idx < MAX_CACHED_FIELDS {
        let ptr = STRUCT_FREE_LIST.with(|fl| {
            let fl = unsafe { &mut *fl.get() };
            let head = fl.heads[idx];
            if !head.is_null() {
                let next = unsafe { *(head as *const *mut u8) };
                fl.heads[idx] = next;
                fl.counts[idx] -= 1;
                head as i64
            } else {
                0i64
            }
        });
        if ptr != 0 {
            unsafe { *(ptr as *mut i64) = n_fields };
            return ptr;
        }
    }
    let total = (n_fields + 1) * 8;
    let layout = std::alloc::Layout::from_size_align(total as usize, 8).unwrap();
    let ptr = unsafe { std::alloc::alloc(layout) } as i64;
    unsafe { *(ptr as *mut i64) = n_fields };
    ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_struct(ptr: i64) {
    if ptr == 0 {
        return;
    }
    let n_fields = unsafe { *(ptr as *const i64) };
    let idx = (n_fields as usize).wrapping_sub(1);
    if idx < MAX_CACHED_FIELDS {
        let returned = STRUCT_FREE_LIST.with(|fl| {
            let fl = unsafe { &mut *fl.get() };
            if fl.counts[idx] < FREE_LIST_CAP {
                let raw = ptr as *mut *mut u8;
                unsafe { *raw = fl.heads[idx] };
                fl.heads[idx] = ptr as *mut u8;
                fl.counts[idx] += 1;
                true
            } else {
                false
            }
        });
        if returned {
            return;
        }
    }
    unsafe {
        let total = ((n_fields + 1) * 8) as usize;
        let layout = std::alloc::Layout::from_size_align_unchecked(total, 8);
        std::alloc::dealloc(ptr as *mut u8, layout);
    }
}
