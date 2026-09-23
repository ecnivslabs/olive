const KIND_FUTURE: i64 = 4;
const KIND_SM_FUTURE: i64 = 5;
const POLL_PENDING: i64 = i64::MIN;

#[cfg(test)]
mod lifecycle_tests;

use crate::StableVec;
use std::collections::VecDeque;
use std::sync::{
    Arc, Condvar, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
};
use std::time::Duration;

struct OliveTask {
    sm_future: i64,
    queued: AtomicBool,
    done: AtomicBool,
    retired: AtomicBool,
    handle_pins: AtomicUsize,
    pending_child: Mutex<Option<Arc<OliveTask>>>,
    completions: Mutex<Vec<Arc<Completion>>>,
    sm_waiters: Mutex<Vec<Arc<OliveTask>>>,
    slabs: Mutex<Option<Box<crate::slab::SlabSet>>>,
}

struct SmPollGuard(i64);

impl Drop for SmPollGuard {
    fn drop(&mut self) {
        unsafe { &(*(self.0 as *const OliveSmFuture)).poll_lock }.store(false, Ordering::Release);
    }
}

fn try_acquire_sm_poll(future: i64) -> Option<SmPollGuard> {
    let acquired = unsafe { &(*(future as *const OliveSmFuture)).poll_lock }
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_ok();
    acquired.then_some(SmPollGuard(future))
}

impl Drop for OliveTask {
    fn drop(&mut self) {
        if !self.retired.load(Ordering::Acquire) {
            return;
        }
        let owns_heap_future = sm_live().lock().unwrap().contains(&self.sm_future);
        if !owns_heap_future {
            return;
        }
        unsafe {
            let future = &mut *(self.sm_future as *mut OliveSmFuture);
            release_sm_frame(future, false);
            let cached = future.cached;
            let result_desc = future.result_desc;
            free_cached_value(cached, result_desc);
            olive_sm_free(self.sm_future, std::mem::size_of::<OliveSmFuture>() as i64);
        }
    }
}

struct Completion {
    result: Mutex<Option<i64>>,
    cvar: Condvar,
}

struct OliveExecutor {
    ready: Mutex<VecDeque<Arc<OliveTask>>>,
    wakeup: Condvar,
    task_map: Mutex<std::collections::HashMap<i64, Arc<OliveTask>>>,
    completed_tasks: Mutex<std::collections::HashMap<i64, Arc<OliveTask>>>,
}

#[derive(Debug, PartialEq, Eq)]
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
            completed_tasks: Mutex::new(std::collections::HashMap::new()),
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
        if task.done.load(Ordering::Acquire) {
            continue;
        }
        let completed_child = task.pending_child.lock().unwrap().take();
        if let Some(completed_child) = completed_child {
            if executor_complete_waker(&ex, &task, completed_child.clone()) {
                release_child_pin(&ex, &completed_child);
            }
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

fn executor_get_or_create_task_locked(
    map: &mut std::collections::HashMap<i64, Arc<OliveTask>>,
    sm_future_ptr: i64,
) -> Arc<OliveTask> {
    if let Some(t) = map.get(&sm_future_ptr) {
        return t.clone();
    }
    let t = Arc::new(OliveTask {
        sm_future: sm_future_ptr,
        queued: AtomicBool::new(false),
        done: AtomicBool::new(false),
        retired: AtomicBool::new(false),
        handle_pins: AtomicUsize::new(0),
        pending_child: Mutex::new(None),
        completions: Mutex::new(Vec::new()),
        sm_waiters: Mutex::new(Vec::new()),
        slabs: Mutex::new(None),
    });
    map.insert(sm_future_ptr, t.clone());
    t
}

fn maybe_remove_retired_task(ex: &OliveExecutor, task: &Arc<OliveTask>) {
    if !task.retired.load(Ordering::Acquire) || task.handle_pins.load(Ordering::Acquire) != 0 {
        return;
    }
    let removed = {
        let mut completed = ex.completed_tasks.lock().unwrap();
        if completed
            .get(&task.sm_future)
            .is_some_and(|current| Arc::ptr_eq(current, task))
        {
            completed.remove(&task.sm_future)
        } else {
            None
        }
    };
    drop(removed);
}

fn install_pending_child(parent: &Arc<OliveTask>, child: &Arc<OliveTask>) {
    let mut slot = parent.pending_child.lock().unwrap();
    if slot.is_none() {
        child.handle_pins.fetch_add(1, Ordering::AcqRel);
        *slot = Some(child.clone());
    }
}

fn release_child_pin(ex: &OliveExecutor, child: &Arc<OliveTask>) {
    if child.handle_pins.fetch_sub(1, Ordering::AcqRel) == 1 {
        maybe_remove_retired_task(ex, child);
    }
}

#[cfg(test)]
fn executor_get_or_create_task(ex: &OliveExecutor, sm_future_ptr: i64) -> Arc<OliveTask> {
    let mut map = ex.task_map.lock().unwrap();
    executor_get_or_create_task_locked(&mut map, sm_future_ptr)
}

fn executor_get_or_create_active_task(
    ex: &OliveExecutor,
    sm_future_ptr: i64,
) -> Option<Arc<OliveTask>> {
    let mut map = ex.task_map.lock().unwrap();
    if unsafe { &(*(sm_future_ptr as *const OliveSmFuture)).terminal }.load(Ordering::Acquire) {
        return None;
    }
    Some(executor_get_or_create_task_locked(&mut map, sm_future_ptr))
}

/// Drives one poll of `task` on the calling worker. The `driving` flag makes a
/// poll exclusive: two workers can otherwise poll the same machine at once (a
/// completion handoff enqueues the awaiter while it is still mid-poll), and
/// the generated poll is not reentrant — both would resume from the same
/// saved state and corrupt the frame. The loser of the flag re-enqueues; the
/// winner clears the flag before parking or completing so no wakeup is lost.
/// Cooperative cancellation contract: a task whose state machine future
/// was marked via `olive_cancel_future` never polls again. The first drive
/// after the mark (before any poll) and any resume after suspension both
/// complete immediately with a zero payload, notifying waiters through the
/// normal completion path so no parent hangs. Frame memory is reclaimed by
/// the completion path; task slabs are dropped there as usual.
fn task_cancelled(task: &Arc<OliveTask>) -> bool {
    unsafe { &*(task.sm_future as *const OliveSmFuture) }
        .cancelled
        .load(Ordering::Acquire)
        != 0
}

fn complete_cancelled(ex: &Arc<OliveExecutor>, task: &Arc<OliveTask>) -> DriveOutcome {
    unregister_from_child(ex, task);
    if let Some(child) = task.pending_child.lock().unwrap().take() {
        release_child_pin(ex, &child);
    }
    let frame = unsafe { &*(task.sm_future as *const OliveSmFuture) }.frame;
    if frame != 0 {
        unsafe {
            *(frame as *mut i64) = -1;
            *((frame as *mut i64).add(1)) = 0;
        }
    }
    executor_complete(ex, task, 0)
}

fn executor_drive(ex: &Arc<OliveExecutor>, task: &Arc<OliveTask>) -> DriveOutcome {
    let Some(_poll_guard) = try_acquire_sm_poll(task.sm_future) else {
        return DriveOutcome::Rerun;
    };

    if task_cancelled(task) {
        return complete_cancelled(ex, task);
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
    let poll_fn: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(sf.poll_fn as usize) };
    let result = poll_fn(sf.frame);

    crate::slab::ACTIVE_SLABS.set(old_active);
    let outcome = if unsafe { *(sf.frame as *const i64) } == -1 {
        return executor_complete(ex, task, result);
    } else {
        park_after_pending(ex, task, sf)
    };
    if task.pending_child.lock().unwrap().is_some() {
        DriveOutcome::Rerun
    } else {
        outcome
    }
}

/// Resumes a parent machine whose awaited sub-machine finished while the
/// parent sat queued. Holding `completed_child` pins its relocated cached
/// result until the parent copies that value. The resumed poll may run to
/// completion or park on a further await; both are handled.
fn executor_complete_waker(
    ex: &Arc<OliveExecutor>,
    task: &Arc<OliveTask>,
    completed_child: Arc<OliveTask>,
) -> bool {
    let Some(_poll_guard) = try_acquire_sm_poll(task.sm_future) else {
        // Mid-poll elsewhere; restore and let that poll's worker pick it up
        // on its next dequeue.
        install_pending_child(task, &completed_child);
        return false;
    };

    if task_cancelled(task) {
        complete_cancelled(ex, task);
        return true;
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
    let poll_fn: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(sf.poll_fn as usize) };
    let final_result = poll_fn(sf.frame);

    crate::slab::ACTIVE_SLABS.set(old_active);
    if unsafe { *(sf.frame as *const i64) } == -1 {
        executor_complete(ex, task, final_result);
        return true;
    }
    let outcome = park_after_pending(ex, task, sf);
    if task.pending_child.lock().unwrap().is_some() || matches!(outcome, DriveOutcome::Rerun) {
        executor_enqueue(ex, task);
    }
    true
}

fn wait_for_plain_child(task: &Arc<OliveTask>, shared: &FutureShared) -> bool {
    let mut state = shared.state.lock().unwrap();
    loop {
        match &*state {
            FutureState::Ready(_) => return true,
            FutureState::Pending => {
                if task_cancelled(task) {
                    return false;
                }
                let (next, _) = shared
                    .cvar
                    .wait_timeout(state, Duration::from_millis(10))
                    .unwrap();
                state = next;
            }
        }
    }
}

fn park_combinator_child(ex: &Arc<OliveExecutor>, task: &Arc<OliveTask>, child_ptr: i64) -> bool {
    if child_ptr == 0 {
        return false;
    }
    let kind = unsafe { *(child_ptr as *const i64) };
    match kind {
        KIND_FUTURE => {
            let child = unsafe { &*(child_ptr as *const OliveFuture) };
            let shared = unsafe { &*(child.shared as *const FutureShared) };
            wait_for_plain_child(task, shared)
        }
        KIND_SM_FUTURE => {
            let Some(child) = executor_get_or_create_active_task(ex, child_ptr) else {
                return false;
            };
            if child.done.load(Ordering::Acquire) {
                return false;
            }
            if child.pending_child.lock().unwrap().take().is_some() {
                install_pending_child(task, &child);
                return false;
            }
            let mut waiters = child.sm_waiters.lock().unwrap();
            if child.done.load(Ordering::Acquire)
                || waiters.iter().any(|waiter| Arc::ptr_eq(waiter, task))
            {
                return false;
            }
            waiters.push(task.clone());
            drop(waiters);
            executor_enqueue(ex, &child);
            true
        }
        _ => false,
    }
}

fn park_combinator(
    ex: &Arc<OliveExecutor>,
    task: &Arc<OliveTask>,
    futures_list: i64,
) -> DriveOutcome {
    let count = future_list_len(futures_list);
    if count == 0 {
        return DriveOutcome::Rerun;
    }
    let list = unsafe { &*(futures_list as *const StableVec) };
    let mut parked = false;
    for index in 0..count {
        let child_ptr = unsafe { *list.ptr.add(index) };
        parked |= park_combinator_child(ex, task, child_ptr);
        if task_cancelled(task) {
            break;
        }
    }
    if parked {
        DriveOutcome::Parked
    } else {
        DriveOutcome::Rerun
    }
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
    if sf.poll_fn == olive_gather_poll as *const () as usize as i64 {
        let list = unsafe { &*(sf.frame as *const GatherFrame) }.futures_list;
        return park_combinator(ex, task, list);
    }
    if sf.poll_fn == olive_select_poll as *const () as usize as i64 {
        let list = unsafe { &*(sf.frame as *const SelectFrame) }.futures_list;
        return park_combinator(ex, task, list);
    }

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
            let Some(sub_task) = executor_get_or_create_active_task(ex, sub_future) else {
                return DriveOutcome::Rerun;
            };
            // Already finished but its wakeup has not been consumed yet.
            if sub_task.pending_child.lock().unwrap().take().is_some() {
                install_pending_child(task, &sub_task);
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
                return DriveOutcome::Rerun;
            }
            waiters.push(task.clone());
            drop(waiters);
            executor_enqueue(ex, &sub_task);
            DriveOutcome::Parked
        }
        _ => DriveOutcome::Rerun,
    }
}

fn unregister_from_child(ex: &Arc<OliveExecutor>, task: &Arc<OliveTask>) {
    let sf = unsafe { &*(task.sm_future as *const OliveSmFuture) };
    if sf.frame == 0 {
        return;
    }
    if sf.poll_fn == olive_gather_poll as *const () as usize as i64 {
        let list_ptr = unsafe { (*(sf.frame as *const GatherFrame)).futures_list };
        if list_ptr != 0 {
            let list = unsafe { &*(list_ptr as *const StableVec) };
            unregister_from_child_list(ex, task, list);
        }
        return;
    }
    if sf.poll_fn == olive_select_poll as *const () as usize as i64 {
        let list_ptr = unsafe { (*(sf.frame as *const SelectFrame)).futures_list };
        if list_ptr != 0 {
            let list = unsafe { &*(list_ptr as *const StableVec) };
            unregister_from_child_list(ex, task, list);
        }
        return;
    }
    let child_ptr = unsafe { *((sf.frame + 8) as *const i64) };
    unregister_one_child(ex, task, child_ptr);
}

fn unregister_from_child_list(ex: &Arc<OliveExecutor>, task: &Arc<OliveTask>, list: &StableVec) {
    for index in 0..list.len {
        let child_ptr = unsafe { *list.ptr.add(index) };
        unregister_one_child(ex, task, child_ptr);
    }
}

fn unregister_one_child(ex: &Arc<OliveExecutor>, task: &Arc<OliveTask>, child_ptr: i64) {
    if child_ptr == 0 || unsafe { *(child_ptr as *const i64) } != KIND_SM_FUTURE {
        return;
    }
    let child = ex.task_map.lock().unwrap().get(&child_ptr).cloned();
    if let Some(child) = child {
        child
            .sm_waiters
            .lock()
            .unwrap()
            .retain(|waiter| !Arc::ptr_eq(waiter, task));
    }
}

fn executor_complete(ex: &Arc<OliveExecutor>, task: &Arc<OliveTask>, result: i64) -> DriveOutcome {
    // Relocate the result into the process-lifetime arena while this task's
    // own arena is still alive: `result` may point into it, and dropping it
    // below deallocates its chunks.
    let sf = unsafe { &*(task.sm_future as *const OliveSmFuture) };
    let mut slabs = task.slabs.lock().unwrap();
    let old_active = crate::slab::ACTIVE_SLABS.get();
    if let Some(slabs) = slabs.as_mut() {
        crate::slab::ACTIVE_SLABS.set(slabs.as_mut());
    }
    let delivered = if sf.result_desc == 0 {
        let delivered = crate::copy_typed::relocate_across_boundary(result);
        crate::olive_free_any(result);
        delivered
    } else {
        let delivered = crate::copy_typed::olive_relocate_typed(result, sf.result_desc);
        crate::free_typed::olive_free_typed(result, sf.result_desc);
        delivered
    };
    crate::slab::ACTIVE_SLABS.set(old_active);
    drop(slabs);
    executor_publish(ex, task, delivered)
}

fn executor_publish(
    ex: &Arc<OliveExecutor>,
    task: &Arc<OliveTask>,
    delivered: i64,
) -> DriveOutcome {
    let sf = unsafe { &*(task.sm_future as *const OliveSmFuture) };
    // Ownership handoff on completion: the frame transfers to the executor,
    // which caches the arena-independent result in the handle for later
    // re-polls (`olive_sm_poll` from gather/select, or a late await) and
    // releases heap frames. Stack/borrowed frames (never routed through
    // `olive_sm_alloc`) stay live for the existing frame slot cache path.
    // The handle stays owned by the creator, released via `olive_free_future`.
    let frame = sf.frame;
    let future_ptr = task.sm_future;
    let mut map = ex.task_map.lock().unwrap();
    let mut completed = ex.completed_tasks.lock().unwrap();
    unsafe {
        let f = &mut *(future_ptr as *mut OliveSmFuture);
        f.cached = delivered;
        if frame != 0 {
            *((frame + 8) as *mut i64) = delivered;
            if sm_live().lock().unwrap().contains(&frame) {
                release_sm_frame(f, true);
            }
        }
        f.terminal.store(true, Ordering::Release);
    };

    let waiters = {
        // Blocking waiters own their published values. Their handle may be
        // retired as soon as terminal state becomes visible.
        for c in std::mem::take(&mut *task.completions.lock().unwrap()) {
            let completion_result = copy_future_value_global(delivered, sf.result_desc);
            *c.result.lock().unwrap() = Some(completion_result);
            c.cvar.notify_all();
        }
        let mut guard = task.sm_waiters.lock().unwrap();
        task.done.store(true, Ordering::SeqCst);
        let waiters = std::mem::take(&mut *guard);
        drop(guard);
        map.remove(&task.sm_future);
        if task.retired.load(Ordering::Acquire) || !waiters.is_empty() {
            completed.insert(task.sm_future, task.clone());
        }
        waiters
    };
    drop(completed);
    drop(map);
    *task.slabs.lock().unwrap() = None;
    for w in waiters {
        install_pending_child(&w, task);
        executor_enqueue(ex, &w);
    }
    maybe_remove_retired_task(ex, task);
    DriveOutcome::Completed
}

#[repr(C)]
struct OliveSmFuture {
    kind: i64,
    poll_fn: i64,
    frame: i64,
    cancelled: AtomicI64,
    result_desc: i64,
    frame_size: i64,
    cached: i64,
    terminal: AtomicBool,
    poll_lock: AtomicBool,
}

/// Allocation counters for state machine frames and handles, backing the
/// reclamation regression tests. Generated code routes both through
/// `olive_sm_alloc`/`olive_sm_free` so JIT and AOT workloads share the
/// same counted path as the Rust constructors below.
static SM_ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static SM_FREE_COUNT: AtomicUsize = AtomicUsize::new(0);
static SM_LIVE: OnceLock<Mutex<std::collections::HashSet<i64>>> = OnceLock::new();

fn sm_live() -> &'static Mutex<std::collections::HashSet<i64>> {
    SM_LIVE.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sm_alloc(size: i64) -> i64 {
    let ptr = crate::olive_alloc(size) as i64;
    SM_ALLOC_COUNT.fetch_add(1, Ordering::SeqCst);
    sm_live().lock().unwrap().insert(ptr);
    ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_sm_free(ptr: i64, size: i64) {
    if ptr == 0 {
        return;
    }
    let owned = sm_live().lock().unwrap().remove(&ptr);
    if !owned {
        return;
    }
    SM_FREE_COUNT.fetch_add(1, Ordering::SeqCst);
    unsafe {
        std::alloc::dealloc(
            ptr as *mut u8,
            std::alloc::Layout::from_size_align(size as usize, 8).unwrap(),
        );
    }
}

#[cfg(test)]
pub(crate) fn sm_alloc_free_counts() -> (usize, usize) {
    (
        SM_ALLOC_COUNT.load(Ordering::SeqCst),
        SM_FREE_COUNT.load(Ordering::SeqCst),
    )
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

fn copy_future_value(value: i64, result_desc: i64) -> i64 {
    if result_desc == 0 {
        crate::copy_typed::copy_any(value, &mut rustc_hash::FxHashMap::default())
    } else {
        crate::copy_typed::olive_copy_typed(value, result_desc)
    }
}

fn copy_future_value_global(value: i64, result_desc: i64) -> i64 {
    crate::slab::with_escape_arena(|| copy_future_value(value, result_desc))
}

fn free_cached_value(value: i64, result_desc: i64) {
    if value == 0 {
        return;
    }
    if result_desc == 0 {
        crate::olive_free_any(value);
    } else {
        crate::free_typed::olive_free_typed(value, result_desc);
    }
}

fn cache_sm_result(future: i64, result: i64) -> i64 {
    let f = unsafe { &mut *(future as *mut OliveSmFuture) };
    let result_desc = f.result_desc;
    let delivered = if result_desc == 0 {
        let delivered = crate::copy_typed::relocate_across_boundary(result);
        crate::olive_free_any(result);
        delivered
    } else {
        let delivered = crate::copy_typed::olive_relocate_typed(result, result_desc);
        crate::free_typed::olive_free_typed(result, result_desc);
        delivered
    };
    f.cached = delivered;
    let frame = f.frame;
    if frame != 0 && sm_live().lock().unwrap().contains(&frame) {
        unsafe { release_sm_frame(f, true) };
    }
    f.terminal.store(true, Ordering::Release);
    delivered
}

/// Returns readiness separately because every payload bit pattern is valid.
/// A pending poll leaves the output word untouched.
#[unsafe(no_mangle)]
pub extern "C" fn olive_sm_poll(future: i64, output: i64) -> i64 {
    if future == 0 {
        unsafe { *(output as *mut i64) = 0 };
        return 1;
    }

    if unsafe { *(future as *const i64) } == KIND_SM_FUTURE {
        // Declared before the poll guard so a retired task outlives the copy
        // taken from its cached result below.
        let mut _completed_task_pin = None;
        let Some(_poll_guard) = try_acquire_sm_poll(future) else {
            return 0;
        };
        let f = unsafe { &*(future as *const OliveSmFuture) };
        let (result, result_desc) = if f.terminal.load(Ordering::Acquire) {
            (Some(f.cached), f.result_desc)
        } else if f.cancelled.load(Ordering::Acquire) != 0
            && unsafe { *(f.frame as *const i64) } != -1
        {
            (None, f.result_desc)
        } else {
            let poll_fn: extern "C" fn(i64) -> i64 =
                unsafe { std::mem::transmute(f.poll_fn as usize) };
            let result = poll_fn(f.frame);
            if unsafe { *(f.frame as *const i64) } == -1 {
                let result_desc = f.result_desc;
                let cached = cache_sm_result(future, result);
                if let Some(ex) = EXECUTOR.get() {
                    let task = ex.task_map.lock().unwrap().get(&future).cloned();
                    if let Some(task) = task {
                        _completed_task_pin = Some(task.clone());
                        executor_publish(ex, &task, cached);
                    }
                }
                (Some(cached), result_desc)
            } else {
                (None, f.result_desc)
            }
        };
        if let Some(value) = result {
            let value = copy_future_value(value, result_desc);
            unsafe { *(output as *mut i64) = value };
            return 1;
        }
        return 0;
    }

    let f = unsafe { &*(future as *const OliveFuture) };
    let shared = unsafe { &*(f.shared as *const FutureShared) };
    let result = {
        let state = shared.state.lock().unwrap();
        match &*state {
            FutureState::Ready(v) => Some(*v),
            FutureState::Pending => None,
        }
    };
    if let Some(value) = result {
        let value = copy_future_value(value, shared.result_desc);
        unsafe { *(output as *mut i64) = value };
        1
    } else {
        0
    }
}

enum FutureState {
    Pending,
    Ready(i64),
}

struct FutureShared {
    state: Mutex<FutureState>,
    result_desc: i64,
    cvar: Condvar,
}

impl Drop for FutureShared {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap();
        if let FutureState::Ready(value) = *state {
            free_cached_value(value, self.result_desc);
        }
    }
}

#[repr(C)]
struct OliveFuture {
    kind: i64,
    shared: i64, // raw ptr into Arc<FutureShared>
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_make_future(val: i64) -> i64 {
    let owned = crate::copy_typed::relocate_across_boundary(val);
    let shared = Arc::new(FutureShared {
        state: Mutex::new(FutureState::Ready(owned)),
        result_desc: 0,
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
        let task = {
            let mut map = ex.task_map.lock().unwrap();
            let state = unsafe { &*(future as *const OliveSmFuture) };
            if state.terminal.load(Ordering::Acquire) {
                return copy_future_value_global(state.cached, state.result_desc);
            }
            let task = executor_get_or_create_task_locked(&mut map, future);
            if task.done.load(Ordering::Acquire) {
                return copy_future_value_global(state.cached, state.result_desc);
            }
            task.completions.lock().unwrap().push(completion.clone());
            task
        };
        executor_enqueue(ex, &task);
        {
            let mut r = completion.result.lock().unwrap();
            loop {
                match *r {
                    Some(v) => break v,
                    None => r = completion.cvar.wait(r).unwrap(),
                }
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
        let result_desc = shared.result_desc;
        std::mem::forget(shared);
        copy_future_value_global(result, result_desc)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_spawn_task(callback: i64) -> i64 {
    // Callback blob carries copied heap args. Spawning thread keeps originals.
    let cb = callback as *const i64;
    let fn_ptr = unsafe { *cb } as usize;
    let nargs = unsafe { *cb.add(1) } as usize;
    let result_desc = unsafe { *cb.add(2) };
    let args: Vec<i64> = (0..nargs).map(|i| unsafe { *cb.add(3 + i) }).collect();
    unsafe {
        let layout = std::alloc::Layout::from_size_align(8 * (3 + nargs), 8).unwrap();
        std::alloc::dealloc(callback as *mut u8, layout);
    }

    let shared = Arc::new(FutureShared {
        state: Mutex::new(FutureState::Pending),
        result_desc,
        cvar: Condvar::new(),
    });
    let shared2 = shared.clone();

    // Result handoff via Mutex/Condvar. A concurrent cancel wins the race
    // by moving Pending to Ready(0) first; the producer then releases its
    // delivered copy instead of overwriting the cancellation.
    crate::debug::spawn_traced("olive-spawn-task", move || {
        let invoke: extern "C" fn(*const i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
        let result = invoke(args.as_ptr());
        let delivered = crate::copy_typed::olive_relocate_typed(result, result_desc);
        crate::free_typed::olive_free_typed(result, result_desc);
        let mut state = shared2.state.lock().unwrap();
        if matches!(*state, FutureState::Pending) {
            *state = FutureState::Ready(delivered);
        } else {
            crate::free_typed::olive_free_typed(delivered, result_desc);
        }
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
    let kind = unsafe { *(future as *const i64) };
    if kind == KIND_SM_FUTURE {
        let task = EXECUTOR.get().and_then(|ex| {
            if let Some(task) = ex.task_map.lock().unwrap().get(&future).cloned() {
                return Some(task);
            }
            ex.completed_tasks.lock().unwrap().get(&future).cloned()
        });
        if let (Some(ex), Some(task)) = (EXECUTOR.get(), task) {
            task.retired.store(true, Ordering::Release);
            unsafe {
                (*(future as *const OliveSmFuture))
                    .cancelled
                    .store(1, Ordering::Release);
            }
            if task.done.load(Ordering::Acquire) {
                maybe_remove_retired_task(ex, &task);
            } else {
                executor_enqueue(ex, &task);
            }
            return 0;
        }

        let (cached, result_desc) = unsafe {
            let f = &mut *(future as *mut OliveSmFuture);
            let cached = f.cached;
            let result_desc = f.result_desc;
            release_sm_frame(f, false);
            (cached, result_desc)
        };
        free_cached_value(cached, result_desc);
        olive_sm_free(future, std::mem::size_of::<OliveSmFuture>() as i64);
    } else {
        let f = unsafe { Box::from_raw(future as *mut OliveFuture) };
        let shared = unsafe { Arc::from_raw(f.shared as *const FutureShared) };
        drop(shared);
    }
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
        result_desc: 0,
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
        if matches!(*state, FutureState::Pending) {
            *state = FutureState::Ready(result);
        } else {
            crate::olive_free_str(result);
        }
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
        result_desc: 0,
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
        if matches!(*state, FutureState::Pending) {
            *state = FutureState::Ready(result);
        }
        shared2.cvar.notify_all();
    });

    let f = Box::new(OliveFuture {
        kind: KIND_FUTURE,
        shared: Arc::into_raw(shared) as i64,
    });
    Box::into_raw(f) as i64
}

unsafe fn retain_plain_future(future: i64) -> i64 {
    if unsafe { *(future as *const i64) } == KIND_FUTURE {
        let f = unsafe { &*(future as *const OliveFuture) };
        unsafe { Arc::increment_strong_count(f.shared as *const FutureShared) };
        return Box::into_raw(Box::new(OliveFuture {
            kind: KIND_FUTURE,
            shared: f.shared,
        })) as i64;
    }
    future
}

unsafe fn release_plain_future_ref(future: i64) {
    if future != 0 && unsafe { *(future as *const i64) } == KIND_FUTURE {
        let f = unsafe { Box::from_raw(future as *mut OliveFuture) };
        unsafe { Arc::decrement_strong_count(f.shared as *const FutureShared) };
        drop(f);
    }
}

fn future_list_len(list_ptr: i64) -> usize {
    if list_ptr == 0
        || !crate::slab::ptr_is_slab_body(list_ptr)
        || !crate::list::owns_list(list_ptr)
    {
        return 0;
    }
    let kind = unsafe { *(list_ptr as *const i64) };
    if kind != crate::KIND_LIST && kind != crate::KIND_ANY_LIST {
        return 0;
    }
    let list = unsafe { &*(list_ptr as *const StableVec) };
    for index in 0..list.len {
        let child = unsafe { *list.ptr.add(index) };
        if child <= 0x1000 || child & 7 != 0 {
            return 0;
        }
        let kind = unsafe { *(child as *const i64) };
        if !matches!(kind, KIND_FUTURE | KIND_SM_FUTURE) {
            return 0;
        }
    }
    list.len
}

#[repr(C)]
struct GatherFrame {
    state: i64,
    cached_result: i64,
    futures_list: i64,
    results: i64,
    completed: Vec<u8>,
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_gather_poll(frame: i64) -> i64 {
    let f = unsafe { &mut *(frame as *mut GatherFrame) };
    if f.state == -1 {
        return f.cached_result;
    }

    let n = future_list_len(f.futures_list);
    if n == 0 {
        f.state = -1;
        f.cached_result = f.results;
        return f.results;
    }
    let list = unsafe { &*(f.futures_list as *const StableVec) };
    let results_vec = unsafe { &*(f.results as *const StableVec) };
    let results = unsafe { std::slice::from_raw_parts_mut(results_vec.ptr, n) };

    let mut any_pending = false;
    for (i, res) in results.iter_mut().enumerate().take(n) {
        if f.completed[i] != 0 {
            continue;
        }
        let fut = unsafe { *list.ptr.add(i) };
        if olive_sm_poll(fut, res as *mut i64 as i64) == 0 {
            any_pending = true;
        } else {
            f.completed[i] = 1;
        }
    }

    if any_pending {
        POLL_PENDING
    } else {
        f.state = -1;
        f.cached_result = f.results;
        f.results
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_gather(futures_list: i64) -> i64 {
    let n = future_list_len(futures_list);
    let retained_list = if n == 0 {
        0
    } else {
        let source = unsafe { &*(futures_list as *const StableVec) };
        crate::slab::with_escape_arena(|| {
            let copy = crate::list::olive_list_new(n as i64);
            for index in 0..n {
                let child = unsafe { *source.ptr.add(index) };
                crate::list::olive_list_set(copy, index as i64, unsafe {
                    retain_plain_future(child)
                });
            }
            copy
        })
    };
    let results_list = crate::slab::with_escape_arena(|| crate::list::list_from_vec(vec![0; n]));

    let frame = olive_sm_alloc(std::mem::size_of::<GatherFrame>() as i64);
    unsafe {
        std::ptr::write(
            frame as *mut GatherFrame,
            GatherFrame {
                state: 0,
                cached_result: 0,
                futures_list: retained_list,
                results: results_list,
                completed: vec![0; n],
            },
        );
    }

    let fut = olive_sm_alloc(std::mem::size_of::<OliveSmFuture>() as i64);
    unsafe {
        std::ptr::write(
            fut as *mut OliveSmFuture,
            OliveSmFuture {
                kind: KIND_SM_FUTURE,
                poll_fn: olive_gather_poll as *const () as usize as i64,
                frame,
                cancelled: AtomicI64::new(0),
                result_desc: 0,
                frame_size: std::mem::size_of::<GatherFrame>() as i64,
                cached: 0,
                terminal: AtomicBool::new(false),
                poll_lock: AtomicBool::new(false),
            },
        );
    }
    fut
}

#[repr(C)]
struct SelectFrame {
    state: i64,
    cached_result: i64,
    futures_list: i64,
}

unsafe fn release_future_list_shell(list: i64) {
    let Some(is_global) = crate::slab::slab_membership(list) else {
        return;
    };
    if !crate::list::owns_list(list) || !crate::slab::slot_is_live(list) {
        return;
    }
    unsafe {
        let snapshot = &*(list as *const StableVec);
        for index in 0..snapshot.len {
            release_plain_future_ref(*snapshot.ptr.add(index));
        }
        crate::list::settle_list_buffer(list);
        crate::list::free_list_slot_raw_with(list, Some(is_global));
    }
}

unsafe fn release_sm_frame(f: &mut OliveSmFuture, result_transferred: bool) {
    let frame = f.frame;
    if frame == 0 {
        return;
    }
    if f.poll_fn == olive_gather_poll as *const () as usize as i64 {
        let gather = unsafe { &mut *(frame as *mut GatherFrame) };
        if !result_transferred && gather.results != 0 {
            crate::olive_free_list(gather.results);
        }
        unsafe { release_future_list_shell(gather.futures_list) };
        gather.futures_list = 0;
        gather.results = 0;
        unsafe {
            std::ptr::drop_in_place(gather as *mut GatherFrame);
        }
    } else if f.poll_fn == olive_select_poll as *const () as usize as i64 {
        let select = unsafe { &mut *(frame as *mut SelectFrame) };
        unsafe { release_future_list_shell(select.futures_list) };
        select.futures_list = 0;
    }
    f.frame = 0;
    olive_sm_free(frame, f.frame_size);
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_select_poll(frame: i64) -> i64 {
    let f = unsafe { &mut *(frame as *mut SelectFrame) };
    if f.state == -1 {
        return f.cached_result;
    }
    let n = future_list_len(f.futures_list);
    if n == 0 {
        f.state = -1;
        f.cached_result = 0;
        return 0;
    }
    let list = unsafe { &*(f.futures_list as *const StableVec) };

    for i in 0..n {
        let fut = unsafe { *list.ptr.add(i) };
        if olive_sm_poll(fut, &mut f.cached_result as *mut i64 as i64) != 0 {
            f.state = -1;
            return f.cached_result;
        }
    }
    POLL_PENDING
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_select(futures_list: i64) -> i64 {
    let n = future_list_len(futures_list);
    if n == 0 {
        return 0;
    }
    let source = unsafe { &*(futures_list as *const StableVec) };
    let retained_list = crate::slab::with_escape_arena(|| {
        let copy = crate::list::olive_list_new(n as i64);
        for index in 0..n {
            let child = unsafe { *source.ptr.add(index) };
            crate::list::olive_list_set(copy, index as i64, unsafe { retain_plain_future(child) });
        }
        copy
    });
    let frame = olive_sm_alloc(std::mem::size_of::<SelectFrame>() as i64);
    unsafe {
        std::ptr::write(
            frame as *mut SelectFrame,
            SelectFrame {
                state: 0,
                cached_result: 0,
                futures_list: retained_list,
            },
        );
    }
    let fut = olive_sm_alloc(std::mem::size_of::<OliveSmFuture>() as i64);
    unsafe {
        std::ptr::write(
            fut as *mut OliveSmFuture,
            OliveSmFuture {
                kind: KIND_SM_FUTURE,
                poll_fn: olive_select_poll as *const () as usize as i64,
                frame,
                cancelled: AtomicI64::new(0),
                result_desc: 0,
                frame_size: std::mem::size_of::<SelectFrame>() as i64,
                cached: 0,
                terminal: AtomicBool::new(false),
                poll_lock: AtomicBool::new(false),
            },
        );
    }
    fut
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_cancel_future(future: i64) -> i64 {
    if future == 0 {
        return 0;
    }
    let kind = unsafe { *(future as *const i64) };
    if kind == KIND_SM_FUTURE {
        let f = unsafe { &mut *(future as *mut OliveSmFuture) };
        f.cancelled.store(1, Ordering::Release);
        if let Some(ex) = EXECUTOR.get() {
            let task = ex.task_map.lock().unwrap().get(&future).cloned();
            if let Some(task) = task {
                executor_enqueue(ex, &task);
            }
        }
    } else if kind == KIND_FUTURE {
        let f = unsafe { &*(future as *const OliveFuture) };
        let shared = unsafe { &*(f.shared as *const FutureShared) };
        let mut state = shared.state.lock().unwrap();
        if matches!(*state, FutureState::Pending) {
            *state = FutureState::Ready(0);
            shared.cvar.notify_all();
        }
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
        result_desc: 0,
        cvar: Condvar::new(),
    });
    let shared2 = shared.clone();
    // The caller transfers ownership of `arg` here (`RUNTIME_ESCAPES`), and
    // the relocated copy is what crosses: release the original now, or every
    // heap argument strands one object. For surface calls `arg` is an `int`
    // and both the copy and this free are no-ops.
    let arg = {
        let relocated = crate::copy_typed::relocate_across_boundary(arg);
        crate::olive_free_any(arg);
        relocated
    };
    crate::debug::spawn_traced("olive-pool-run", move || {
        let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(fn_ptr as usize) };
        // The result crosses back to the awaiting thread, which frees it from
        // a different per-thread slab than the one this thread allocated it
        // in; copy it into the process-lifetime arena first.
        let result = {
            let returned = f(arg);
            // The called function lends `arg`: the relocated copy served its
            // purpose once `f` returns.
            crate::olive_free_any(arg);
            let relocated = crate::copy_typed::relocate_across_boundary(returned);
            crate::olive_free_any(returned);
            relocated
        };
        let mut state = shared2.state.lock().unwrap();
        if matches!(*state, FutureState::Pending) {
            *state = FutureState::Ready(result);
        } else {
            crate::olive_free_any(result);
        }
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
        result_desc: 0,
        cvar: Condvar::new(),
    });
    let shared2 = shared.clone();
    // Same ownership as `olive_pool_run`: the caller's argument is consumed
    // here and the called function's return is consumed below; both originals
    // are released once their arena copies take over.
    let arg = {
        let relocated = crate::copy_typed::relocate_across_boundary(arg);
        crate::olive_free_any(arg);
        relocated
    };
    crate::debug::spawn_traced("olive-pool-run-sync", move || {
        let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(fn_ptr as usize) };
        // Same cross-slab free hazard as `olive_pool_run`: the caller frees
        // the result from its own arena.
        let result = {
            let returned = f(arg);
            crate::olive_free_any(arg);
            let relocated = crate::copy_typed::relocate_across_boundary(returned);
            crate::olive_free_any(returned);
            relocated
        };
        let mut state = shared2.state.lock().unwrap();
        if matches!(*state, FutureState::Pending) {
            *state = FutureState::Ready(result);
        } else {
            crate::olive_free_any(result);
        }
        shared2.cvar.notify_all();
    });
    let mut state = shared.state.lock().unwrap();
    loop {
        if let FutureState::Ready(val) = *state {
            *state = FutureState::Ready(0);
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

    extern "C" fn echo_heap_len(x: i64) -> i64 {
        let _ = crate::olive_str_from_ptr(x);
        let s = crate::olive_str_internal("pool-result");
        let g = crate::string_slab::olive_str_gen_of(s);
        *ECHO_SLOT.lock().unwrap() = (s, g);
        s
    }

    static ECHO_SLOT: std::sync::Mutex<(i64, i64)> = std::sync::Mutex::new((0, 0));

    #[test]
    fn pool_run_sync_releases_heap_arg_and_result() {
        // `RUNTIME_ESCAPES` hands `arg` over and the worker lends it to `f`;
        // both originals (argument and `f`'s return) must be released once
        // their arena copies take over, or each call strands two objects.
        let arg = crate::olive_str_internal("pool-arg");
        let arg_gen = crate::string_slab::olive_str_gen_of(arg);
        let out = olive_pool_run_sync(echo_heap_len as *const () as i64, arg);
        assert_eq!(crate::olive_str_from_ptr(out), "pool-result");
        assert_eq!(crate::string_slab::olive_str_gen_stale(arg, arg_gen), 1);
        let (echo_ptr, echo_gen) = *ECHO_SLOT.lock().unwrap();
        assert_ne!(echo_ptr, 0);
        assert_eq!(
            crate::string_slab::olive_str_gen_stale(echo_ptr, echo_gen),
            1
        );
        let out_gen = crate::string_slab::olive_str_gen_of(out);
        crate::olive_free_any(out);
        assert_eq!(crate::string_slab::olive_str_gen_stale(out, out_gen), 1);
    }

    #[test]
    fn spawn_n_tasks_stress() {
        let mut handles = Vec::new();
        for i in 0..10i64 {
            let shared = Arc::new(FutureShared {
                state: Mutex::new(FutureState::Pending),
                result_desc: 0,
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
