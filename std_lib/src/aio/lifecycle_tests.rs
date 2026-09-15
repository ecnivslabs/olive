use super::*;

enum ChildState {
    Pending,
    Delivered,
    Done,
    Registered,
}

fn check_child_lifetime(state: ChildState) {
    let ex = Arc::new(OliveExecutor {
        ready: Mutex::new(VecDeque::new()),
        wakeup: Condvar::new(),
        task_map: Mutex::new(std::collections::HashMap::new()),
    });
    let mut child_frame = [0i64; 2];
    let child_future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: 0,
        frame: child_frame.as_mut_ptr() as i64,
        cancelled: 0,
        result_desc: [crate::format::D_INT].as_ptr() as i64,
    };
    let mut parent_frame = [0, &child_future as *const OliveSmFuture as i64];
    let parent_future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: 0,
        frame: parent_frame.as_mut_ptr() as i64,
        cancelled: 0,
        result_desc: [crate::format::D_INT].as_ptr() as i64,
    };
    let parent = executor_get_or_create_task(&ex, &parent_future as *const OliveSmFuture as i64);
    let child = executor_get_or_create_task(&ex, &child_future as *const OliveSmFuture as i64);
    let weak_child = Arc::downgrade(&child);
    let weak_parent = Arc::downgrade(&parent);
    match state {
        ChildState::Pending => {}
        ChildState::Delivered => *child.pending_result.lock().unwrap() = Some(42),
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
            assert_eq!(*parent.pending_result.lock().unwrap(), Some(42));
            assert!(!ex.task_map.lock().unwrap().contains_key(&child.sm_future));
        }
        ChildState::Delivered => {
            assert!(outcome == DriveOutcome::Rerun);
            assert_eq!(*parent.pending_result.lock().unwrap(), Some(42));
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
fn delivered_child_releases_temporary_reference() {
    check_child_lifetime(ChildState::Delivered);
}

#[test]
fn finished_child_releases_temporary_reference() {
    check_child_lifetime(ChildState::Done);
}

#[test]
fn duplicate_waiter_releases_temporary_reference() {
    check_child_lifetime(ChildState::Registered);
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
    assert_ne!(result, original);
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
        cancelled: 0,
        result_desc: descriptor.as_ptr() as i64,
    };
    let ex = Arc::new(OliveExecutor {
        ready: Mutex::new(VecDeque::new()),
        wakeup: Condvar::new(),
        task_map: Mutex::new(std::collections::HashMap::new()),
    });
    let task = executor_get_or_create_task(&ex, &future as *const OliveSmFuture as i64);
    assert!(executor_complete(&ex, &task, original) == DriveOutcome::Completed);
    assert_ne!(frame[1], original);
    assert_ne!(crate::slab::slot_generation(original), generation);
    assert_eq!(crate::list::olive_list_get(frame[1], 0), 42);
    crate::free_typed::olive_free_typed(frame[1], descriptor.as_ptr() as i64);
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
    let futures = [olive_make_future(i64::MIN), olive_make_future(42)];
    let list = crate::list::list_from_vec(futures.to_vec());
    let gathered = olive_gather(list);
    let selected = olive_select(list);
    let mut output = 0;
    assert_eq!(olive_sm_poll(gathered, &mut output as *mut i64 as i64), 1);
    let results = output;
    assert_eq!(crate::olive_list_get(results, 0), i64::MIN);
    assert_eq!(crate::olive_list_get(results, 1), 42);
    for _ in 0..2 {
        assert_eq!(olive_sm_poll(selected, &mut output as *mut i64 as i64), 1);
        assert_eq!(output, i64::MIN);
    }
    unsafe {
        let future = Box::from_raw(gathered as *mut OliveSmFuture);
        drop(Box::from_raw(future.frame as *mut GatherFrame));
        let future = Box::from_raw(selected as *mut OliveSmFuture);
        drop(Box::from_raw(future.frame as *mut SelectFrame));
    }
    crate::olive_free_list(results);
    crate::olive_free_list(list);
    for future in futures {
        olive_free_future(future);
    }
}

#[test]
fn empty_gather_returns_a_ready_future() {
    let gathered = olive_gather(0);
    let mut output = 0;
    assert_eq!(olive_sm_poll(gathered, &mut output as *mut i64 as i64), 1);
    assert_eq!(crate::olive_list_len(output), 0);
    unsafe {
        let future = Box::from_raw(gathered as *mut OliveSmFuture);
        drop(Box::from_raw(future.frame as *mut GatherFrame));
    }
    crate::olive_free_list(output);
}
