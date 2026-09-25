use super::*;
use std::sync::atomic::{AtomicBool, AtomicI64};

fn test_executor() -> Arc<OliveExecutor> {
    Arc::new(OliveExecutor {
        ready: Mutex::new(VecDeque::new()),
        wakeup: Condvar::new(),
        task_map: Mutex::new(std::collections::HashMap::new()),
        completed_tasks: Mutex::new(std::collections::HashMap::new()),
    })
}

fn test_future(frame: i64, locked: bool) -> OliveSmFuture {
    OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: 0,
        frame,
        cancelled: AtomicI64::new(0),
        result_desc: 0,
        frame_size: 0,
        cached: 0,
        terminal: AtomicBool::new(false),
        poll_lock: AtomicBool::new(locked),
    }
}

#[test]
fn contended_parent_poll_restores_existing_child_pin() {
    let ex = test_executor();
    let parent_future = test_future(0, true);
    let child_future = test_future(0, false);
    let parent = executor_get_or_create_task(&ex, &parent_future as *const _ as i64);
    let child = executor_get_or_create_task(&ex, &child_future as *const _ as i64);
    install_pending_child(&parent, &child);
    let taken = parent.pending_child.lock().unwrap().take().unwrap();
    assert!(!executor_complete_waker(&ex, &parent, taken.clone()));
    assert_eq!(child.handle_pins.load(Ordering::Acquire), 1);
    assert!(Arc::ptr_eq(
        parent.pending_child.lock().unwrap().as_ref().unwrap(),
        &child
    ));
    let restored = parent.pending_child.lock().unwrap().take().unwrap();
    release_child_pin(&ex, &restored);
    assert_eq!(child.handle_pins.load(Ordering::Acquire), 0);
}

#[test]
fn awaiting_child_preserves_its_pending_delivery() {
    let ex = test_executor();
    let grandchild_future = test_future(0, false);
    let child_future = test_future(0, false);
    let mut parent_frame = [0, &child_future as *const _ as i64];
    let parent_future = test_future(parent_frame.as_mut_ptr() as i64, false);
    let parent = executor_get_or_create_task(&ex, &parent_future as *const _ as i64);
    let child = executor_get_or_create_task(&ex, &child_future as *const _ as i64);
    let grandchild = executor_get_or_create_task(&ex, &grandchild_future as *const _ as i64);
    install_pending_child(&child, &grandchild);
    assert_eq!(
        park_after_pending(&ex, &parent, &parent_future),
        DriveOutcome::Parked
    );
    assert_eq!(grandchild.handle_pins.load(Ordering::Acquire), 1);
    assert!(Arc::ptr_eq(
        child.pending_child.lock().unwrap().as_ref().unwrap(),
        &grandchild
    ));
    unregister_one_child(&ex, &parent, child.sm_future);
    let pending = child.pending_child.lock().unwrap().take().unwrap();
    release_child_pin(&ex, &pending);
}
