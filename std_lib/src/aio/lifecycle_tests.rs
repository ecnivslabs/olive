use super::*;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize};

fn wait_for_sm_free(future: i64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while super::sm_live().lock().unwrap().contains(&future) {
        assert!(std::time::Instant::now() < deadline, "future was not freed");
        std::thread::yield_now();
    }
}

#[test]
fn contended_poll_does_not_release_another_drivers_lock() {
    let future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: 0,
        frame: 0,
        cancelled: AtomicI64::new(0),
        result_desc: 0,
        frame_size: 0,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let ptr = &future as *const OliveSmFuture as i64;
    let guard = try_acquire_sm_poll(ptr).unwrap();
    assert!(try_acquire_sm_poll(ptr).is_none());
    assert!(future.poll_lock.load(Ordering::Acquire));
    drop(guard);
    assert!(!future.poll_lock.load(Ordering::Acquire));
}

enum ChildState {
    Pending,
    Done,
    Registered,
}

fn check_child_lifetime(state: ChildState) {
    let ex = Arc::new(OliveExecutor {
        ready: Mutex::new(VecDeque::new()),
        wakeup: Condvar::new(),
        task_map: Mutex::new(std::collections::HashMap::new()),
        completed_tasks: Mutex::new(std::collections::HashMap::new()),
    });
    let mut child_frame = [0i64; 2];
    let child_future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: 0,
        frame: child_frame.as_mut_ptr() as i64,
        cancelled: AtomicI64::new(0),
        result_desc: [crate::format::D_INT].as_ptr() as i64,
        frame_size: 16,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let mut parent_frame = [0, &child_future as *const OliveSmFuture as i64];
    let parent_future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: 0,
        frame: parent_frame.as_mut_ptr() as i64,
        cancelled: AtomicI64::new(0),
        result_desc: [crate::format::D_INT].as_ptr() as i64,
        frame_size: 16,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let parent = executor_get_or_create_task(&ex, &parent_future as *const OliveSmFuture as i64);
    let child = executor_get_or_create_task(&ex, &child_future as *const OliveSmFuture as i64);
    let weak_child = Arc::downgrade(&child);
    let weak_parent = Arc::downgrade(&parent);
    match state {
        ChildState::Pending => {}
        ChildState::Done => child.done.store(true, Ordering::SeqCst),
        ChildState::Registered => child.sm_waiters.lock().unwrap().push(parent.clone()),
    }

    let outcome = park_after_pending(&ex, &parent, &parent_future);
    match state {
        ChildState::Pending => {
            assert!(outcome == DriveOutcome::Parked);
            assert_eq!(child.sm_waiters.lock().unwrap().len(), 1);
            assert!(Arc::ptr_eq(&ex.ready.lock().unwrap()[0], &child));
            assert!(executor_complete(&ex, &child, 42) == DriveOutcome::Completed);
            assert!(Arc::ptr_eq(
                parent.pending_child.lock().unwrap().as_ref().unwrap(),
                &child
            ));
            assert!(!ex.task_map.lock().unwrap().contains_key(&child.sm_future));
        }
        ChildState::Done | ChildState::Registered => {
            assert!(outcome == DriveOutcome::Rerun);
        }
    }
    drop(parent);
    drop(child);
    drop(ex);
    assert!(weak_child.upgrade().is_none(), "child task was leaked");
    assert!(weak_parent.upgrade().is_none(), "parent task was leaked");
}

#[test]
fn completed_child_releases_waiter_references() {
    check_child_lifetime(ChildState::Pending);
}

#[test]
fn finished_child_releases_temporary_reference() {
    check_child_lifetime(ChildState::Done);
}

#[test]
fn duplicate_waiter_releases_temporary_reference() {
    check_child_lifetime(ChildState::Registered);
}

#[test]
fn freed_completed_child_survives_until_parent_consumes_it() {
    let ex = Arc::new(OliveExecutor {
        ready: Mutex::new(VecDeque::new()),
        wakeup: Condvar::new(),
        task_map: Mutex::new(std::collections::HashMap::new()),
        completed_tasks: Mutex::new(std::collections::HashMap::new()),
    });
    let mut child_frame = [0i64; 2];
    let child_future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: 0,
        frame: child_frame.as_mut_ptr() as i64,
        cancelled: AtomicI64::new(0),
        result_desc: [crate::format::D_INT].as_ptr() as i64,
        frame_size: 16,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let mut parent_frame = [0, &child_future as *const OliveSmFuture as i64];
    let parent_future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: 0,
        frame: parent_frame.as_mut_ptr() as i64,
        cancelled: AtomicI64::new(0),
        result_desc: [crate::format::D_INT].as_ptr() as i64,
        frame_size: 16,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let parent = executor_get_or_create_task(&ex, &parent_future as *const OliveSmFuture as i64);
    let child = executor_get_or_create_task(&ex, &child_future as *const OliveSmFuture as i64);
    let weak_child = Arc::downgrade(&child);

    assert!(park_after_pending(&ex, &parent, &parent_future) == DriveOutcome::Parked);
    assert!(executor_complete(&ex, &child, 42) == DriveOutcome::Completed);
    let pinned = parent.pending_child.lock().unwrap().take().unwrap();
    assert!(Arc::ptr_eq(&pinned, &child));

    // Another owner frees the completed child before the parent polls it.
    child.retired.store(true, Ordering::SeqCst);
    maybe_remove_retired_task(&ex, &child);
    assert!(
        ex.completed_tasks
            .lock()
            .unwrap()
            .contains_key(&child.sm_future),
        "pinned completed handle must not be released early"
    );
    assert!(weak_child.upgrade().is_some());

    // Parent consumes the handoff; the pin release reclaims the handle.
    release_child_pin(&ex, &pinned);
    assert!(
        !ex.completed_tasks
            .lock()
            .unwrap()
            .contains_key(&child.sm_future)
    );
    drop(pinned);
    drop(parent);
    drop(child);
    drop(ex);
    assert!(weak_child.upgrade().is_none(), "child task was leaked");
}

extern "C" fn make_bytes(_: *const i64) -> i64 {
    crate::bytes::new_buf(b"spawned bytes".to_vec())
}

extern "C" fn make_struct(_: *const i64) -> i64 {
    let value = crate::olive_struct_alloc(1);
    unsafe { *((value as *mut i64).add(1)) = make_bytes(std::ptr::null()) };
    value
}

fn await_callback(function: extern "C" fn(*const i64) -> i64, descriptor: &'static [u8]) -> i64 {
    let callback = Box::into_raw(Box::new([
        function as *const () as i64,
        0,
        descriptor.as_ptr() as i64,
    ])) as i64;
    let future = olive_spawn_task(callback);
    let result = olive_await_future(future);
    olive_free_future(future);
    result
}

#[test]
fn spawned_bytes_result_owns_escape_storage() {
    let result = await_callback(make_bytes, &[crate::format::D_BYTES]);
    assert!(crate::slab::chunk_is_global(result as usize));
    assert_eq!(crate::bytes::olive_buf_len(result), 13);
    assert_eq!(crate::bytes::olive_buf_get(result, 0), b's' as i64);
    crate::bytes::olive_buf_free(result);
}

#[test]
fn spawned_struct_result_uses_its_static_descriptor() {
    use crate::format::{D_BYTES, D_STRUCT};

    let descriptor = &[D_STRUCT, 14, b'S', 14, 14, b'b', D_BYTES];
    let result = await_callback(make_struct, descriptor);
    assert!(crate::slab::chunk_is_global(result as usize));
    let bytes = unsafe { *((result as *const i64).add(1)) };
    assert!(crate::slab::chunk_is_global(bytes as usize));
    assert_eq!(crate::bytes::olive_buf_len(bytes), 13);
    crate::free_typed::olive_free_typed(result, descriptor.as_ptr() as i64);
    assert!(!crate::slab::slot_is_live(result));
    assert!(!crate::slab::slot_is_live(bytes));
}

extern "C" fn return_argument(args: *const i64) -> i64 {
    unsafe { *args }
}

#[test]
fn spawned_return_releases_the_original_escape_allocation() {
    use crate::format::{D_INT, D_LIST};
    let descriptor = [D_LIST, D_INT];
    let original = crate::slab::with_escape_arena(|| crate::list::list_from_vec(vec![42]));
    let generation = crate::slab::slot_generation(original);
    let callback = Box::into_raw(Box::new([
        return_argument as *const () as i64,
        1,
        descriptor.as_ptr() as i64,
        original,
    ])) as i64;
    let future = olive_spawn_task(callback);
    let result = olive_await_future(future);
    assert!(crate::slab::slot_is_live(result));
    assert_ne!(crate::slab::slot_generation(original), generation);
    assert_eq!(crate::list::olive_list_get(result, 0), 42);
    crate::free_typed::olive_free_typed(result, descriptor.as_ptr() as i64);
    olive_free_future(future);
}

#[test]
fn completed_state_machine_releases_the_original_escape_allocation() {
    use crate::format::{D_INT, D_LIST};
    let descriptor = [D_LIST, D_INT];
    let original = crate::slab::with_escape_arena(|| crate::list::list_from_vec(vec![42]));
    let generation = crate::slab::slot_generation(original);
    let mut frame = [-1, original];
    let future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: 0,
        frame: frame.as_mut_ptr() as i64,
        cancelled: AtomicI64::new(0),
        result_desc: descriptor.as_ptr() as i64,
        frame_size: 16,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let ex = Arc::new(OliveExecutor {
        ready: Mutex::new(VecDeque::new()),
        wakeup: Condvar::new(),
        task_map: Mutex::new(std::collections::HashMap::new()),
        completed_tasks: Mutex::new(std::collections::HashMap::new()),
    });
    let task = executor_get_or_create_task(&ex, &future as *const OliveSmFuture as i64);
    assert!(executor_complete(&ex, &task, original) == DriveOutcome::Completed);
    assert_ne!(frame[1], original);
    assert_ne!(crate::slab::slot_generation(original), generation);
    assert_eq!(crate::list::olive_list_get(frame[1], 0), 42);
    crate::free_typed::olive_free_typed(frame[1], descriptor.as_ptr() as i64);
}

#[test]
fn completion_owns_result_after_future_release() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    use crate::format::{D_INT, D_LIST};

    let descriptor = [D_LIST, D_INT];
    let result_desc = descriptor.as_ptr() as i64;
    let original = crate::slab::with_escape_arena(|| crate::list::list_from_vec(vec![42]));
    let mut frame = [-1, original];
    let mut future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: 0,
        frame: frame.as_mut_ptr() as i64,
        cancelled: AtomicI64::new(0),
        result_desc,
        frame_size: 16,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let future_ptr = &mut future as *mut OliveSmFuture as i64;
    let ex = test_executor();
    let task = executor_get_or_create_task(&ex, future_ptr);
    let first_completion = Arc::new(Completion {
        result: Mutex::new(None),
        cvar: Condvar::new(),
    });
    let second_completion = Arc::new(Completion {
        result: Mutex::new(None),
        cvar: Condvar::new(),
    });
    task.completions
        .lock()
        .unwrap()
        .extend([first_completion.clone(), second_completion.clone()]);

    assert_eq!(
        executor_complete(&ex, &task, original),
        DriveOutcome::Completed
    );
    let cached = unsafe { (*(future_ptr as *const OliveSmFuture)).cached };
    let first_result = first_completion.result.lock().unwrap().take().unwrap();
    let second_result = second_completion.result.lock().unwrap().take().unwrap();
    assert_ne!(first_result, cached);
    assert_ne!(second_result, cached);
    assert_ne!(first_result, second_result);

    olive_free_future(future_ptr);
    assert!(!crate::slab::slot_is_live(cached));
    for result in [first_result, second_result] {
        assert!(crate::slab::slot_is_live(result));
        assert_eq!(crate::list::olive_list_get(result, 0), 42);
        crate::free_typed::olive_free_typed(result, result_desc);
    }
}

#[test]
fn poll_status_is_separate_from_payload_bits() {
    for value in [i64::MIN, 0, 42, (-0.0f64).to_bits() as i64] {
        let future = olive_make_future(value);
        let mut output = 99;
        assert_eq!(olive_sm_poll(future, &mut output as *mut i64 as i64), 1);
        assert_eq!(output, value);
        olive_free_future(future);
    }
}

#[test]
fn pending_poll_does_not_write_a_payload() {
    let future = olive_make_future(0);
    let shared = unsafe { &*((*(future as *const OliveFuture)).shared as *const FutureShared) };
    *shared.state.lock().unwrap() = FutureState::Pending;
    let mut output = 99;
    assert_eq!(olive_sm_poll(future, &mut output as *mut i64 as i64), 0);
    assert_eq!(output, 99);
    *shared.state.lock().unwrap() = FutureState::Ready(i64::MIN);
    assert_eq!(olive_sm_poll(future, &mut output as *mut i64 as i64), 1);
    assert_eq!(output, i64::MIN);
    olive_free_future(future);
}

#[test]
fn gather_and_select_accept_minimum_integer_payloads() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    let futures = [olive_make_future(i64::MIN), olive_make_future(42)];
    let list = crate::list::list_from_vec(futures.to_vec());
    let gathered = olive_gather(list);
    let selected = olive_select(list);
    crate::olive_free_list(list);
    let mut output = 0;
    assert_eq!(olive_sm_poll(gathered, &mut output as *mut i64 as i64), 1);
    let results = output;
    assert_eq!(crate::olive_list_get(results, 0), i64::MIN);
    assert_eq!(crate::olive_list_get(results, 1), 42);
    for _ in 0..2 {
        assert_eq!(olive_sm_poll(selected, &mut output as *mut i64 as i64), 1);
        assert_eq!(output, i64::MIN);
    }
    olive_free_future(gathered);
    olive_free_future(selected);
    crate::olive_free_list(results);
    for future in futures {
        olive_free_future(future);
    }
}

#[test]
fn pending_combinators_park_instead_of_requeueing() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    let ex = test_executor();

    let make_child = || {
        let frame = super::olive_sm_alloc(16);
        unsafe {
            *(frame as *mut i64) = 0;
            *((frame as *mut i64).add(1)) = 0;
        }
        let future = super::olive_sm_alloc(std::mem::size_of::<super::OliveSmFuture>() as i64);
        unsafe {
            std::ptr::write(
                future as *mut super::OliveSmFuture,
                super::OliveSmFuture {
                    kind: KIND_SM_FUTURE,
                    poll_fn: counting_suspend_once as *const () as usize as i64,
                    frame,
                    cancelled: AtomicI64::new(0),
                    result_desc: 0,
                    frame_size: 16,
                    cached: 0,
                    terminal: AtomicBool::new(false),
                    poll_lock: AtomicBool::new(false),
                },
            );
        }
        future
    };

    let gather_child = make_child();
    let gather_list = crate::list::list_from_vec(vec![gather_child]);
    let gathered = olive_gather(gather_list);
    let gathered_task = executor_get_or_create_task(&ex, gathered);
    let gather_child_task = executor_get_or_create_task(&ex, gather_child);
    assert!(matches!(
        executor_drive(&ex, &gathered_task),
        DriveOutcome::Parked
    ));
    assert!(matches!(
        executor_drive(&ex, &gather_child_task),
        DriveOutcome::Completed
    ));
    assert!(matches!(
        executor_drive(&ex, &gathered_task),
        DriveOutcome::Completed
    ));
    crate::olive_free_list(gather_list);
    olive_free_future(gathered);
    olive_free_future(gather_child);

    let select_child = make_child();
    let select_list = crate::list::list_from_vec(vec![select_child]);
    let selected = olive_select(select_list);
    let selected_task = executor_get_or_create_task(&ex, selected);
    let select_child_task = executor_get_or_create_task(&ex, select_child);
    assert!(matches!(
        executor_drive(&ex, &selected_task),
        DriveOutcome::Parked
    ));
    assert!(matches!(
        executor_drive(&ex, &select_child_task),
        DriveOutcome::Completed
    ));
    assert!(matches!(
        executor_drive(&ex, &selected_task),
        DriveOutcome::Completed
    ));
    crate::olive_free_list(select_list);
    olive_free_future(selected);
    olive_free_future(select_child);
}

#[test]
fn invalid_future_lists_are_safe() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    let gathered = olive_gather(2);
    let mut output = 0;
    assert_eq!(olive_sm_poll(gathered, &mut output as *mut i64 as i64), 1);
    assert_eq!(crate::olive_list_len(output), 0);
    olive_free_future(gathered);
    crate::olive_free_list(output);
    assert_eq!(olive_select(2), 0);
}

#[test]
fn empty_gather_returns_a_ready_future() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    let gathered = olive_gather(0);
    let mut output = 0;
    assert_eq!(olive_sm_poll(gathered, &mut output as *mut i64 as i64), 1);
    assert_eq!(crate::olive_list_len(output), 0);
    olive_free_future(gathered);
    crate::olive_free_list(output);
}

static CANCEL_COUNT: AtomicUsize = AtomicUsize::new(0);
static CANCEL_LOCK: Mutex<()> = Mutex::new(());

extern "C" fn counting_complete(frame: i64) -> i64 {
    CANCEL_COUNT.fetch_add(1, Ordering::SeqCst);
    unsafe {
        *(frame as *mut i64) = -1;
    }
    42
}

extern "C" fn counting_suspend_once(frame: i64) -> i64 {
    let state = unsafe { *(frame as *const i64) };
    if state == 0 {
        CANCEL_COUNT.fetch_add(1, Ordering::SeqCst);
        unsafe {
            *(frame as *mut i64) = 1;
        }
        super::POLL_PENDING
    } else {
        CANCEL_COUNT.fetch_add(1, Ordering::SeqCst);
        unsafe {
            *(frame as *mut i64) = -1;
        }
        42
    }
}

static EXCLUSIVE_POLL_CALLS: AtomicUsize = AtomicUsize::new(0);
static EXCLUSIVE_POLL_ENTERED: AtomicBool = AtomicBool::new(false);
static EXCLUSIVE_POLL_RELEASE: AtomicBool = AtomicBool::new(false);

extern "C" fn exclusive_blocking_poll(frame: i64) -> i64 {
    EXCLUSIVE_POLL_CALLS.fetch_add(1, Ordering::SeqCst);
    EXCLUSIVE_POLL_ENTERED.store(true, Ordering::Release);
    while !EXCLUSIVE_POLL_RELEASE.load(Ordering::Acquire) {
        std::hint::spin_loop();
    }
    unsafe {
        *(frame as *mut i64) = -1;
    }
    42
}

#[test]
fn direct_and_executor_polls_have_single_driver() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    EXCLUSIVE_POLL_CALLS.store(0, Ordering::SeqCst);
    EXCLUSIVE_POLL_ENTERED.store(false, Ordering::SeqCst);
    EXCLUSIVE_POLL_RELEASE.store(false, Ordering::SeqCst);

    let frame = olive_sm_alloc(16);
    unsafe {
        *(frame as *mut i64) = 0;
        *((frame as *mut i64).add(1)) = 0;
    }
    let future = olive_sm_alloc(std::mem::size_of::<OliveSmFuture>() as i64);
    unsafe {
        std::ptr::write(
            future as *mut OliveSmFuture,
            OliveSmFuture {
                kind: KIND_SM_FUTURE,
                poll_fn: exclusive_blocking_poll as *const () as usize as i64,
                frame,
                cancelled: AtomicI64::new(0),
                result_desc: 0,
                frame_size: 16,
                cached: 0,
                terminal: AtomicBool::new(false),
                poll_lock: AtomicBool::new(false),
            },
        );
    }

    let ex = test_executor();
    let task = executor_get_or_create_task(&ex, future);
    let worker_ex = ex.clone();
    let worker_task = task.clone();
    let first = std::thread::spawn(move || executor_drive(&worker_ex, &worker_task));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let entered = loop {
        if EXCLUSIVE_POLL_ENTERED.load(Ordering::Acquire) {
            break true;
        }
        if std::time::Instant::now() >= deadline {
            break false;
        }
        std::thread::yield_now();
    };
    if !entered {
        EXCLUSIVE_POLL_RELEASE.store(true, Ordering::Release);
        first.join().unwrap();
        panic!("first poll did not enter state machine");
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let second = std::thread::spawn(move || {
        let mut output: i64 = 99;
        let status = olive_sm_poll(future, &mut output as *mut i64 as i64);
        tx.send((status, output)).unwrap();
    });
    let second_result = rx.recv_timeout(std::time::Duration::from_secs(1));
    EXCLUSIVE_POLL_RELEASE.store(true, Ordering::Release);
    let first_result = first.join().unwrap();
    second.join().unwrap();

    assert_eq!(second_result, Ok((0, 99)));
    assert_eq!(first_result, DriveOutcome::Completed);
    assert_eq!(EXCLUSIVE_POLL_CALLS.load(Ordering::SeqCst), 1);
    let mut output: i64 = 0;
    assert_eq!(olive_sm_poll(future, &mut output as *mut i64 as i64), 1);
    assert_eq!(output, 42);
    olive_free_future(future);
}

#[test]
fn direct_poll_publishes_registered_task() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    let frame = olive_sm_alloc(16);
    unsafe {
        *(frame as *mut i64) = 0;
        *((frame as *mut i64).add(1)) = 0;
    }
    let future = olive_sm_alloc(std::mem::size_of::<OliveSmFuture>() as i64);
    unsafe {
        std::ptr::write(
            future as *mut OliveSmFuture,
            OliveSmFuture {
                kind: KIND_SM_FUTURE,
                poll_fn: counting_complete as *const () as usize as i64,
                frame,
                cancelled: AtomicI64::new(0),
                result_desc: 0,
                frame_size: 16,
                cached: 0,
                terminal: AtomicBool::new(false),
                poll_lock: AtomicBool::new(false),
            },
        );
    }

    let ex = olive_executor();
    executor_get_or_create_task(ex, future);
    let mut output: i64 = 0;
    assert_eq!(olive_sm_poll(future, &mut output as *mut i64 as i64), 1);
    assert_eq!(output, 42);
    assert!(!ex.task_map.lock().unwrap().contains_key(&future));
    olive_free_future(future);
}

fn test_executor() -> Arc<OliveExecutor> {
    Arc::new(OliveExecutor {
        ready: Mutex::new(VecDeque::new()),
        wakeup: Condvar::new(),
        task_map: Mutex::new(std::collections::HashMap::new()),
        completed_tasks: Mutex::new(std::collections::HashMap::new()),
    })
}

#[test]
fn cancel_before_first_poll_runs_no_poll() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    CANCEL_COUNT.store(0, Ordering::SeqCst);
    let mut frame = [0i64, 0i64];
    let mut future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: counting_complete as *const () as usize as i64,
        frame: frame.as_mut_ptr() as i64,
        cancelled: AtomicI64::new(0),
        result_desc: 0,
        frame_size: 16,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let future_ptr = &mut future as *mut OliveSmFuture as i64;
    let ex = test_executor();
    let task = executor_get_or_create_task(&ex, future_ptr);
    olive_cancel_future(future_ptr);
    assert!(executor_drive(&ex, &task) == DriveOutcome::Completed);
    assert_eq!(CANCEL_COUNT.load(Ordering::SeqCst), 0);
    assert_eq!(frame[0], -1);
    assert_eq!(frame[1], 0);
    assert!(!ex.task_map.lock().unwrap().contains_key(&future_ptr));
}

#[test]
fn cancel_after_suspension_runs_no_second_poll() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    CANCEL_COUNT.store(0, Ordering::SeqCst);
    let mut frame = [0i64, 0i64];
    let mut future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: counting_suspend_once as *const () as usize as i64,
        frame: frame.as_mut_ptr() as i64,
        cancelled: AtomicI64::new(0),
        result_desc: 0,
        frame_size: 16,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let future_ptr = &mut future as *mut OliveSmFuture as i64;
    let ex = test_executor();
    let task = executor_get_or_create_task(&ex, future_ptr);
    assert!(executor_drive(&ex, &task) == DriveOutcome::Rerun);
    assert_eq!(CANCEL_COUNT.load(Ordering::SeqCst), 1);
    olive_cancel_future(future_ptr);
    assert!(executor_drive(&ex, &task) == DriveOutcome::Completed);
    assert_eq!(CANCEL_COUNT.load(Ordering::SeqCst), 1);
    assert_eq!(frame[0], -1);
}

#[test]
fn cancel_notifies_waiter_with_zero() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    CANCEL_COUNT.store(0, Ordering::SeqCst);
    let mut child_frame = [0i64, 0i64];
    let mut child_future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: counting_complete as *const () as usize as i64,
        frame: child_frame.as_mut_ptr() as i64,
        cancelled: AtomicI64::new(0),
        result_desc: 0,
        frame_size: 16,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let child_ptr = &mut child_future as *mut OliveSmFuture as i64;
    let mut parent_frame = [0, child_ptr];
    let mut parent_future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: 0,
        frame: parent_frame.as_mut_ptr() as i64,
        cancelled: AtomicI64::new(0),
        result_desc: 0,
        frame_size: 16,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let parent_ptr = &mut parent_future as *mut OliveSmFuture as i64;
    let ex = test_executor();
    let parent = executor_get_or_create_task(&ex, parent_ptr);
    let child = executor_get_or_create_task(&ex, child_ptr);
    assert!(park_after_pending(&ex, &parent, &parent_future) == DriveOutcome::Parked);
    olive_cancel_future(child_ptr);
    assert!(executor_drive(&ex, &child) == DriveOutcome::Completed);
    assert_eq!(CANCEL_COUNT.load(Ordering::SeqCst), 0);
    assert!(Arc::ptr_eq(
        parent.pending_child.lock().unwrap().as_ref().unwrap(),
        &child
    ));
}

#[test]
fn completed_sm_future_can_be_awaited_twice() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    let frame = super::olive_sm_alloc(16);
    unsafe {
        *(frame as *mut i64) = 0;
        *((frame as *mut i64).add(1)) = 0;
    }
    let future = super::olive_sm_alloc(std::mem::size_of::<super::OliveSmFuture>() as i64);
    unsafe {
        std::ptr::write(
            future as *mut super::OliveSmFuture,
            super::OliveSmFuture {
                kind: KIND_SM_FUTURE,
                poll_fn: counting_complete as *const () as usize as i64,
                frame,
                cancelled: AtomicI64::new(0),
                result_desc: 0,
                frame_size: 16,
                cached: 0,
                terminal: AtomicBool::new(false),
                poll_lock: AtomicBool::new(false),
            },
        );
    }

    assert_eq!(olive_await_future(future), 42);
    assert_eq!(olive_await_future(future), 42);
    olive_free_future(future);
    wait_for_sm_free(future);
}

#[test]
fn completed_sm_future_remains_pinned_until_poll_guard_exits() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    let frame = super::olive_sm_alloc(16);
    unsafe {
        *(frame as *mut i64) = -1;
        *((frame as *mut i64).add(1)) = 0;
    }
    let future = super::olive_sm_alloc(std::mem::size_of::<super::OliveSmFuture>() as i64);
    unsafe {
        std::ptr::write(
            future as *mut super::OliveSmFuture,
            super::OliveSmFuture {
                kind: KIND_SM_FUTURE,
                poll_fn: counting_complete as *const () as usize as i64,
                frame,
                cancelled: AtomicI64::new(0),
                result_desc: 0,
                frame_size: 16,
                cached: 0,
                terminal: AtomicBool::new(false),
                poll_lock: AtomicBool::new(false),
            },
        );
    }

    let ex = olive_executor();
    let task = executor_get_or_create_task(ex, future);
    let poll_guard = super::try_acquire_sm_poll(future).unwrap();
    assert_eq!(executor_publish(ex, &task, 42), DriveOutcome::Completed);
    assert!(ex.completed_tasks.lock().unwrap().contains_key(&future));

    olive_free_future(future);
    assert!(!ex.completed_tasks.lock().unwrap().contains_key(&future));
    assert!(super::sm_live().lock().unwrap().contains(&future));

    drop(poll_guard);
    drop(task);
    assert!(!super::sm_live().lock().unwrap().contains(&future));
}

#[test]
fn freeing_running_sm_future_defers_handle_release() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    EXCLUSIVE_POLL_CALLS.store(0, Ordering::SeqCst);
    EXCLUSIVE_POLL_ENTERED.store(false, Ordering::SeqCst);
    EXCLUSIVE_POLL_RELEASE.store(false, Ordering::SeqCst);
    let frame = super::olive_sm_alloc(16);
    unsafe {
        *(frame as *mut i64) = 0;
        *((frame as *mut i64).add(1)) = 0;
    }
    let future = super::olive_sm_alloc(std::mem::size_of::<super::OliveSmFuture>() as i64);
    unsafe {
        std::ptr::write(
            future as *mut super::OliveSmFuture,
            super::OliveSmFuture {
                kind: KIND_SM_FUTURE,
                poll_fn: exclusive_blocking_poll as *const () as usize as i64,
                frame,
                cancelled: AtomicI64::new(0),
                result_desc: 0,
                frame_size: 16,
                cached: 0,
                terminal: AtomicBool::new(false),
                poll_lock: AtomicBool::new(false),
            },
        );
    }

    let waiter = std::thread::spawn(move || olive_await_future(future));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !EXCLUSIVE_POLL_ENTERED.load(Ordering::Acquire) {
        if std::time::Instant::now() >= deadline {
            EXCLUSIVE_POLL_RELEASE.store(true, Ordering::Release);
            panic!("future poll did not start");
        }
        std::thread::yield_now();
    }

    olive_free_future(future);
    assert!(super::sm_live().lock().unwrap().contains(&future));
    EXCLUSIVE_POLL_RELEASE.store(true, Ordering::Release);
    assert_eq!(waiter.join().unwrap(), 42);
    wait_for_sm_free(future);
}

#[test]
fn cancel_parent_unregisters_from_child_waiters() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    let ex = test_executor();

    let child_frame = super::olive_sm_alloc(16);
    unsafe {
        *(child_frame as *mut i64) = 0;
        *((child_frame as *mut i64).add(1)) = 0;
    }
    let child_future = super::olive_sm_alloc(std::mem::size_of::<super::OliveSmFuture>() as i64);
    unsafe {
        std::ptr::write(
            child_future as *mut super::OliveSmFuture,
            super::OliveSmFuture {
                kind: KIND_SM_FUTURE,
                poll_fn: counting_complete as *const () as usize as i64,
                frame: child_frame,
                cancelled: AtomicI64::new(0),
                result_desc: 0,
                frame_size: 16,
                cached: 0,
                terminal: AtomicBool::new(false),
                poll_lock: AtomicBool::new(false),
            },
        );
    }

    let parent_frame = super::olive_sm_alloc(16);
    unsafe {
        *(parent_frame as *mut i64) = 0;
        *((parent_frame as *mut i64).add(1)) = child_future;
    }
    let parent_future = super::olive_sm_alloc(std::mem::size_of::<super::OliveSmFuture>() as i64);
    unsafe {
        std::ptr::write(
            parent_future as *mut super::OliveSmFuture,
            super::OliveSmFuture {
                kind: KIND_SM_FUTURE,
                poll_fn: 0,
                frame: parent_frame,
                cancelled: AtomicI64::new(0),
                result_desc: 0,
                frame_size: 16,
                cached: 0,
                terminal: AtomicBool::new(false),
                poll_lock: AtomicBool::new(false),
            },
        );
    }

    let parent = executor_get_or_create_task(&ex, parent_future);
    let child = executor_get_or_create_task(&ex, child_future);
    assert!(matches!(
        park_after_pending(&ex, &parent, unsafe {
            &*(parent_future as *const super::OliveSmFuture)
        },),
        DriveOutcome::Parked
    ));
    assert_eq!(child.sm_waiters.lock().unwrap().len(), 1);

    olive_cancel_future(parent_future);
    assert!(matches!(
        executor_drive(&ex, &parent),
        DriveOutcome::Completed
    ));
    assert!(child.sm_waiters.lock().unwrap().is_empty());

    assert!(matches!(
        executor_drive(&ex, &child),
        DriveOutcome::Completed
    ));
    olive_free_future(child_future);
}

#[test]
fn cancel_plain_future_unblocks_with_zero() {
    let future = olive_make_future(0);
    let shared = unsafe { &*((*(future as *const OliveFuture)).shared as *const FutureShared) };
    *shared.state.lock().unwrap() = FutureState::Pending;
    olive_cancel_future(future);
    let mut output: i64 = 99;
    assert_eq!(olive_sm_poll(future, &mut output as *mut i64 as i64), 1);
    assert_eq!(output, 0);
    olive_free_future(future);
}

#[test]
fn sm_alloc_free_counts_balance() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    let (a0, f0) = super::sm_alloc_free_counts();
    for _ in 0..16 {
        let list = crate::list::list_from_vec(vec![olive_make_future(1), olive_make_future(2)]);
        let gathered = olive_gather(list);
        let mut output: i64 = 0;
        assert_eq!(olive_sm_poll(gathered, &mut output as *mut i64 as i64), 1);
        crate::olive_free_list(output);
        olive_free_future(gathered);
        for i in 0..2 {
            olive_free_future(crate::olive_list_get(list, i));
        }
        crate::olive_free_list(list);
    }
    let (a1, f1) = super::sm_alloc_free_counts();
    assert_eq!(a1 - a0, f1 - f0);
    assert_eq!(a1 - a0, 32);
}

#[test]
fn sm_frame_reclaimed_on_executor_complete() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    CANCEL_COUNT.store(0, Ordering::SeqCst);
    let (a0, f0) = super::sm_alloc_free_counts();
    let ex = test_executor();
    let frame = olive_sm_alloc(16);
    unsafe {
        *(frame as *mut i64) = 0;
        *((frame as *mut i64).add(1)) = 0;
    }
    let fut = olive_sm_alloc(std::mem::size_of::<OliveSmFuture>() as i64);
    unsafe {
        std::ptr::write(
            fut as *mut OliveSmFuture,
            OliveSmFuture {
                kind: KIND_SM_FUTURE,
                poll_fn: counting_complete as *const () as usize as i64,
                frame,
                cancelled: AtomicI64::new(0),
                result_desc: 0,
                frame_size: 16,
                cached: 0,
                terminal: AtomicBool::new(false),
                poll_lock: AtomicBool::new(false),
            },
        );
    }
    let task = executor_get_or_create_task(&ex, fut);
    CANCEL_COUNT.store(0, Ordering::SeqCst);
    assert!(executor_drive(&ex, &task) == DriveOutcome::Completed);
    assert_eq!(CANCEL_COUNT.load(Ordering::SeqCst), 1);
    let (a1, f1) = super::sm_alloc_free_counts();
    assert_eq!(a1 - a0, 2);
    assert_eq!(f1 - f0, 1);
    let mut output: i64 = 99;
    assert_eq!(olive_sm_poll(fut, &mut output as *mut i64 as i64), 1);
    olive_free_future(fut);
    let (a2, f2) = super::sm_alloc_free_counts();
    assert_eq!(a2 - a0, 2);
    assert_eq!(f2 - f0, 2);
}

#[test]
fn double_cancel_is_idempotent() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    CANCEL_COUNT.store(0, Ordering::SeqCst);
    let mut frame = [0i64, 0i64];
    let mut future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: counting_complete as *const () as usize as i64,
        frame: frame.as_mut_ptr() as i64,
        cancelled: AtomicI64::new(0),
        result_desc: 0,
        frame_size: 16,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let future_ptr = &mut future as *mut OliveSmFuture as i64;
    let ex = test_executor();
    let task = executor_get_or_create_task(&ex, future_ptr);
    olive_cancel_future(future_ptr);
    assert!(executor_drive(&ex, &task) == DriveOutcome::Completed);
    assert_eq!(CANCEL_COUNT.load(Ordering::SeqCst), 0);
    olive_cancel_future(future_ptr);
    assert_eq!(CANCEL_COUNT.load(Ordering::SeqCst), 0);
    assert_eq!(frame[0], -1);
    assert!(!ex.task_map.lock().unwrap().contains_key(&future_ptr));
}

#[test]
fn cancel_after_natural_completion_is_harmless() {
    let _guard = CANCEL_LOCK.lock().unwrap();
    CANCEL_COUNT.store(0, Ordering::SeqCst);
    let mut frame = [0i64, 0i64];
    let mut future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: counting_complete as *const () as usize as i64,
        frame: frame.as_mut_ptr() as i64,
        cancelled: AtomicI64::new(0),
        result_desc: 0,
        frame_size: 16,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(false),
    };
    let future_ptr = &mut future as *mut OliveSmFuture as i64;
    let ex = test_executor();
    let task = executor_get_or_create_task(&ex, future_ptr);
    assert!(executor_drive(&ex, &task) == DriveOutcome::Completed);
    assert_eq!(CANCEL_COUNT.load(Ordering::SeqCst), 1);
    olive_cancel_future(future_ptr);
    assert_eq!(CANCEL_COUNT.load(Ordering::SeqCst), 1);
    assert_eq!(frame[0], -1);
}
