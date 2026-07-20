use super::engine::{DebugEvent, StopReason};
use super::hooks;
use super::launch::{DebugSession, launch};
use super::values::{self, Variable};
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
    session.cont(1);
    loop {
        match session.events().recv().unwrap() {
            DebugEvent::Exited(_) => return,
            DebugEvent::Stopped { .. } => session.cont(1),
            DebugEvent::Output(_) => {}
            DebugEvent::ThreadStarted { .. } | DebugEvent::ThreadExited { .. } => {}
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

    session.cont(1);
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { reason, frame, .. } => {
            assert_eq!(reason, StopReason::Breakpoint);
            assert_eq!(frame.line, 3);
            assert_eq!(frame.name, "main");
            assert_eq!(frame.file, path.to_str().unwrap());
        }
        other => panic!("expected Stopped, got {other:?}"),
    }

    let stack = session.stack(1);
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

    session.cont(1);
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

    session.cont(1);
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { .. } => {}
        other => panic!("expected Stopped, got {other:?}"),
    }

    let stack = session.stack(1);
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

    session.cont(1);
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { .. } => {}
        other => panic!("expected Stopped, got {other:?}"),
    }

    let stack = session.stack(1);
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
    session.cont(1);
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
    session.cont(1);
    session.events().recv().unwrap();

    session.next(1);
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { reason, frame, .. } => {
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
    session.cont(1);
    session.events().recv().unwrap();

    session.step_in(1);
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { reason, frame, .. } => {
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
    session.cont(1);
    session.events().recv().unwrap();

    session.step_out(1);
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { reason, frame, .. } => {
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
    session.cont(1);
    session.events().recv().unwrap();

    session.next(1);
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

    session.cont(1);
    std::thread::sleep(std::time::Duration::from_millis(50));
    session.pause(1);
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { reason, frame, .. } => {
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
    session.cont(1);
    session.events().recv().unwrap(); // parked at breakpoint, session still active

    let path2 = temp_file("fn main():\n    print(2)\n");
    assert!(launch(path2.to_str().unwrap(), false).is_err());

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
    std::fs::remove_file(&path2).ok();
}

fn var<'a>(vars: &'a [Variable], name: &str) -> &'a Variable {
    vars.iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no variable named {name} in {vars:?}"))
}

impl std::fmt::Debug for Variable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} = {}", self.name, self.type_name, self.value)
    }
}

const VARS_SRC: &str = "\
struct Point:
    x: int
    y: int
    fn __init__(self, x: int, y: int):
        self.x = x
        self.y = y

enum Shape:
    Circle(float)
    Square(int, int)

fn main():
    let n = 42
    let f = 3.5
    let b = True
    let s = \"hi\"
    let xs = [1, 2, 3]
    let d = {\"a\": 1, \"b\": 2}
    let p = Point(1, 2)
    let sh = Circle(2.5)
    let pts = [Point(1, 2), Point(3, 4)]
    print(n)
    print(f)
    print(b)
    print(s)
    print(xs)
    print(d)
    print(p)
    print(sh)
    print(pts)
";

#[test]
fn frame_variables_render_every_scalar_and_collection_kind() {
    let _guard = exec_lock();
    let path = temp_file(VARS_SRC);
    let session = launch(path.to_str().unwrap(), false).unwrap();

    let result = session.set_breakpoints(0, &[22]); // `print(n)`
    assert_eq!(result, vec![(22, true)]);
    session.cont(1);
    session.events().recv().unwrap();

    let vars = values::frame_variables(&session, 0);

    assert_eq!(var(&vars, "n").value, "42");
    assert_eq!(var(&vars, "n").type_name, "int");
    assert_eq!(var(&vars, "n").reference, 0);

    assert_eq!(var(&vars, "f").value, "3.5");
    assert_eq!(var(&vars, "b").value, "True");
    assert_eq!(var(&vars, "s").value, "\"hi\"");

    let xs = var(&vars, "xs");
    assert_eq!(xs.value, "[1, 2, 3]");
    assert_ne!(xs.reference, 0);
    let xs_children = values::children(&session, xs.reference);
    let child_values: Vec<&str> = xs_children.iter().map(|v| v.value.as_str()).collect();
    assert_eq!(child_values, vec!["1", "2", "3"]);
    assert_eq!(
        xs_children
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>(),
        vec!["0", "1", "2"]
    );

    let d = var(&vars, "d");
    assert_ne!(d.reference, 0);
    let d_children = values::children(&session, d.reference);
    assert_eq!(d_children.len(), 2);
    assert!(
        d_children
            .iter()
            .any(|v| v.name == "\"a\"" && v.value == "1")
    );
    assert!(
        d_children
            .iter()
            .any(|v| v.name == "\"b\"" && v.value == "2")
    );

    let p = var(&vars, "p");
    assert_eq!(p.value, "Point(x=1, y=2)");
    assert_ne!(p.reference, 0);
    let p_children = values::children(&session, p.reference);
    assert_eq!(p_children.len(), 2);
    assert_eq!(var(&p_children, "x").value, "1");
    assert_eq!(var(&p_children, "y").value, "2");

    let sh = var(&vars, "sh");
    assert_eq!(sh.value, "Circle(2.5)");

    let pts = var(&vars, "pts");
    let pts_children = values::children(&session, pts.reference);
    assert_eq!(pts_children.len(), 2);
    assert_eq!(pts_children[0].value, "Point(x=1, y=2)");
    let first_point_fields = values::children(&session, pts_children[0].reference);
    assert_eq!(var(&first_point_fields, "x").value, "1");
    assert_eq!(var(&first_point_fields, "y").value, "2");

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn f32_local_renders_correctly_despite_narrow_width() {
    let _guard = exec_lock();
    let src = "fn main():\n    let x: f32 = 1.5\n    print(x)\n";
    let path = temp_file(src);
    let session = launch(path.to_str().unwrap(), false).unwrap();

    let result = session.set_breakpoints(0, &[3]);
    assert_eq!(result, vec![(3, true)]);
    session.cont(1);
    session.events().recv().unwrap();

    let vars = values::frame_variables(&session, 0);
    assert_eq!(var(&vars, "x").value, "1.5");

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

#[test]
fn mutating_a_list_element_is_visible_after_a_step() {
    let _guard = exec_lock();
    let src = "fn main():\n    let mut xs = [1, 2, 3]\n    xs[0] = 99\n    print(xs)\n";
    let path = temp_file(src);
    let session = launch(path.to_str().unwrap(), false).unwrap();

    let result = session.set_breakpoints(0, &[3]); // `xs[0] = 99`, xs already assigned
    assert_eq!(result, vec![(3, true)]);
    session.cont(1);
    session.events().recv().unwrap();

    let xs = var(&values::frame_variables(&session, 0), "xs").reference;
    assert_eq!(
        values::children(&session, xs)
            .iter()
            .map(|v| v.value.clone())
            .collect::<Vec<_>>(),
        vec!["1", "2", "3"]
    );

    session.next(1);
    session.events().recv().unwrap();

    let xs = var(&values::frame_variables(&session, 0), "xs").reference;
    assert_eq!(
        values::children(&session, xs)
            .iter()
            .map(|v| v.value.clone())
            .collect::<Vec<_>>(),
        vec!["99", "2", "3"]
    );

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

/// `set_local_cell` on an `f32` cell round-trips through `debug_load`'s
/// special bitcast path (`translate.rs`) without corrupting the JIT'd
/// function or crashing it: the reload compiles and runs to a clean exit
/// with the patched value still legible through the debugger afterward.
/// Doesn't assert the patched value survives into further `f32` arithmetic
/// or a printed result -- `pit run` confirms (independent of any debug
/// session) that `f32` cast, comparison, and call-argument passing all have
/// pre-existing bugs of their own, so any assertion built on those would be
/// pinning compiler defects unrelated to `setVariable`, not this feature.
#[test]
fn set_local_cell_on_f32_reloads_without_corrupting_the_session() {
    let _guard = exec_lock();
    let src = "fn main():\n    let mut f: f32 = 1.0\n    print(1)\n";
    let path = temp_file(src);
    let session = launch(path.to_str().unwrap(), false).unwrap();

    let result = session.set_breakpoints(0, &[3]);
    assert_eq!(result, vec![(3, true)]);

    session.cont(1);
    match session.events().recv().unwrap() {
        DebugEvent::Stopped { .. } => {}
        other => panic!("expected Stopped, got {other:?}"),
    }

    let f_idx = session
        .fn_cells(session.frame_cells(0).unwrap().0)
        .iter()
        .position(|c| c.name == "f")
        .unwrap();
    let patched = 2.75f32.to_bits() as i64;
    assert!(session.set_local_cell(0, f_idx, patched));
    assert_eq!(
        var(&values::frame_variables(&session, 0), "f").value,
        "2.75"
    );

    run_to_exit(&session);
    std::fs::remove_file(&path).ok();
}

/// A second traced thread, running real instrumented olive code, gets its
/// own DAP identity end to end: registration, a breakpoint hit reporting
/// its own `threadId`, an independent parked stack, and clean resume --
/// exactly what `aio`'s executor/spawn_task/pool_run threads rely on
/// (`hooks::enable_debuggee_thread`, the same entry point `spawn_traced`
/// calls in `olive_std`). Drives `helper` directly via `raw_fn` on a
/// manually spawned thread rather than through a real `async fn`, so this
/// covers the session/threading machinery in isolation from `aio`'s own
/// scheduling behavior. `main` is deliberately never resumed (no `cont(1)`
/// is ever sent): it stays parked in `wait_for_start`, which keeps the
/// session alive for the whole test without a real race against it running
/// to completion and tearing the session down first.
#[test]
fn a_manually_spawned_worker_thread_hits_its_own_breakpoint_independently_of_main() {
    let _guard = exec_lock();
    let src = "fn helper(n: int) -> int:\n    let x = n + 1\n    print(x)\n    return x\nfn main():\n    print(1)\n";
    let path = temp_file(src);
    let mut session = launch(path.to_str().unwrap(), false).expect("launch failed");

    let result = session.set_breakpoints(0, &[2]); // `let x = n + 1` inside helper
    assert_eq!(result, vec![(2, true)]);

    let helper_ptr = session.raw_fn("helper").expect("helper compiled");
    let helper_fn: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(helper_ptr) };
    let worker = std::thread::Builder::new()
        .name("worker".to_string())
        .spawn(move || {
            hooks::enable_debuggee_thread("worker");
            let result = helper_fn(5);
            hooks::disable_debuggee_thread();
            result
        })
        .expect("spawn worker");

    let worker_id = loop {
        match session.events().recv().unwrap() {
            DebugEvent::ThreadStarted { .. } => continue,
            DebugEvent::Stopped {
                reason,
                frame,
                thread_id,
            } => {
                assert_eq!(reason, StopReason::Breakpoint);
                assert_eq!(frame.name, "helper");
                assert_eq!(frame.line, 2);
                assert_ne!(thread_id, 1, "helper runs on the worker thread, not main");
                break thread_id;
            }
            other => panic!("expected ThreadStarted or Stopped, got {other:?}"),
        }
    };

    let worker_stack = session.stack(worker_id);
    assert_eq!(worker_stack.len(), 1);
    assert_eq!(worker_stack[0].name, "helper");

    // Main never ran a single statement (no `cont(1)` was ever sent), so it
    // has no parked frames of its own -- proof that inspecting one thread's
    // stack never borrows or fabricates another's.
    assert!(session.stack(1).is_empty());

    let threads = session.threads_snapshot();
    assert!(
        threads
            .iter()
            .any(|&(id, ref name)| id == 1 && name == "main")
    );
    assert!(threads.iter().any(|&(id, _)| id == worker_id));

    session.cont(worker_id);
    assert_eq!(worker.join().unwrap(), 6);

    drop(session);
    std::fs::remove_file(&path).ok();
}
