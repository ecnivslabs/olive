use super::engine::{DebugEvent, StopReason};
use super::launch::{DebugSession, launch};
use crate::test_utils::exec_lock;
use std::sync::atomic::{AtomicU64, Ordering};

fn temp_file(src: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("olive_dap_d2_{}_{}.liv", std::process::id(), id));
    std::fs::write(&path, src).unwrap();
    path
}

/// Resumes past whatever the session is currently doing until it exits,
/// draining and re-continuing past any breakpoint that fires along the way.
fn run_to_exit(session: &DebugSession) {
    session.cont();
    loop {
        match session.events().recv().unwrap() {
            DebugEvent::Exited(_) => return,
            DebugEvent::Stopped { .. } => session.cont(),
        }
    }
}

#[test]
fn breakpoint_stops_at_correct_line_and_function() {
    let _guard = exec_lock();
    let path = temp_file("fn main():\n    let x = 1\n    let y = 2\n    print(x + y)\n");
    let session = launch(path.to_str().unwrap(), false).expect("launch failed");

    let result = session.set_breakpoints(0, &[3]);
    assert_eq!(result, vec![(3, true)]);

    session.cont();
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { reason, frame } => {
            assert_eq!(reason, StopReason::Breakpoint);
            assert_eq!(frame.line, 3);
            assert_eq!(frame.name, "main");
            assert_eq!(frame.file, path.to_str().unwrap());
        }
        other => panic!("expected Stopped, got {other:?}"),
    }

    let stack = session.stack();
    assert_eq!(stack.len(), 1);
    assert_eq!(stack[0].line, 3);
    assert_eq!(stack[0].name, "main");
    assert_eq!(stack[0].file, path.to_str().unwrap());

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn cont_with_no_breakpoints_runs_to_exit_code_zero() {
    let _guard = exec_lock();
    let path = temp_file("fn main():\n    print(1)\n");
    let session = launch(path.to_str().unwrap(), false).unwrap();

    session.cont();
    match session.events().recv().unwrap() {
        DebugEvent::Exited(code) => assert_eq!(code, 0),
        other => panic!("expected Exited, got {other:?}"),
    }
    std::fs::remove_file(&path).ok();
}

#[test]
fn breakpoint_on_blank_line_snaps_to_next_instrumented_line() {
    let _guard = exec_lock();
    let path = temp_file("fn main():\n\n    let x = 1\n    print(x)\n");
    let session = launch(path.to_str().unwrap(), false).unwrap();

    let result = session.set_breakpoints(0, &[2]);
    assert_eq!(result, vec![(3, true)]);

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn breakpoint_past_last_line_is_unverified() {
    let _guard = exec_lock();
    let path = temp_file("fn main():\n    print(1)\n");
    let session = launch(path.to_str().unwrap(), false).unwrap();

    let result = session.set_breakpoints(0, &[99]);
    assert_eq!(result, vec![(99, false)]);

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn three_deep_call_chain_shows_three_frames() {
    let _guard = exec_lock();
    let src = "fn b() -> int:\n    return 1\nfn a() -> int:\n    return b()\nfn main():\n    print(a())\n";
    let path = temp_file(src);
    let session = launch(path.to_str().unwrap(), false).unwrap();

    let result = session.set_breakpoints(0, &[2]); // `return 1` inside b()
    assert_eq!(result, vec![(2, true)]);

    session.cont();
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { .. } => {}
        other => panic!("expected Stopped, got {other:?}"),
    }

    let stack = session.stack();
    let names: Vec<&str> = stack.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["b", "a", "main"]);

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn recursion_shows_repeated_frames() {
    let _guard = exec_lock();
    let src = "fn countdown(n: int) -> int:\n    if n <= 0:\n        return 0\n    return countdown(n - 1)\nfn main():\n    print(countdown(3))\n";
    let path = temp_file(src);
    let session = launch(path.to_str().unwrap(), false).unwrap();

    let result = session.set_breakpoints(0, &[3]); // base-case `return 0`
    assert_eq!(result, vec![(3, true)]);

    session.cont();
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { .. } => {}
        other => panic!("expected Stopped, got {other:?}"),
    }

    let stack = session.stack();
    let countdown: Vec<_> = stack.iter().filter(|f| f.name == "countdown").collect();
    assert_eq!(countdown.len(), 4, "n=3,2,1,0 each push a countdown frame");
    assert!(
        countdown.iter().all(|f| f.fn_id == countdown[0].fn_id),
        "every recursive call is the same function, so fn_id must match across frames"
    );
    assert_eq!(stack.last().unwrap().name, "main");
    assert_ne!(stack.last().unwrap().fn_id, countdown[0].fn_id);

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn stop_on_entry_halts_before_first_statement() {
    let _guard = exec_lock();
    let path = temp_file("fn main():\n    print(1)\n");
    let session = launch(path.to_str().unwrap(), true).unwrap();

    // Releases the initial start barrier; stop-on-entry re-stops immediately
    // at the first real statement rather than letting the program run.
    session.cont();
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { reason, .. } => assert_eq!(reason, StopReason::Entry),
        other => panic!("expected Stopped(Entry), got {other:?}"),
    }

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn next_over_a_call_lands_on_the_next_line_not_inside_it() {
    let _guard = exec_lock();
    let src = "fn helper():\n    print(0)\nfn main():\n    helper()\n    print(1)\n";
    let path = temp_file(src);
    let session = launch(path.to_str().unwrap(), false).unwrap();

    let result = session.set_breakpoints(0, &[4]); // `helper()`
    assert_eq!(result, vec![(4, true)]);
    session.cont();
    session.events().recv().unwrap();

    session.next();
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { reason, frame } => {
            assert_eq!(reason, StopReason::Step);
            assert_eq!(frame.name, "main");
            assert_eq!(frame.line, 5);
        }
        other => panic!("expected Stopped, got {other:?}"),
    }

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn step_in_lands_on_the_callees_first_line() {
    let _guard = exec_lock();
    let src = "fn helper():\n    print(0)\nfn main():\n    helper()\n    print(1)\n";
    let path = temp_file(src);
    let session = launch(path.to_str().unwrap(), false).unwrap();

    let result = session.set_breakpoints(0, &[4]); // `helper()`
    assert_eq!(result, vec![(4, true)]);
    session.cont();
    session.events().recv().unwrap();

    session.step_in();
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { reason, frame } => {
            assert_eq!(reason, StopReason::Step);
            assert_eq!(frame.name, "helper");
            assert_eq!(frame.line, 2);
        }
        other => panic!("expected Stopped, got {other:?}"),
    }

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn step_out_returns_to_the_callers_line() {
    let _guard = exec_lock();
    let src = "fn helper():\n    print(0)\nfn main():\n    helper()\n    print(1)\n";
    let path = temp_file(src);
    let session = launch(path.to_str().unwrap(), false).unwrap();

    let result = session.set_breakpoints(0, &[2]); // `let z = 1` inside helper
    assert_eq!(result, vec![(2, true)]);
    session.cont();
    session.events().recv().unwrap();

    session.step_out();
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { reason, frame } => {
            assert_eq!(reason, StopReason::Step);
            assert_eq!(frame.name, "main");
            assert_eq!(frame.line, 5);
        }
        other => panic!("expected Stopped, got {other:?}"),
    }

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn stepping_past_the_last_line_of_main_yields_exited() {
    let _guard = exec_lock();
    let path = temp_file("fn main():\n    print(1)\n");
    let session = launch(path.to_str().unwrap(), false).unwrap();

    let result = session.set_breakpoints(0, &[2]);
    assert_eq!(result, vec![(2, true)]);
    session.cont();
    session.events().recv().unwrap();

    session.next();
    match session.events().recv().unwrap() {
        DebugEvent::Exited(code) => assert_eq!(code, 0),
        other => panic!("expected Exited, got {other:?}"),
    }
    std::fs::remove_file(&path).ok();
}

#[test]
fn pause_during_a_long_loop_stops_inside_it() {
    let _guard = exec_lock();
    let src = "fn main():\n    let mut i = 0\n    while i < 100000000:\n        i = i + 1\n    print(i)\n";
    let path = temp_file(src);
    let session = launch(path.to_str().unwrap(), false).unwrap();

    session.cont();
    std::thread::sleep(std::time::Duration::from_millis(50));
    session.pause();
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { reason, frame } => {
            assert_eq!(reason, StopReason::Pause);
            assert_eq!(frame.name, "main");
            // Pause stops at whichever stmt hook fires next -- the loop
            // condition (line 3) or its body (line 4) -- both are inside it.
            assert!(frame.line == 3 || frame.line == 4);
        }
        other => panic!("expected Stopped, got {other:?}"),
    }

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn relaunch_while_active_errors() {
    let _guard = exec_lock();
    let path = temp_file("fn main():\n    let x = 1\n    print(x)\n");
    let session = launch(path.to_str().unwrap(), false).unwrap();
    session.set_breakpoints(0, &[2]);
    session.cont();
    session.events().recv().unwrap(); // parked at breakpoint, session still active

    let path2 = temp_file("fn main():\n    print(2)\n");
    assert!(launch(path2.to_str().unwrap(), false).is_err());

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
    std::fs::remove_file(&path2).ok();
}
