//! Debug hook fast path. Every instrumented program calls these on every
//! statement/assign/enter/return; a program not under a debugger still
//! contains the calls (JIT-only, see `mir::debug_hooks`), so the check
//! against `DEBUGGEE_ENABLED` must be a single load-and-branch. These are
//! registered directly into the JIT via `builder.symbol`, never through
//! `SYMBOL_MAP` or olive_std, so a plain run never links against them.
//!
//! Process-global state lives only in this file; `engine`/`launch` reach it
//! through the `pub(crate)` functions below, never through a bare static.
//!
//! `main.rs` doesn't call into this subsystem yet, so most of it is
//! unreachable from the bin target's `main`; `tests.rs` already exercises
//! it in full.
#![cfg_attr(not(test), allow(dead_code))]

use super::engine::{EngineShared, StopReason};
use rustc_hash::FxHashSet;
use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

thread_local! {
    /// Set only on the spawned debuggee thread for the lifetime of a debug
    /// session. Runtime worker threads (thread pools, async executors) never
    /// set it, so hooks on those threads are a load, compare, return.
    static DEBUGGEE_ENABLED: Cell<bool> = const { Cell::new(false) };
    /// Per-thread call stack. Only the debuggee thread ever pushes to this;
    /// a snapshot is cloned into the parked engine state on a stop.
    static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
}

/// Raw captured frame: `line` is the packed `(file_id << 32) | line` value
/// from the most recent `debug_stmt` call in this frame, `cells` holds raw
/// i64 bits indexed by cell index, decoded later via the cell's MIR `Type`.
#[derive(Clone)]
pub(crate) struct Frame {
    pub(crate) fn_id: u32,
    pub(crate) line: i64,
    pub(crate) cells: Vec<i64>,
}

/// Nonzero whenever any breakpoint is set anywhere in the program; checked
/// before `FORCE_CHECK` so the overwhelmingly common "no breakpoints, not
/// stepping" case only pays for one relaxed load.
static BP_COUNT: AtomicUsize = AtomicUsize::new(0);
/// True whenever a stop condition doesn't depend on the breakpoint set
/// (pending stop-on-entry, an active pause request).
static FORCE_CHECK: AtomicBool = AtomicBool::new(false);

fn session_slot() -> &'static RwLock<Option<Arc<EngineShared>>> {
    static SESSION: OnceLock<RwLock<Option<Arc<EngineShared>>>> = OnceLock::new();
    SESSION.get_or_init(|| RwLock::new(None))
}

fn breakpoint_set() -> &'static RwLock<FxHashSet<i64>> {
    static SET: OnceLock<RwLock<FxHashSet<i64>>> = OnceLock::new();
    SET.get_or_init(|| RwLock::new(FxHashSet::default()))
}

/// Installs the process's one live session. Errors if a session is already
/// active; resets breakpoint/force-check state so a prior session's leftover
/// packed keys (file ids are only unique within a single compile) never leak
/// into the new one.
pub(crate) fn install_session(shared: Arc<EngineShared>) -> Result<(), ()> {
    let mut slot = session_slot().write().unwrap();
    if slot.is_some() {
        return Err(());
    }
    replace_breakpoints(FxHashSet::default());
    set_force_check(shared.wants_stop_on_entry());
    *slot = Some(shared);
    Ok(())
}

pub(crate) fn clear_session() {
    *session_slot().write().unwrap() = None;
    set_force_check(false);
}

pub(crate) fn is_breakpoint(packed: i64) -> bool {
    breakpoint_set().read().unwrap().contains(&packed)
}

pub(crate) fn replace_breakpoints(keys: FxHashSet<i64>) {
    BP_COUNT.store(keys.len(), Ordering::Relaxed);
    *breakpoint_set().write().unwrap() = keys;
}

pub(crate) fn set_force_check(v: bool) {
    FORCE_CHECK.store(v, Ordering::Relaxed);
}

pub(crate) fn enable_debuggee() {
    DEBUGGEE_ENABLED.set(true);
}

pub(crate) fn disable_debuggee() {
    DEBUGGEE_ENABLED.set(false);
}

pub extern "C" fn debug_stmt(packed: i64) {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
    if BP_COUNT.load(Ordering::Relaxed) == 0 && !FORCE_CHECK.load(Ordering::Relaxed) {
        return;
    }
    stmt_slow_path(packed);
}

fn stmt_slow_path(packed: i64) {
    let depth = FRAMES.with_borrow_mut(|frames| {
        if let Some(top) = frames.last_mut() {
            top.line = packed;
        }
        frames.len()
    });

    let Some(shared) = session_slot().read().unwrap().clone() else {
        return;
    };
    let Some(reason) = shared.stop_reason_for(packed, depth) else {
        return;
    };
    let snapshot = FRAMES.with_borrow(|frames| frames.clone());
    shared.park(reason, snapshot);
}

pub extern "C" fn debug_enter(fn_id: i64) {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
    let cell_count = session_slot()
        .read()
        .unwrap()
        .as_ref()
        .and_then(|s| s.cell_count(fn_id as u32))
        .unwrap_or(0);
    FRAMES.with_borrow_mut(|frames| {
        frames.push(Frame {
            fn_id: fn_id as u32,
            line: 0,
            cells: vec![0; cell_count],
        });
    });
}

pub extern "C" fn debug_store(cell_idx: i64, value: i64) {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
    FRAMES.with_borrow_mut(|frames| {
        if let Some(top) = frames.last_mut()
            && let Some(slot) = top.cells.get_mut(cell_idx as usize)
        {
            *slot = value;
        }
    });
}

pub extern "C" fn debug_exit() {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
    FRAMES.with_borrow_mut(|frames| {
        frames.pop();
    });
}

/// Installed once at launch via `olive_debug_set_fault_hook`, called from
/// inside `abort_with` on whatever thread panicked (the debuggee thread in
/// this single-session model). Parks like a breakpoint hit, using the same
/// `FRAMES` snapshot `debug_stmt` would: nothing was popped, since a fault
/// aborts mid-statement rather than returning cleanly.
pub extern "C" fn debug_fault_hook(code_ptr: i64, msg_ptr: i64, _loc_ptr: i64) {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
    let code = unsafe { CStr::from_ptr(code_ptr as *const c_char) }
        .to_string_lossy()
        .into_owned();
    let message = unsafe { CStr::from_ptr(msg_ptr as *const c_char) }
        .to_string_lossy()
        .into_owned();
    let Some(shared) = session_slot().read().unwrap().clone() else {
        return;
    };
    let snapshot = FRAMES.with_borrow(|frames| frames.clone());
    shared.park(StopReason::Fault { code, message }, snapshot);
}

/// Symbols registered unconditionally into every JIT module. Nothing calls
/// through them unless a debug session instrumented the MIR first, so a
/// registered-but-unused symbol costs nothing at runtime.
pub fn jit_symbols() -> [(&'static str, *const u8); 4] {
    [
        ("__olive_debug_stmt", debug_stmt as *const u8),
        ("__olive_debug_enter", debug_enter as *const u8),
        ("__olive_debug_store", debug_store as *const u8),
        ("__olive_debug_exit", debug_exit as *const u8),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn instrumented_program_runs_to_exit_code_zero() {
        let (mut jit, program) = crate::test_utils::compile_instrumented(
            "fn add(a: int, b: int) -> int:\n    return a + b\nfn main():\n    print(add(1, 2))\n",
        );
        assert!(program.functions.iter().any(|f| f.name == "add"));
        assert!(program.functions.iter().any(|f| f.name == "main"));

        let ptr = jit.get_function("__main__").expect("__main__ not found");
        let main_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(ptr) };
        let _guard = crate::test_utils::exec_lock();
        assert_eq!(main_fn(), 0);
    }
}
