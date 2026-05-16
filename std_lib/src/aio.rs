const KIND_FUTURE: i64 = 4;
const KIND_SM_FUTURE: i64 = 5;
const KIND_LIST: i64 = 1;
const POLL_PENDING: i64 = i64::MIN;

use crate::StableVec;
use std::collections::VecDeque;
use std::sync::{
    Arc, Condvar, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

struct OliveTask {
    sm_future: i64,
    queued: AtomicBool,
    done: AtomicBool,
    completions: Mutex<Vec<Arc<Completion>>>,
    sm_waiters: Mutex<Vec<Arc<OliveTask>>>,
}

struct Completion {
    result: Mutex<Option<i64>>,
    cvar: Condvar,
}

struct OliveExecutor {
    ready: Mutex<VecDeque<Arc<OliveTask>>>,
    wakeup: Condvar,
    task_map: Mutex<std::collections::HashMap<i64, Arc<OliveTask>>>,
    shutdown: AtomicBool,
}

static EXECUTOR: OnceLock<Arc<OliveExecutor>> = OnceLock::new();

fn olive_executor() -> &'static Arc<OliveExecutor> {
    EXECUTOR.get_or_init(|| {
        let ex = Arc::new(OliveExecutor {
            ready: Mutex::new(VecDeque::new()),
            wakeup: Condvar::new(),
            task_map: Mutex::new(std::collections::HashMap::new()),
            shutdown: AtomicBool::new(false),
        });
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        for _ in 0..n {
            let ex2 = ex.clone();
            std::thread::Builder::new()
                .name("olive-executor".into())
                .spawn(move || executor_worker(ex2))
                .unwrap();
        }
        ex
    })
}

fn executor_worker(ex: Arc<OliveExecutor>) {
    loop {
        if ex.shutdown.load(Ordering::Relaxed) {
            return;
        }
        let task = {
            let mut q = ex.ready.lock().unwrap();
            loop {
                if let Some(t) = q.pop_front() {
                    break t;
                }
                q = ex.wakeup.wait(q).unwrap();
                if ex.shutdown.load(Ordering::Relaxed) {
                    return;
                }
            }
        };
        task.queued.store(false, Ordering::SeqCst);
        executor_drive(&ex, task);
    }
}

fn executor_enqueue(ex: &OliveExecutor, task: Arc<OliveTask>) {
    if task
        .queued
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        ex.ready.lock().unwrap().push_back(task);
        ex.wakeup.notify_one();
    }
}

fn executor_get_or_create_task(ex: &OliveExecutor, sm_future_ptr: i64) -> Arc<OliveTask> {
    let mut map = ex.task_map.lock().unwrap();
    if let Some(t) = map.get(&sm_future_ptr) {
        return t.clone();
    }
    let t = Arc::new(OliveTask {
        sm_future: sm_future_ptr,
        queued: AtomicBool::new(false),
        done: AtomicBool::new(false),
        completions: Mutex::new(Vec::new()),
        sm_waiters: Mutex::new(Vec::new()),
    });
    map.insert(sm_future_ptr, t.clone());
    t
}

fn executor_drive(ex: &Arc<OliveExecutor>, task: Arc<OliveTask>) {
    let sf = unsafe { &*(task.sm_future as *const OliveSmFuture) };
    let poll_fn: fn(i64) -> i64 = unsafe { std::mem::transmute(sf.poll_fn as usize) };
    let result = poll_fn(sf.frame);

    if result != POLL_PENDING {
        task.done.store(true, Ordering::SeqCst);
        let comps = std::mem::take(&mut *task.completions.lock().unwrap());
        for c in &comps {
            *c.result.lock().unwrap() = Some(result);
            c.cvar.notify_all();
        }
        let waiters = std::mem::take(&mut *task.sm_waiters.lock().unwrap());
        for w in waiters {
            executor_enqueue(ex, w);
        }
        ex.task_map.lock().unwrap().remove(&task.sm_future);
        return;
    }

    let sub_future = unsafe { *((sf.frame + 8) as *const i64) };
    if sub_future == 0 {
        executor_enqueue(ex, task);
        return;
    }

    let sub_kind = unsafe { *(sub_future as *const i64) };
    if sub_kind == KIND_FUTURE {
        let sf_obj = unsafe { &*(sub_future as *const OliveFuture) };
        let shared = unsafe { Arc::from_raw(sf_obj.shared as *const FutureShared) };
        let shared2 = shared.clone();
        std::mem::forget(shared); // keep ref-count balanced
        let ex2 = ex.clone();
        std::thread::Builder::new()
            .name("olive-waker".into())
            .spawn(move || {
                let mut st = shared2.state.lock().unwrap();
                loop {
                    match &*st {
                        FutureState::Ready(_) => break,
                        FutureState::Pending => {
                            st = shared2.cvar.wait(st).unwrap();
                        }
                    }
                }
                drop(st);
                executor_enqueue(&ex2, task);
            })
            .unwrap();
    } else if sub_kind == KIND_SM_FUTURE {
        let sub_task = executor_get_or_create_task(ex, sub_future);
        sub_task.sm_waiters.lock().unwrap().push(task.clone());
        if sub_task.done.load(Ordering::SeqCst) {
            sub_task
                .sm_waiters
                .lock()
                .unwrap()
                .retain(|t| !Arc::ptr_eq(t, &task));
            executor_enqueue(ex, task);
        } else {
            executor_enqueue(ex, sub_task);
        }
    } else {
        executor_enqueue(ex, task);
    }
}

#[repr(C)]
struct OliveSmFuture {
    kind: i64,
    poll_fn: i64,
    frame: i64,
    cancelled: i64,
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sm_poll(future: i64) -> i64 {
    if future == 0 {
        return 0;
    }
    let kind = unsafe { *(future as *const i64) };
    if kind == KIND_SM_FUTURE {
        let f = unsafe { &*(future as *const OliveSmFuture) };
        let poll_fn: fn(i64) -> i64 = unsafe { std::mem::transmute(f.poll_fn as usize) };
        poll_fn(f.frame)
    } else {
        let f = unsafe { &*(future as *const OliveFuture) };
        let shared = unsafe { &*(f.shared as *const FutureShared) };
        let guard = shared.state.lock().unwrap();
        match &*guard {
            FutureState::Ready(v) => *v,
            FutureState::Pending => POLL_PENDING,
        }
    }
}

enum FutureState {
    Pending,
    Ready(i64),
}

struct FutureShared {
    state: Mutex<FutureState>,
    cvar: Condvar,
}

#[repr(C)]
struct OliveFuture {
    kind: i64,
    shared: i64, // raw ptr into Arc<FutureShared>
}

fn call_jit_fn(fn_ptr: usize, args: &[i64]) -> i64 {
    unsafe {
        match args.len() {
            0 => {
                let f: extern "C" fn() -> i64 = std::mem::transmute(fn_ptr);
                f()
            }
            1 => {
                let f: extern "C" fn(i64) -> i64 = std::mem::transmute(fn_ptr);
                f(args[0])
            }
            2 => {
                let f: extern "C" fn(i64, i64) -> i64 = std::mem::transmute(fn_ptr);
                f(args[0], args[1])
            }
            3 => {
                let f: extern "C" fn(i64, i64, i64) -> i64 = std::mem::transmute(fn_ptr);
                f(args[0], args[1], args[2])
            }
            4 => {
                let f: extern "C" fn(i64, i64, i64, i64) -> i64 = std::mem::transmute(fn_ptr);
                f(args[0], args[1], args[2], args[3])
            }
            5 => {
                let f: extern "C" fn(i64, i64, i64, i64, i64) -> i64 = std::mem::transmute(fn_ptr);
                f(args[0], args[1], args[2], args[3], args[4])
            }
            6 => {
                let f: extern "C" fn(i64, i64, i64, i64, i64, i64) -> i64 =
                    std::mem::transmute(fn_ptr);
                f(args[0], args[1], args[2], args[3], args[4], args[5])
            }
            7 => {
                let f: extern "C" fn(i64, i64, i64, i64, i64, i64, i64) -> i64 =
                    std::mem::transmute(fn_ptr);
                f(
                    args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                )
            }
            8 => {
                let f: extern "C" fn(i64, i64, i64, i64, i64, i64, i64, i64) -> i64 =
                    std::mem::transmute(fn_ptr);
                f(
                    args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7],
                )
            }
            _ => panic!("async fn: too many arguments (max 8)"),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_make_future(val: i64) -> i64 {
    let shared = Arc::new(FutureShared {
        state: Mutex::new(FutureState::Ready(val)),
        cvar: Condvar::new(),
    });
    let f = Box::new(OliveFuture {
        kind: KIND_FUTURE,
        shared: Arc::into_raw(shared) as i64,
    });
    Box::into_raw(f) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_await_future(future: i64) -> i64 {
    if future == 0 {
        return 0;
    }
    let kind = unsafe { *(future as *const i64) };
    if kind == KIND_SM_FUTURE {
        let completion = Arc::new(Completion {
            result: Mutex::new(None),
            cvar: Condvar::new(),
        });
        let ex = olive_executor();
        let task = executor_get_or_create_task(ex, future);
        task.completions.lock().unwrap().push(completion.clone());
        executor_enqueue(ex, task);
        let mut r = completion.result.lock().unwrap();
        loop {
            match *r {
                Some(v) => return v,
                None => r = completion.cvar.wait(r).unwrap(),
            }
        }
    } else {
        let f = unsafe { &*(future as *const OliveFuture) };
        let shared = unsafe { Arc::from_raw(f.shared as *const FutureShared) };
        let result = {
            let mut state = shared.state.lock().unwrap();
            loop {
                match &*state {
                    FutureState::Ready(v) => break *v,
                    FutureState::Pending => {
                        state = shared.cvar.wait(state).unwrap();
                    }
                }
            }
        };
        std::mem::forget(shared);
        result
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_spawn_task(callback: i64) -> i64 {
    let cb = callback as *const i64;
    let fn_ptr = unsafe { *cb } as usize;
    let nargs = unsafe { *cb.add(1) } as usize;
    let args: Vec<i64> = (0..nargs).map(|i| unsafe { *cb.add(2 + i) }).collect();
    unsafe {
        let layout = std::alloc::Layout::from_size_align(8 * (2 + nargs), 8).unwrap();
        std::alloc::dealloc(callback as *mut u8, layout);
    }

    let shared = Arc::new(FutureShared {
        state: Mutex::new(FutureState::Pending),
        cvar: Condvar::new(),
    });
    let shared2 = shared.clone();

    std::thread::spawn(move || {
        let result = call_jit_fn(fn_ptr, &args);
        let mut state = shared2.state.lock().unwrap();
        *state = FutureState::Ready(result);
        shared2.cvar.notify_all();
    });

    let f = Box::new(OliveFuture {
        kind: KIND_FUTURE,
        shared: Arc::into_raw(shared) as i64,
    });
    Box::into_raw(f) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_free_future(future: i64) -> i64 {
    if future == 0 {
        return 0;
    }
    let f = unsafe { Box::from_raw(future as *mut OliveFuture) };
    unsafe { Arc::from_raw(f.shared as *const FutureShared) };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_async_file_read(path: i64) -> i64 {
    let path_str = if path == 0 {
        String::new()
    } else {
        let ptr = (path & !1) as *const std::ffi::c_char;
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    };

    let shared = Arc::new(FutureShared {
        state: Mutex::new(FutureState::Pending),
        cvar: Condvar::new(),
    });
    let shared2 = shared.clone();

    std::thread::spawn(move || {
        let result = match std::fs::read_to_string(&path_str) {
            Ok(content) => {
                let mut bytes = content.into_bytes();
                bytes.push(0);
                let ptr = Box::into_raw(bytes.into_boxed_slice()) as *mut u8;
                (ptr as i64) | 1
            }
            Err(_) => 0,
        };
        let mut state = shared2.state.lock().unwrap();
        *state = FutureState::Ready(result);
        shared2.cvar.notify_all();
    });

    let f = Box::new(OliveFuture {
        kind: KIND_FUTURE,
        shared: Arc::into_raw(shared) as i64,
    });
    Box::into_raw(f) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_async_file_write(path: i64, data: i64) -> i64 {
    let path_str = if path == 0 {
        String::new()
    } else {
        let ptr = (path & !1) as *const std::ffi::c_char;
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    };
    let data_str = if data == 0 {
        String::new()
    } else {
        let ptr = (data & !1) as *const std::ffi::c_char;
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    };

    let shared = Arc::new(FutureShared {
        state: Mutex::new(FutureState::Pending),
        cvar: Condvar::new(),
    });
    let shared2 = shared.clone();

    std::thread::spawn(move || {
        let result = match std::fs::write(&path_str, data_str.as_bytes()) {
            Ok(_) => 0i64,
            Err(_) => -1i64,
        };
        let mut state = shared2.state.lock().unwrap();
        *state = FutureState::Ready(result);
        shared2.cvar.notify_all();
    });

    let f = Box::new(OliveFuture {
        kind: KIND_FUTURE,
        shared: Arc::into_raw(shared) as i64,
    });
    Box::into_raw(f) as i64
}

#[repr(C)]
struct GatherFrame {
    state: i64,
    awaiting_list: i64,
    futures_list: i64,
    results: i64,
    done: usize,
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_gather_poll(frame: i64) -> i64 {
    let f = unsafe { &mut *(frame as *mut GatherFrame) };
    if f.state == -1 {
        return f.results;
    }

    let list = unsafe { &*(f.futures_list as *const StableVec) };
    let n = list.len;
    let results_vec = unsafe { &*(f.results as *const StableVec) };
    let results = unsafe { std::slice::from_raw_parts_mut(results_vec.ptr, n) };

    let mut any_pending = false;
    for (i, res) in results.iter_mut().enumerate().take(n) {
        if *res == POLL_PENDING {
            let fut = unsafe { *list.ptr.add(i) };
            let r = olive_sm_poll(fut);
            if r != POLL_PENDING {
                *res = r;
                f.done += 1;
            } else {
                any_pending = true;
            }
        }
    }

    if any_pending {
        f.awaiting_list = f.futures_list;
        POLL_PENDING
    } else {
        f.state = -1;
        f.results
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_gather(futures_list: i64) -> i64 {
    if futures_list == 0 {
        let v = Box::new(StableVec {
            kind: KIND_LIST,
            ptr: std::ptr::null_mut(),
            cap: 0,
            len: 0,
        });
        return Box::into_raw(v) as i64;
    }
    let list = unsafe { &*(futures_list as *const StableVec) };
    let n = list.len;

    let mut res_vec = vec![POLL_PENDING; n];
    let ptr = res_vec.as_mut_ptr();
    let cap = res_vec.capacity();
    let len = res_vec.len();
    std::mem::forget(res_vec);

    let results_list = Box::into_raw(Box::new(StableVec {
        kind: KIND_LIST,
        ptr,
        cap,
        len,
    })) as i64;

    let frame = Box::into_raw(Box::new(GatherFrame {
        state: 0,
        awaiting_list: 0,
        futures_list,
        results: results_list,
        done: 0,
    })) as i64;

    Box::into_raw(Box::new(OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: olive_gather_poll as *const () as usize as i64,
        frame,
        cancelled: 0,
    })) as i64
}

#[repr(C)]
struct SelectFrame {
    state: i64,
    awaiting_list: i64,
    futures_list: i64,
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_select_poll(frame: i64) -> i64 {
    let f = unsafe { &mut *(frame as *mut SelectFrame) };
    let list = unsafe { &*(f.futures_list as *const StableVec) };
    let n = list.len;

    for i in 0..n {
        let fut = unsafe { *list.ptr.add(i) };
        let r = olive_sm_poll(fut);
        if r != POLL_PENDING {
            let mut res_vec = vec![i as i64, r];
            let ptr = res_vec.as_mut_ptr();
            let cap = res_vec.capacity();
            let len = res_vec.len();
            std::mem::forget(res_vec);
            return Box::into_raw(Box::new(StableVec {
                kind: KIND_LIST,
                ptr,
                cap,
                len,
            })) as i64;
        }
    }
    f.awaiting_list = f.futures_list;
    POLL_PENDING
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_select(futures_list: i64) -> i64 {
    if futures_list == 0 {
        return 0;
    }
    let frame = Box::into_raw(Box::new(SelectFrame {
        state: 0,
        awaiting_list: 0,
        futures_list,
    })) as i64;
    Box::into_raw(Box::new(OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: olive_select_poll as *const () as usize as i64,
        frame,
        cancelled: 0,
    })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_cancel_future(future: i64) -> i64 {
    if future == 0 {
        return 0;
    }
    let kind = unsafe { *(future as *const i64) };
    if kind == KIND_SM_FUTURE {
        let f = unsafe { &mut *(future as *mut OliveSmFuture) };
        f.cancelled = 1;
    }
    0
}

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

use std::sync::atomic::AtomicI64;

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

#[unsafe(no_mangle)]
pub extern "C" fn olive_pool_size() -> i64 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i64)
        .unwrap_or(4)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_pool_run(fn_ptr: i64, arg: i64) -> i64 {
    if fn_ptr == 0 {
        return 0;
    }
    let shared = Arc::new(FutureShared {
        state: Mutex::new(FutureState::Pending),
        cvar: Condvar::new(),
    });
    let shared2 = shared.clone();
    std::thread::spawn(move || {
        let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(fn_ptr as usize) };
        let result = f(arg);
        let mut state = shared2.state.lock().unwrap();
        *state = FutureState::Ready(result);
        shared2.cvar.notify_all();
    });
    Box::into_raw(Box::new(OliveFuture {
        kind: KIND_FUTURE,
        shared: Arc::into_raw(shared) as i64,
    })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_pool_run_sync(fn_ptr: i64, arg: i64) -> i64 {
    if fn_ptr == 0 {
        return 0;
    }
    let shared = Arc::new(FutureShared {
        state: Mutex::new(FutureState::Pending),
        cvar: Condvar::new(),
    });
    let shared2 = shared.clone();
    std::thread::spawn(move || {
        let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(fn_ptr as usize) };
        let result = f(arg);
        let mut state = shared2.state.lock().unwrap();
        *state = FutureState::Ready(result);
        shared2.cvar.notify_all();
    });
    let mut state = shared.state.lock().unwrap();
    loop {
        if let FutureState::Ready(val) = *state {
            return val;
        }
        state = shared.cvar.wait(state).unwrap();
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
        let ch = olive_chan_new();
        let handle = std::thread::spawn(move || {
            let v = crate::olive_str_internal("from thread");
            olive_chan_send(ch, v);
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
    fn pool_size_positive() {
        assert!(olive_pool_size() >= 1);
    }

    extern "C" fn add_one(x: i64) -> i64 {
        x + 1
    }

    #[test]
    fn pool_run_sync_executes() {
        let result = olive_pool_run_sync(add_one as *const () as i64, 41);
        assert_eq!(result, 42);
    }
}
