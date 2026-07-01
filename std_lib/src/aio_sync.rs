//! Channels, mutexes, and atomic ints for `aio`'s share-nothing concurrency
//! model -- split out of `aio.rs` to keep that file under the line-count
//! cap. No dependency on the executor/future machinery there; these are
//! plain OS-level primitives `lib/aio.liv`'s `Chan`/`Mutex` wrappers and
//! `atomic_*` functions call directly.

use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, AtomicI64, Ordering},
};

struct OliveChannel {
    queue: Mutex<std::collections::VecDeque<i64>>,
    cvar: Condvar,
    closed: AtomicBool,
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_chan_new() -> i64 {
    Box::into_raw(Box::new(OliveChannel {
        queue: Mutex::new(std::collections::VecDeque::new()),
        cvar: Condvar::new(),
        closed: AtomicBool::new(false),
    })) as i64
}

/// `val` must already be relocated into the shared escape arena by the
/// caller (`chan_send[T]` in `lib/aio.liv` does this via
/// `__olive_relocate_typed`, a compiler-recognized call the same way
/// `__olive_copy_typed` is). This function used to relocate `val` itself
/// via a runtime kind-tag guess on word 0, which is not sound for a struct
/// or closure record (word 0 is that value's own field count, which
/// routinely collides with an unrelated `KIND_*` constant).
#[unsafe(no_mangle)]
pub extern "C" fn olive_chan_send(chan: i64, val: i64) -> i64 {
    if chan == 0 {
        return 0;
    }
    let ch = unsafe { &*(chan as *const OliveChannel) };
    if ch.closed.load(Ordering::SeqCst) {
        return 0;
    }
    ch.queue.lock().unwrap().push_back(val);
    ch.cvar.notify_one();
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_chan_recv(chan: i64) -> i64 {
    if chan == 0 {
        return 0;
    }
    let ch = unsafe { &*(chan as *const OliveChannel) };
    let mut q = ch.queue.lock().unwrap();
    loop {
        if let Some(v) = q.pop_front() {
            return v;
        }
        if ch.closed.load(Ordering::SeqCst) {
            return 0;
        }
        q = ch.cvar.wait(q).unwrap();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_chan_try_recv(chan: i64) -> i64 {
    if chan == 0 {
        return i64::MIN;
    }
    let ch = unsafe { &*(chan as *const OliveChannel) };
    ch.queue.lock().unwrap().pop_front().unwrap_or(i64::MIN)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_chan_len(chan: i64) -> i64 {
    if chan == 0 {
        return 0;
    }
    let ch = unsafe { &*(chan as *const OliveChannel) };
    ch.queue.lock().unwrap().len() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_chan_close(chan: i64) {
    if chan == 0 {
        return;
    }
    let ch = unsafe { &*(chan as *const OliveChannel) };
    ch.closed.store(true, Ordering::SeqCst);
    ch.cvar.notify_all();
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_chan_free(chan: i64) {
    if chan != 0 {
        unsafe { drop(Box::from_raw(chan as *mut OliveChannel)) };
    }
}

struct OliveMutex {
    inner: Mutex<(bool, i64)>,
    cvar: Condvar,
}

/// See `olive_chan_send`: `val` must already be relocated by the caller.
#[unsafe(no_mangle)]
pub extern "C" fn olive_mutex_new(val: i64) -> i64 {
    Box::into_raw(Box::new(OliveMutex {
        inner: Mutex::new((false, val)),
        cvar: Condvar::new(),
    })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_mutex_lock(m: i64) -> i64 {
    if m == 0 {
        return 0;
    }
    let mx = unsafe { &*(m as *const OliveMutex) };
    let mut guard = mx.inner.lock().unwrap();
    while guard.0 {
        guard = mx.cvar.wait(guard).unwrap();
    }
    guard.0 = true;
    guard.1
}

/// See `olive_chan_send`: `new_val` must already be relocated by the caller.
#[unsafe(no_mangle)]
pub extern "C" fn olive_mutex_unlock(m: i64, new_val: i64) {
    if m == 0 {
        return;
    }
    let mx = unsafe { &*(m as *const OliveMutex) };
    let mut guard = mx.inner.lock().unwrap();
    guard.0 = false;
    guard.1 = new_val;
    mx.cvar.notify_one();
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_mutex_free(m: i64) {
    if m != 0 {
        unsafe { drop(Box::from_raw(m as *mut OliveMutex)) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_atomic_new(val: i64) -> i64 {
    Box::into_raw(Box::new(AtomicI64::new(val))) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_atomic_get(ptr: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    unsafe { &*(ptr as *const AtomicI64) }.load(Ordering::SeqCst)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_atomic_set(ptr: i64, val: i64) {
    if ptr == 0 {
        return;
    }
    unsafe { &*(ptr as *const AtomicI64) }.store(val, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_atomic_add(ptr: i64, delta: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    unsafe { &*(ptr as *const AtomicI64) }.fetch_add(delta, Ordering::SeqCst)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_atomic_cas(ptr: i64, expected: i64, new_val: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    let a = unsafe { &*(ptr as *const AtomicI64) };
    match a.compare_exchange(expected, new_val, Ordering::SeqCst, Ordering::SeqCst) {
        Ok(_) => 1,
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_atomic_free(ptr: i64) {
    if ptr != 0 {
        unsafe { drop(Box::from_raw(ptr as *mut AtomicI64)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chan_send_recv() {
        let ch = olive_chan_new();
        let val = crate::olive_str_internal("hello");
        assert_eq!(olive_chan_send(ch, val), 1);
        assert_eq!(olive_chan_len(ch), 1);
        let got = olive_chan_recv(ch);
        assert_eq!(crate::olive_str_from_ptr(got), "hello");
        assert_eq!(olive_chan_len(ch), 0);
        olive_chan_free(ch);
    }

    #[test]
    fn chan_try_recv_empty() {
        let ch = olive_chan_new();
        assert_eq!(olive_chan_try_recv(ch), i64::MIN);
        olive_chan_free(ch);
    }

    #[test]
    fn chan_close_unblocks_recv() {
        let ch = olive_chan_new();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            olive_chan_close(ch);
        });
        let result = olive_chan_recv(ch);
        assert_eq!(result, 0);
        handle.join().unwrap();
        olive_chan_free(ch);
    }

    #[test]
    fn chan_threaded_send_recv() {
        // `olive_chan_send` no longer relocates its argument itself (E5.6);
        // a caller crossing threads with a raw value must relocate first,
        // the same contract `chan_send[T]` (`lib/aio.liv`) follows via
        // `__olive_relocate_typed`.
        let ch = olive_chan_new();
        let handle = std::thread::spawn(move || {
            let v = crate::olive_str_internal("from thread");
            let desc = Box::leak(vec![crate::format::D_STR].into_boxed_slice()).as_ptr() as i64;
            let relocated = crate::copy_typed::olive_relocate_typed(v, desc);
            olive_chan_send(ch, relocated);
        });
        let got = olive_chan_recv(ch);
        assert_eq!(crate::olive_str_from_ptr(got), "from thread");
        handle.join().unwrap();
        olive_chan_free(ch);
    }

    #[test]
    fn mutex_lock_unlock() {
        let m = olive_mutex_new(42);
        let val = olive_mutex_lock(m);
        assert_eq!(val, 42);
        olive_mutex_unlock(m, 99);
        let val2 = olive_mutex_lock(m);
        assert_eq!(val2, 99);
        olive_mutex_unlock(m, 0);
        olive_mutex_free(m);
    }

    #[test]
    fn mutex_threaded() {
        let m = olive_mutex_new(0);
        let mut handles = vec![];
        for _ in 0..4 {
            handles.push(std::thread::spawn(move || {
                let v = olive_mutex_lock(m);
                olive_mutex_unlock(m, v + 1);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let final_val = olive_mutex_lock(m);
        assert_eq!(final_val, 4);
        olive_mutex_unlock(m, 0);
        olive_mutex_free(m);
    }

    #[test]
    fn atomic_get_set() {
        let a = olive_atomic_new(10);
        assert_eq!(olive_atomic_get(a), 10);
        olive_atomic_set(a, 20);
        assert_eq!(olive_atomic_get(a), 20);
        olive_atomic_free(a);
    }

    #[test]
    fn atomic_add() {
        let a = olive_atomic_new(0);
        let old = olive_atomic_add(a, 5);
        assert_eq!(old, 0);
        assert_eq!(olive_atomic_get(a), 5);
        olive_atomic_free(a);
    }

    #[test]
    fn atomic_cas() {
        let a = olive_atomic_new(1);
        assert_eq!(olive_atomic_cas(a, 1, 2), 1);
        assert_eq!(olive_atomic_get(a), 2);
        assert_eq!(olive_atomic_cas(a, 1, 3), 0);
        assert_eq!(olive_atomic_get(a), 2);
        olive_atomic_free(a);
    }

    #[test]
    fn atomic_threaded_increment() {
        let a = olive_atomic_new(0);
        let mut handles = vec![];
        for _ in 0..8 {
            handles.push(std::thread::spawn(move || {
                olive_atomic_add(a, 1);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(olive_atomic_get(a), 8);
        olive_atomic_free(a);
    }

    #[test]
    fn threaded_chan_send_list_copy() {
        let ch = olive_chan_new();
        let val = crate::olive_str_internal("from_main");
        assert_eq!(olive_chan_send(ch, val), 1);
        let got = olive_chan_recv(ch);
        assert_eq!(crate::olive_str_from_ptr(got), "from_main");
        assert_eq!(crate::olive_str_from_ptr(val), "from_main");
        olive_chan_free(ch);
    }

    #[test]
    fn threaded_mutex_roundtrip() {
        let m = olive_mutex_new(42);
        let handle = std::thread::spawn(move || {
            let v = olive_mutex_lock(m);
            assert_eq!(v, 42);
            olive_mutex_unlock(m, 99);
        });
        handle.join().unwrap();
        let v = olive_mutex_lock(m);
        assert_eq!(v, 99);
        olive_mutex_unlock(m, 0);
        olive_mutex_free(m);
    }
}
