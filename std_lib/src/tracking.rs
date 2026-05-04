use rustc_hash::FxHashSet;
use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

const SHARDS: usize = 16;
static ACTIVE_OBJECTS: OnceLock<[std::sync::RwLock<FxHashSet<i64>>; SHARDS]> = OnceLock::new();

static IS_MULTITHREADED: AtomicBool = AtomicBool::new(false);
static MAIN_THREAD_ID: OnceLock<std::thread::ThreadId> = OnceLock::new();

thread_local! {
    static IS_MAIN_THREAD: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

#[inline]
fn check_multithreaded() -> bool {
    if IS_MULTITHREADED.load(Ordering::Relaxed) {
        return true;
    }
    let is_main = IS_MAIN_THREAD.with(|c| {
        if let Some(v) = c.get() {
            v
        } else {
            let main_id = MAIN_THREAD_ID.get_or_init(|| std::thread::current().id());
            let v = std::thread::current().id() == *main_id;
            c.set(Some(v));
            v
        }
    });
    if !is_main {
        IS_MULTITHREADED.store(true, Ordering::Relaxed);
        true
    } else {
        false
    }
}

static GLOBAL_MIN_PTR: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(i64::MAX);
static GLOBAL_MAX_PTR: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

const CACHE_SIZE: usize = 256;

thread_local! {
    static ACTIVE_CACHE: std::cell::UnsafeCell<[i64; CACHE_SIZE]> = const { std::cell::UnsafeCell::new([0; CACHE_SIZE]) };
    static LOCAL_ACTIVE_OBJECTS: RefCell<FxHashSet<i64>> = RefCell::new(FxHashSet::default());
}

#[inline]
fn get_shard(ptr: i64) -> usize {
    (ptr as usize >> 4) % SHARDS
}

/// Registers an object pointer for active tracking.
///
/// # Examples
///
/// ```
/// use olive_std::register_object;
/// register_object(8);
/// ```
pub fn register_object(ptr: i64) {
    if ptr != 0 {
        let mut current_min = GLOBAL_MIN_PTR.load(Ordering::Relaxed);
        while ptr < current_min {
            match GLOBAL_MIN_PTR.compare_exchange_weak(
                current_min,
                ptr,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_min = actual,
            }
        }
        let mut current_max = GLOBAL_MAX_PTR.load(Ordering::Relaxed);
        while ptr > current_max {
            match GLOBAL_MAX_PTR.compare_exchange_weak(
                current_max,
                ptr,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_max = actual,
            }
        }

        ACTIVE_CACHE.with(|c| {
            let cache = unsafe { &mut *c.get() };
            let idx = (ptr as usize >> 3) & (CACHE_SIZE - 1);
            cache[idx] = ptr;
        });

        LOCAL_ACTIVE_OBJECTS.with(|cache| cache.borrow_mut().insert(ptr));
        if check_multithreaded() {
            let shards = ACTIVE_OBJECTS.get_or_init(|| {
                std::array::from_fn(|_| std::sync::RwLock::new(FxHashSet::default()))
            });
            shards[get_shard(ptr)].write().unwrap().insert(ptr);
        }
    }
}

/// Unregisters an object pointer from active tracking.
///
/// # Examples
///
/// ```
/// use olive_std::{register_object, unregister_object};
/// register_object(8);
/// unregister_object(8);
/// ```
pub fn unregister_object(ptr: i64) {
    if ptr != 0 {
        ACTIVE_CACHE.with(|c| {
            let cache = unsafe { &mut *c.get() };
            let idx = (ptr as usize >> 3) & (CACHE_SIZE - 1);
            if cache[idx] == ptr {
                cache[idx] = 0;
            }
        });

        LOCAL_ACTIVE_OBJECTS.with(|cache| cache.borrow_mut().remove(&ptr));
        if check_multithreaded()
            && let Some(shards) = ACTIVE_OBJECTS.get()
        {
            shards[get_shard(ptr)].write().unwrap().remove(&ptr);
        }
    }
}

/// Returns `true` if the pointer is currently registered as an active object.
///
/// # Examples
///
/// ```
/// use olive_std::{register_object, unregister_object, is_active_object};
/// register_object(8);
/// assert!(is_active_object(8));
/// unregister_object(8);
/// assert!(!is_active_object(8));
/// ```
pub fn is_active_object(ptr: i64) -> bool {
    if ptr == 0 {
        return false;
    }
    if (ptr & 7) != 0 {
        return false;
    }
    if ptr < GLOBAL_MIN_PTR.load(Ordering::Relaxed) || ptr > GLOBAL_MAX_PTR.load(Ordering::Relaxed)
    {
        return false;
    }

    let cache_hit = ACTIVE_CACHE.with(|c| {
        let cache = unsafe { &*c.get() };
        let idx = (ptr as usize >> 3) & (CACHE_SIZE - 1);
        cache[idx] == ptr
    });
    if cache_hit {
        return true;
    }

    let in_local = LOCAL_ACTIVE_OBJECTS.with(|cache| cache.borrow().contains(&ptr));
    if in_local {
        ACTIVE_CACHE.with(|c| {
            let cache = unsafe { &mut *c.get() };
            let idx = (ptr as usize >> 3) & (CACHE_SIZE - 1);
            cache[idx] = ptr;
        });
        return true;
    }

    if check_multithreaded()
        && let Some(shards) = ACTIVE_OBJECTS.get()
    {
        let in_global = shards[get_shard(ptr)].read().unwrap().contains(&ptr);
        if in_global {
            ACTIVE_CACHE.with(|c| {
                let cache = unsafe { &mut *c.get() };
                let idx = (ptr as usize >> 3) & (CACHE_SIZE - 1);
                cache[idx] = ptr;
            });
            return true;
        }
    }
    false
}

/// Returns the number of currently active registered objects.
///
/// # Examples
///
/// ```
/// use olive_std::{register_object, active_objects_count};
/// let before = active_objects_count();
/// register_object(8);
/// assert_eq!(active_objects_count(), before + 1);
/// ```
pub fn active_objects_count() -> usize {
    if check_multithreaded() {
        if let Some(shards) = ACTIVE_OBJECTS.get() {
            shards.iter().map(|shard| shard.read().unwrap().len()).sum()
        } else {
            0
        }
    } else {
        LOCAL_ACTIVE_OBJECTS.with(|cache| cache.borrow().len())
    }
}
