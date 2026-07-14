const KIND_FUTURE: i64 = 4;
const KIND_SM_FUTURE: i64 = 5;
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
    driving: AtomicBool,
    done: AtomicBool,
    pending_result: Mutex<Option<i64>>,
    completions: Mutex<Vec<Arc<Completion>>>,
    sm_waiters: Mutex<Vec<Arc<OliveTask>>>,
    slabs: Mutex<Option<Box<crate::slab::SlabSet>>>,
}

struct Completion {
    result: Mutex<Option<i64>>,
    cvar: Condvar,
}

struct OliveExecutor {
    ready: Mutex<VecDeque<Arc<OliveTask>>>,
    wakeup: Condvar,
    task_map: Mutex<std::collections::HashMap<i64, Arc<OliveTask>>>,
}

#[derive(PartialEq, Eq)]
enum DriveOutcome {
    Completed,
    Rerun,
    Parked,
}

static EXECUTOR: OnceLock<Arc<OliveExecutor>> = OnceLock::new();

fn olive_executor() -> &'static Arc<OliveExecutor> {
    EXECUTOR.get_or_init(|| {
        let ex = Arc::new(OliveExecutor {
            ready: Mutex::new(VecDeque::new()),
            wakeup: Condvar::new(),
            task_map: Mutex::new(std::collections::HashMap::new()),
        });
        // One worker per CPU. Workers block in `poll_fn` while a machine runs
        // and park on `wakeup` otherwise, so this is the concurrency ceiling
        // for state machines; blocking syscalls inside an async body still
        // pin a worker, but polling itself never spawns extra threads.
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        for _ in 0..n {
            let ex2 = ex.clone();
            crate::debug::spawn_traced("olive-executor", move || executor_worker(ex2));
        }
        ex
    })
}

fn executor_worker(ex: Arc<OliveExecutor>) {
    loop {
        let task = {
            let mut q = ex.ready.lock().unwrap();
            loop {
                if let Some(t) = q.pop_front() {
                    break t;
                }
                q = ex.wakeup.wait(q).unwrap();
            }
        };
        task.queued.store(false, Ordering::SeqCst);
        let delivered = task.pending_result.lock().unwrap().take();
        if let Some(v) = delivered {
            executor_complete_waker(&ex, &task, v);
        } else {
            match executor_drive(&ex, &task) {
                DriveOutcome::Completed | DriveOutcome::Parked => {}
                DriveOutcome::Rerun => {
                    executor_enqueue(&ex, &task);
                }
            }
        }
    }
}

/// Resumes a parent machine whose awaited sub-machine finished while the
/// parent sat queued, delivering its cached result word. The sub-task already
/// relocated that word out of its own arena before dropping it.
fn executor_enqueue(ex: &OliveExecutor, task: &Arc<OliveTask>) -> bool {
    if task
        .queued
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        ex.ready.lock().unwrap().push_back(task.clone());
        ex.wakeup.notify_one();
        true
    } else {
        false
    }
}

fn executor_get_or_create_task(ex: &OliveExecutor, sm_future_ptr: i64) -> Arc<OliveTask> {
    let mut map = ex.task_map.lock().unwrap();
    if let Some(t) = map.get(&sm_future_ptr) {
        return t.clone();
    }
    // Caller must `std::mem::forget` the returned handle unless it stores it:
    // these Arcs live in `task_map`, so a plain drop would decrement toward
    // an early free while the map still holds its clone.
    let t = Arc::new(OliveTask {
        sm_future: sm_future_ptr,
        queued: AtomicBool::new(false),
        driving: AtomicBool::new(false),
        done: AtomicBool::new(false),
        pending_result: Mutex::new(None),
        completions: Mutex::new(Vec::new()),
        sm_waiters: Mutex::new(Vec::new()),
        slabs: Mutex::new(None),
    });
    map.insert(sm_future_ptr, t.clone());
    t
}

/// Drives one poll of `task` on the calling worker. The `driving` flag makes a
/// poll exclusive: two workers can otherwise poll the same machine at once (a
/// completion handoff enqueues the awaiter while it is still mid-poll), and
/// the generated poll is not reentrant — both would resume from the same
/// saved state and corrupt the frame. The loser of the flag re-enqueues; the
/// winner clears the flag before parking or completing so no wakeup is lost.
fn executor_drive(ex: &Arc<OliveExecutor>, task: &Arc<OliveTask>) -> DriveOutcome {
    if task
        .driving
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return DriveOutcome::Rerun;
    }

    let slabs_ptr = {
        let mut slabs_guard = task.slabs.lock().unwrap();
        if slabs_guard.is_none() {
            *slabs_guard = Some(Box::new(crate::slab::SlabSet::new()));
        }
        slabs_guard.as_mut().unwrap().as_mut() as *mut crate::slab::SlabSet
    };
    let old_active = crate::slab::ACTIVE_SLABS.get();
    crate::slab::ACTIVE_SLABS.set(slabs_ptr);

    let sf = unsafe { &*(task.sm_future as *const OliveSmFuture) };
    let poll_fn: fn(i64) -> i64 = unsafe { std::mem::transmute(sf.poll_fn as usize) };
    let result = poll_fn(sf.frame);

    crate::slab::ACTIVE_SLABS.set(old_active);
    task.driving.store(false, Ordering::SeqCst);

    if result != POLL_PENDING {
        return executor_complete(ex, task, result);
    }
    park_after_pending(ex, task, sf)
}

/// Resumes a parent machine whose awaited sub-machine finished while the
/// parent sat queued, delivering the sub-task's cached result word (already
/// relocated out of the sub-task's arena before it was dropped). The resumed
/// poll may run to completion or park on a further await; both are handled.
fn executor_complete_waker(ex: &Arc<OliveExecutor>, task: &Arc<OliveTask>, result: i64) {
    if task
        .driving
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // Mid-poll elsewhere; restore and let that poll's worker pick it up
        // on its next dequeue.
        *task.pending_result.lock().unwrap() = Some(result);
        return;
    }

    let slabs_ptr = {
        let mut slabs_guard = task.slabs.lock().unwrap();
        if slabs_guard.is_none() {
            *slabs_guard = Some(Box::new(crate::slab::SlabSet::new()));
        }
        slabs_guard.as_mut().unwrap().as_mut() as *mut crate::slab::SlabSet
    };
    let old_active = crate::slab::ACTIVE_SLABS.get();
    crate::slab::ACTIVE_SLABS.set(slabs_ptr);

    let sf = unsafe { &*(task.sm_future as *const OliveSmFuture) };
    let poll_fn: fn(i64) -> i64 = unsafe { std::mem::transmute(sf.poll_fn as usize) };
    let final_result = poll_fn(sf.frame);

    crate::slab::ACTIVE_SLABS.set(old_active);
    task.driving.store(false, Ordering::SeqCst);

    if final_result != POLL_PENDING {
        executor_complete(ex, task, final_result);
        return;
    }
    park_after_pending(ex, task, sf);
}

/// Parks a task whose poll just returned Pending: blocks on a plain future's
/// condvar, registers as a waiter on a sub state-machine, or re-enqueues.
/// Awaiting a `KIND_FUTURE` blocks this worker, which is safe because its
/// producer is always a non-executor thread (`olive_spawn_task`, pool, file
/// IO); the old runtime instead spawned one OS thread per suspension.
fn park_after_pending(
    ex: &Arc<OliveExecutor>,
    task: &Arc<OliveTask>,
    sf: &OliveSmFuture,
) -> DriveOutcome {
    let sub_future = unsafe { *((sf.frame + 8) as *const i64) };
    if sub_future == 0 {
        return DriveOutcome::Rerun;
    }

    let sub_kind = unsafe { *(sub_future as *const i64) };
    match sub_kind {
        KIND_FUTURE => {
            let sf_obj = unsafe { &*(sub_future as *const OliveFuture) };
            // Rebuild the Arc without changing the refcount: the raw pointer
            // is the one live reference owned by the OliveFuture itself.
            let shared = unsafe { Arc::from_raw(sf_obj.shared as *const FutureShared) };
            {
                let mut st = shared.state.lock().unwrap();
                loop {
                    match &*st {
                        FutureState::Ready(_) => break,
                        FutureState::Pending => st = shared.cvar.wait(st).unwrap(),
                    }
                }
            }
            std::mem::forget(shared);
            DriveOutcome::Rerun
        }
        KIND_SM_FUTURE => {
            let sub_task = executor_get_or_create_task(ex, sub_future);
            // Already finished but its wakeup has not been consumed yet.
            let delivered = sub_task.pending_result.lock().unwrap().take();
            if let Some(v) = delivered {
                std::mem::forget(sub_task);
                *task.pending_result.lock().unwrap() = Some(v);
                return DriveOutcome::Rerun;
            }
            // Check-and-push under the waiters lock, mirrored by the done
            // store inside the same lock in `executor_complete`: checking
            // done outside that lock races completion's take and the waiter
            // is orphaned, hanging the parent forever. A duplicate push is
            // refused the same way: one outstanding registration per task.
            let mut waiters = sub_task.sm_waiters.lock().unwrap();
            if sub_task.done.load(Ordering::SeqCst) || waiters.iter().any(|w| Arc::ptr_eq(w, task))
            {
                drop(waiters);
                std::mem::forget(sub_task);
                return DriveOutcome::Rerun;
            }
            waiters.push(task.clone());
            drop(waiters);
            executor_enqueue(ex, &sub_task);
            std::mem::forget(sub_task);
            DriveOutcome::Parked
        }
        _ => DriveOutcome::Rerun,
    }
}

fn executor_complete(ex: &Arc<OliveExecutor>, task: &Arc<OliveTask>, result: i64) -> DriveOutcome {
    // Relocate the result into the process-lifetime arena while this task's
    // own arena is still alive: `result` may point into it, and dropping it
    // below deallocates its chunks.
    let delivered = crate::copy_typed::relocate_across_boundary(result);
    // The compiled completion path caches the raw result in the frame's
    // sub-future slot for later re-polls (`olive_sm_poll` from gather/select,
    // or a late await). Rewrite it with the arena-independent copy so every
    // such harvester sees memory that outlives this task's arena.
    let frame = unsafe { (*(task.sm_future as *const OliveSmFuture)).frame };
    unsafe { *((frame + 8) as *mut i64) = delivered };

    for c in std::mem::take(&mut *task.completions.lock().unwrap()) {
        *c.result.lock().unwrap() = Some(delivered);
        c.cvar.notify_all();
    }
    let waiters = {
        let mut guard = task.sm_waiters.lock().unwrap();
        task.done.store(true, Ordering::SeqCst);
        std::mem::take(&mut *guard)
    };
    ex.task_map.lock().unwrap().remove(&task.sm_future);
    *task.slabs.lock().unwrap() = None;
    for w in waiters {
        *w.pending_result.lock().unwrap() = Some(delivered);
        executor_enqueue(ex, &w);
    }
    DriveOutcome::Completed
}

#[repr(C)]
struct OliveSmFuture {
    kind: i64,
    poll_fn: i64,
    frame: i64,
    cancelled: i64,
}

/// Debugger-only: the heap-frame pointer of the task logically awaiting the
/// state-machine task whose own frame is `frame_ptr`, or 0 if none (the
/// awaited task is a root, spawned rather than awaited). Lets a debug
/// session reconstruct the async call stack -- the chain of suspended `async
/// fn` frames parked on an `await` up from wherever the debuggee stopped --
/// out of the executor's existing wait graph (`sm_waiters`), which already
/// records exactly who is waiting on whom. Read-only, called from the
/// controller thread while the debuggee is parked.
#[unsafe(no_mangle)]
pub extern "C" fn olive_debug_sm_awaiter_frame(frame_ptr: i64) -> i64 {
    let Some(ex) = EXECUTOR.get() else {
        return 0;
    };
    // Both locks are taken only here, always in this order; the executor
    // takes each one alone, so this cannot deadlock against it. A parked
    // awaiter stays in `task_map` until its own completion, so the map lock
    // also pins it for the duration of the read.
    let map = ex.task_map.lock().unwrap();
    let task = map.values().find(|t| {
        let sf = unsafe { &*(t.sm_future as *const OliveSmFuture) };
        sf.frame == frame_ptr
    });
    let Some(task) = task else {
        return 0;
    };
    let waiters = task.sm_waiters.lock().unwrap();
    let Some(w) = waiters.first() else {
        return 0;
    };
    let wf = unsafe { &*(w.sm_future as *const OliveSmFuture) };
    wf.frame
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
        executor_enqueue(ex, &task);
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
    // Callback blob carries copied heap args. Spawning thread keeps originals.
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

    // Result handoff via Mutex/Condvar.
    crate::debug::spawn_traced("olive-spawn-task", move || {
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
        crate::string::olive_str_from_ptr(path)
    };

    let shared = Arc::new(FutureShared {
        state: Mutex::new(FutureState::Pending),
        cvar: Condvar::new(),
    });
    let shared2 = shared.clone();

    // Result handoff via Mutex/Condvar.
    std::thread::spawn(move || {
        let result = match std::fs::read(&path_str) {
            Ok(bytes) => {
                // The awaiting thread frees this through its own slab arena,
                // so the string must be built in the process-lifetime escape
                // arena, not this thread's private one.
                let text = String::from_utf8_lossy(&bytes);
                crate::slab::with_escape_arena(|| crate::string_slab::str_alloc(text.as_bytes()))
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
        crate::string::olive_str_from_ptr(path)
    };
    // Byte-exact: length-delimited read, so strings with interior nul bytes
    // survive (the old CStr path truncated at the first one).
    let data_bytes = if data == 0 {
        Vec::new()
    } else {
        crate::string::olive_str_to_bytes(data).to_vec()
    };

    let shared = Arc::new(FutureShared {
        state: Mutex::new(FutureState::Pending),
        cvar: Condvar::new(),
    });
    let shared2 = shared.clone();

    // Result handoff via Mutex/Condvar.
    std::thread::spawn(move || {
        let result = match std::fs::write(&path_str, &data_bytes) {
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
    futures_list: i64,
    results: i64,
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
            } else {
                any_pending = true;
            }
        }
    }

    if any_pending {
        POLL_PENDING
    } else {
        f.state = -1;
        f.results
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_gather(futures_list: i64) -> i64 {
    if futures_list == 0 {
        return crate::list::list_from_vec(Vec::new());
    }
    let list = unsafe { &*(futures_list as *const StableVec) };
    let n = list.len;

    let results_list = crate::list::list_from_vec(vec![POLL_PENDING; n]);

    let frame = Box::into_raw(Box::new(GatherFrame {
        state: 0,
        futures_list,
        results: results_list,
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
            return r;
        }
    }
    POLL_PENDING
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_select(futures_list: i64) -> i64 {
    if futures_list == 0 {
        return 0;
    }
    let frame = Box::into_raw(Box::new(SelectFrame {
        state: 0,
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
    let arg = crate::copy_typed::relocate_across_boundary(arg);
    crate::debug::spawn_traced("olive-pool-run", move || {
        let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(fn_ptr as usize) };
        // The result crosses back to the awaiting thread, which frees it from
        // a different per-thread slab than the one this thread allocated it
        // in; copy it into the process-lifetime arena first.
        let result = crate::copy_typed::relocate_across_boundary(f(arg));
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
    let arg = crate::copy_typed::relocate_across_boundary(arg);
    crate::debug::spawn_traced("olive-pool-run-sync", move || {
        let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(fn_ptr as usize) };
        // Same cross-slab free hazard as `olive_pool_run`: the caller frees
        // the result from its own arena.
        let result = crate::copy_typed::relocate_across_boundary(f(arg));
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
    fn pool_size_positive() {
        assert!(olive_pool_size() >= 1);
    }

    extern "C" fn add_one(x: i64) -> i64 {
        x + 1
    }

    #[test]
    fn pool_run_sync_executes() {
        assert_eq!(olive_pool_run_sync(add_one as *const () as i64, 41), 42);
    }

    #[test]
    fn spawn_n_tasks_stress() {
        let mut handles = Vec::new();
        for i in 0..10i64 {
            let shared = Arc::new(FutureShared {
                state: Mutex::new(FutureState::Pending),
                cvar: Condvar::new(),
            });
            let shared2 = shared.clone();
            handles.push(std::thread::spawn(move || {
                *shared2.state.lock().unwrap() = FutureState::Ready(i * i);
                shared2.cvar.notify_all();
            }));
            let mut state = shared.state.lock().unwrap();
            while let FutureState::Pending = *state {
                state = shared.cvar.wait(state).unwrap();
            }
            if let FutureState::Ready(v) = *state {
                assert_eq!(v, i * i);
            }
        }
        for h in handles {
            h.join().unwrap();
        }
    }
}
