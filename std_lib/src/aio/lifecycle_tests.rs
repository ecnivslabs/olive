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
    };
    let mut parent_frame = [0, &child_future as *const OliveSmFuture as i64];
    let parent_future = OliveSmFuture {
        kind: KIND_SM_FUTURE,
        poll_fn: 0,
        frame: parent_frame.as_mut_ptr() as i64,
        cancelled: 0,
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
