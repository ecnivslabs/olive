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
/// Admission and the closed flag are serialized under the queue mutex so a
/// sender cannot slip an item in after close and a receiver cannot miss the
/// close notification between its predicate check and its condvar wait.
/// `val` arrives owned (moved, or an escape-arena copy the caller keeps the
/// original of); `desc` is its static type descriptor. A rejected value
/// (null channel, or closed channel) is released through `desc` instead of
/// stranded: shutdown races that keep sending after close would otherwise
/// leak one arena value per rejected send.
#[unsafe(no_mangle)]
pub extern "C" fn olive_chan_send(chan: i64, val: i64, desc: i64) -> i64 {
    if chan == 0 {
        crate::free_typed::olive_free_typed(val, desc);
        return 0;
    }
    let ch = unsafe { &*(chan as *const OliveChannel) };
    let mut q = ch.queue.lock().unwrap();
    if ch.closed.load(Ordering::SeqCst) {
        drop(q);
        crate::free_typed::olive_free_typed(val, desc);
        return 0;
    }
    q.push_back(val);
    drop(q);
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

/// The closed transition holds the queue mutex so it is atomic with respect
/// to the receiver predicate check and the sender admission check above.
#[unsafe(no_mangle)]
pub extern "C" fn olive_chan_close(chan: i64) {
    if chan == 0 {
        return;
    }
    let ch = unsafe { &*(chan as *const OliveChannel) };
    let _q = ch.queue.lock().unwrap();
    ch.closed.store(true, Ordering::SeqCst);
    ch.cvar.notify_all();
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_chan_free(chan: i64) {
    if chan == 0 {
        return;
    }
    // Queued words are owned solely by the channel (receivers pop them out
    // as transfers, and nothing else aliases them), so draining owns them
    // exactly once. Unlock before freeing: element drops run user `__drop__`
    // shims that must never execute under the queue lock.
    let pending = unsafe {
        let ch = &*(chan as *const OliveChannel);
        std::mem::take(&mut *ch.queue.lock().unwrap())
    };
    for val in pending {
        crate::free_any_word(val);
    }
    unsafe { drop(Box::from_raw(chan as *mut OliveChannel)) };
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
/// A null mutex cannot store it, so it is released through `desc` instead of
/// stranded, the same rejected-ownership discipline as `olive_chan_send`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_mutex_unlock(m: i64, new_val: i64, desc: i64) {
    if m == 0 {
        crate::free_typed::olive_free_typed(new_val, desc);
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

    fn int_desc() -> i64 {
        static DESC: [u8; 1] = [crate::format::D_INT];
        DESC.as_ptr() as i64
    }

    fn str_desc() -> i64 {
        static DESC: [u8; 1] = [crate::format::D_STR];
        DESC.as_ptr() as i64
    }

    #[test]
    fn rejected_send_releases_heap_string() {
        let ch = olive_chan_new();
        olive_chan_close(ch);
        let s = crate::olive_str_internal("rejected");
        let g = crate::string_slab::olive_str_gen_of(s);
        assert_eq!(olive_chan_send(ch, s, str_desc()), 0);
        assert_eq!(crate::string_slab::olive_str_gen_stale(s, g), 1);
        olive_chan_free(ch);
    }

    #[test]
    fn null_send_releases_heap_string() {
        let s = crate::olive_str_internal("null-send");
        let g = crate::string_slab::olive_str_gen_of(s);
        assert_eq!(olive_chan_send(0, s, str_desc()), 0);
        assert_eq!(crate::string_slab::olive_str_gen_stale(s, g), 1);
    }

    #[test]
    fn null_unlock_releases_heap_string() {
        let s = crate::olive_str_internal("null-unlock");
        let g = crate::string_slab::olive_str_gen_of(s);
        olive_mutex_unlock(0, s, str_desc());
        assert_eq!(crate::string_slab::olive_str_gen_stale(s, g), 1);
    }

    #[test]
    fn chan_send_recv() {
        let ch = olive_chan_new();
        let val = crate::olive_str_internal("hello");
        assert_eq!(olive_chan_send(ch, val, str_desc()), 1);
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
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let ch = olive_chan_new();
        let started = Arc::new(AtomicBool::new(false));
        let started_recv = started.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            started_recv.store(true, Ordering::SeqCst);
            let result = olive_chan_recv(ch);
            let _ = tx.send(result);
            olive_chan_free(ch);
        });
        while !started.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        olive_chan_close(ch);
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("recv must unblock on close");
        assert_eq!(result, 0);
        handle.join().unwrap();
    }

    #[test]
    fn chan_send_after_close_rejected() {
        let ch = olive_chan_new();
        assert_eq!(olive_chan_send(ch, 11, int_desc()), 1);
        olive_chan_close(ch);
        assert_eq!(olive_chan_send(ch, 22, int_desc()), 0);
        assert_eq!(olive_chan_len(ch), 1);
        assert_eq!(olive_chan_recv(ch), 11);
        assert_eq!(olive_chan_recv(ch), 0);
        assert_eq!(olive_chan_try_recv(ch), i64::MIN);
        olive_chan_free(ch);
    }

    #[test]
    fn chan_close_serialized_with_pending_items() {
        let ch = olive_chan_new();
        for v in [1, 2, 3] {
            assert_eq!(olive_chan_send(ch, v, int_desc()), 1);
        }
        olive_chan_close(ch);
        assert_eq!(olive_chan_send(ch, 4, int_desc()), 0);
        assert_eq!(olive_chan_recv(ch), 1);
        assert_eq!(olive_chan_recv(ch), 2);
        assert_eq!(olive_chan_recv(ch), 3);
        assert_eq!(olive_chan_recv(ch), 0);
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
            olive_chan_send(ch, relocated, str_desc());
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
        olive_mutex_unlock(m, 99, int_desc());
        let val2 = olive_mutex_lock(m);
        assert_eq!(val2, 99);
        olive_mutex_unlock(m, 0, int_desc());
        olive_mutex_free(m);
    }

    #[test]
    fn mutex_threaded() {
        let m = olive_mutex_new(0);
        let mut handles = vec![];
        for _ in 0..4 {
            handles.push(std::thread::spawn(move || {
                let v = olive_mutex_lock(m);
                olive_mutex_unlock(m, v + 1, int_desc());
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let final_val = olive_mutex_lock(m);
        assert_eq!(final_val, 4);
        olive_mutex_unlock(m, 0, int_desc());
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
        assert_eq!(olive_chan_send(ch, val, str_desc()), 1);
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
            olive_mutex_unlock(m, 99, int_desc());
        });
        handle.join().unwrap();
        let v = olive_mutex_lock(m);
        assert_eq!(v, 99);
        olive_mutex_unlock(m, 0, int_desc());
        olive_mutex_free(m);
    }

    #[test]
    fn free_with_pending_items_releases_them() {
        let ch = olive_chan_new();
        let a = crate::olive_str_internal("pending-a");
        let ga = crate::string_slab::olive_str_gen_of(a);
        let b = crate::olive_str_internal("pending-b");
        let gb = crate::string_slab::olive_str_gen_of(b);
        assert_eq!(olive_chan_send(ch, a, str_desc()), 1);
        assert_eq!(olive_chan_send(ch, b, str_desc()), 1);
        olive_chan_free(ch);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, ga), 1);
        assert_eq!(crate::string_slab::olive_str_gen_stale(b, gb), 1);
    }

    #[test]
    fn free_after_drain_leaves_nothing_to_release() {
        let ch = olive_chan_new();
        let a = crate::olive_str_internal("drained");
        let ga = crate::string_slab::olive_str_gen_of(a);
        assert_eq!(olive_chan_send(ch, a, str_desc()), 1);
        assert_eq!(crate::olive_str_from_ptr(olive_chan_recv(ch)), "drained");
        olive_chan_free(ch);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, ga), 0);
        crate::olive_free_str(a);
        assert_eq!(crate::string_slab::olive_str_gen_stale(a, ga), 1);
    }
}
