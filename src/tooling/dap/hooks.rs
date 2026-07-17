//! Debug hook fast path. Every instrumented program calls these on every
//! statement/assign/enter/return; a program not under a debugger still
//! contains the calls (JIT-only, see `mir::debug_hooks`), so the check
//! against `DEBUGGEE_ENABLED` must be a single load-and-branch. These are
//! registered directly into the JIT via `builder.symbol`, never through
//! `SYMBOL_MAP` or olive_std, so a plain run never links against them.
//!
//! Process-global state lives only in this file; `engine`/`launch` reach it
//! through the `pub(crate)` functions below, never through a bare static.

use super::engine::{EngineShared, StopReason};
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

thread_local! {
    /// Set only on the spawned debuggee thread for the lifetime of a debug
    /// session. Runtime worker threads (thread pools, async executors) never
    /// set it, so hooks on those threads are a load, compare, return.
    static DEBUGGEE_ENABLED: Cell<bool> = const { Cell::new(false) };
    /// Per-thread call stack. Only the debuggee thread ever pushes to this;
    /// a snapshot is cloned into the parked engine state on a stop.
    static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
    /// Cache of `(fn_id → cell_count)` populated on first visit per
    /// fn_id. Avoids the session slot lookup in `debug_enter` for loops
    /// where the same fn_id repeats.
    static CELL_COUNT_CACHE: RefCell<FxHashMap<u32, usize>> =
        RefCell::new(FxHashMap::default());
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

// Combined session + breakpoint snapshot. Replaces two separate
// RwLocks (session_slot + breakpoint_set) with a single snapshot so
// stmt_slow_path acquires one lock instead of two.
struct SessionSnapshot {
    shared: Option<Arc<EngineShared>>,
    breakpoints: FxHashSet<i64>,
}

/// Nonzero whenever any breakpoint is set anywhere in the program; checked
/// before `FORCE_CHECK` so the overwhelmingly common "no breakpoints, not
/// stepping" case only pays for one relaxed load.
static BP_COUNT: AtomicUsize = AtomicUsize::new(0);
/// True whenever a stop condition doesn't depend on the breakpoint set
/// (pending stop-on-entry, an active pause request).
static FORCE_CHECK: AtomicBool = AtomicBool::new(false);
/// Whether a runtime fault parks the debuggee before the process exits, per
/// the client's `faults` exception-breakpoint filter. Defaults to on so a
/// session that never touches `setExceptionBreakpoints` keeps today's
/// stop-on-fault behavior.
static STOP_ON_FAULT: AtomicBool = AtomicBool::new(true);
/// True when any data watchpoint is active. Always false until the watch
/// table lands; reserves the checkpoint so `debug_store`
/// skips its body when no watchers exist.
static WATCH_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Fast-path breakpoint set, synced with `SessionSnapshot::breakpoints`.
/// Read directly by `debug_stmt` to avoid the `session_state()` indirection
/// (OnceLock + RwLock<Option<...>> + as_ref) on every statement.
static FAST_BP_SET: OnceLock<RwLock<FxHashSet<i64>>> = OnceLock::new();
/// Single-breakpoint fast path: when exactly one breakpoint is set, stores
/// its packed line here so `should_check_stmt` can check it with one
/// relaxed atomic load instead of acquiring a RwLock. -1 = no breakpoints,
/// i64::MAX = multiple breakpoints (fall back to FAST_BP_SET).
static SINGLE_BP_LINE: AtomicI64 = AtomicI64::new(-1);

fn session_state() -> &'static RwLock<Option<SessionSnapshot>> {
    static STATE: OnceLock<RwLock<Option<SessionSnapshot>>> = OnceLock::new();
    STATE.get_or_init(|| RwLock::new(None))
}

/// Installs the process's one live session. Errors if a session is already
/// active; resets breakpoint/force-check state so a prior session's leftover
/// packed keys (file ids are only unique within a single compile) never leak
/// into the new one.
pub(crate) fn install_session(shared: Arc<EngineShared>) -> Result<(), ()> {
    let mut state = session_state().write().unwrap();
    if state.is_some() {
        return Err(());
    }
    set_force_check(shared.wants_stop_on_entry());
    set_stop_on_fault(true);
    WATCH_ACTIVE.store(false, Ordering::Relaxed);
    SINGLE_BP_LINE.store(-1, Ordering::Relaxed);
    *FAST_BP_SET
        .get_or_init(|| RwLock::new(FxHashSet::default()))
        .write()
        .unwrap() = FxHashSet::default();
    *state = Some(SessionSnapshot {
        breakpoints: FxHashSet::default(),
        shared: Some(shared),
    });
    BP_COUNT.store(0, Ordering::Relaxed);
    Ok(())
}

pub(crate) fn clear_session() {
    *session_state().write().unwrap() = None;
    set_force_check(false);
    WATCH_ACTIVE.store(false, Ordering::Relaxed);
    SINGLE_BP_LINE.store(-1, Ordering::Relaxed);
    if let Some(bp) = FAST_BP_SET.get() {
        *bp.write().unwrap() = FxHashSet::default();
    }
}

/// Returns a clone of the engine shared reference, if the session is active.
pub(crate) fn session_shared() -> Option<Arc<EngineShared>> {
    session_state()
        .read()
        .unwrap()
        .as_ref()
        .and_then(|s| s.shared.clone())
}

pub(crate) fn replace_breakpoints(keys: FxHashSet<i64>) {
    BP_COUNT.store(keys.len(), Ordering::Relaxed);
    let mut state = session_state().write().unwrap();
    if let Some(snap) = state.as_mut() {
        snap.breakpoints = keys.clone();
    }
    // Sync SINGLE_BP_LINE for the lock-free single-breakpoint fast path.
    match keys.len() {
        0 => SINGLE_BP_LINE.store(-1, Ordering::Relaxed),
        1 => {
            if let Some(&line) = keys.iter().next() {
                SINGLE_BP_LINE.store(line, Ordering::Relaxed);
            }
        }
        _ => SINGLE_BP_LINE.store(i64::MAX, Ordering::Relaxed),
    }
    *FAST_BP_SET
        .get_or_init(|| RwLock::new(FxHashSet::default()))
        .write()
        .unwrap() = keys;
}

fn fast_bp_contains(packed: i64) -> bool {
    FAST_BP_SET
        .get()
        .map(|set| set.read().unwrap().contains(&packed))
        .unwrap_or(false)
}

/// Read-only access to the breakpoint set. Used only by `stop_reason_for`.
pub(crate) fn contains_breakpoint(packed: i64) -> bool {
    session_state()
        .read()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.breakpoints.contains(&packed))
}

pub(crate) fn set_force_check(v: bool) {
    FORCE_CHECK.store(v, Ordering::Relaxed);
}

pub(crate) fn set_stop_on_fault(v: bool) {
    STOP_ON_FAULT.store(v, Ordering::Relaxed);
}

pub(crate) fn enable_debuggee() {
    DEBUGGEE_ENABLED.set(true);
}

pub(crate) fn disable_debuggee() {
    DEBUGGEE_ENABLED.set(false);
}

/// Inline stmt-check helper. Called from JIT'd code before every
/// `debug_stmt` call. Returns 0 when this specific statement can be
/// skipped (no breakpoint on this line and no stepping is active).
/// Combined zero-check + per-line breakpoint check in one function so
/// the MIR conditional ONLY dispatches to `debug_stmt` when there's
/// genuinely a reason to stop.
///
/// Uses three-tier fast path:
/// 1. No breakpoints + not stepping → return 0 (2 relaxed atomics, no lock)
/// 2. Singe breakpoint → compare `SINGLE_BP_LINE` directly (1 atomic load)
/// 3. Multi breakpoint → fall back to `FAST_BP_SET` RwLock
pub extern "C" fn debug_should_check_stmt(packed: i64) -> i64 {
    if FORCE_CHECK.load(Ordering::Relaxed) {
        return 1;
    }
    let count = BP_COUNT.load(Ordering::Relaxed);
    if count == 0 {
        return 0;
    }
    // Single-breakpoint fast path: compare against the cached line.
    if count == 1 {
        let single = SINGLE_BP_LINE.load(Ordering::Relaxed);
        // single == -1 is the clear-session sentinel; safe to compare.
        return if single == packed { 1 } else { 0 };
    }
    // Multiple breakpoints: fall back to the hash set.
    if fast_bp_contains(packed) {
        return 1;
    }
    0
}

/// Called from JIT code when the inline check above determines this
/// statement actually needs a stop. Only called for lines that are either
/// a breakpoint or being stepped through, so it goes directly to the
/// slow path without re-checking.
pub extern "C" fn debug_stmt(packed: i64) {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
    stmt_slow_path(packed);
}

fn stmt_slow_path(packed: i64) {
    // Update line without cloning the frame yet.
    FRAMES.with_borrow_mut(|frames| {
        if let Some(top) = frames.last_mut() {
            top.line = packed;
        }
    });
    let depth = FRAMES.with_borrow(|frames| frames.len());

    // Check breakpoint membership before cloning the frame.
    // Use the combined snapshot to check both session and bp set.
    let state = session_state().read().unwrap();
    let Some(snap) = state.as_ref() else {
        return;
    };
    let Some(shared) = snap.shared.clone() else {
        return;
    };
    // Delay frame clone until after we know there's a reason to stop.
    // We need the top frame for stop_reason_for's condition evaluation.
    let top: Option<Frame> = FRAMES.with_borrow(|frames| frames.last().cloned());
    let Some(top) = top else {
        return;
    };
    drop(state);

    let Some(reason) = shared.stop_reason_for(packed, depth, &top) else {
        return;
    };
    let snapshot = FRAMES.with_borrow(|frames| frames.clone());
    shared.park(reason, snapshot);
}

/// Cached cell count. Checks the TLS cache before falling back to
/// the session slot. Loops calling the same fn_id hit the cache after
/// the first entry.
pub extern "C" fn debug_enter(fn_id: i64) {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
    let fid = fn_id as u32;
    let cell_count = CELL_COUNT_CACHE.with_borrow_mut(|cache| {
        if let Some(&count) = cache.get(&fid) {
            return count;
        }
        let count = session_state()
            .read()
            .unwrap()
            .as_ref()
            .and_then(|s| s.shared.as_ref())
            .and_then(|s| s.cell_count(fid))
            .unwrap_or(0);
        cache.insert(fid, count);
        count
    });
    FRAMES.with_borrow_mut(|frames| {
        frames.push(Frame {
            fn_id: fid,
            line: 0,
            cells: vec![0; cell_count],
        });
    });
}

/// Fast path that reserves the WATCH_ACTIVE check point. The watch
/// table body is empty until data breakpoints land; the frame write always
/// runs since variable inspection needs current cell values.
pub extern "C" fn debug_store(cell_idx: i64, value: i64) {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
    if WATCH_ACTIVE.load(Ordering::Relaxed) {
        // Watch table lookup goes here once data breakpoints exist.
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
    if !DEBUGGEE_ENABLED.get() || !STOP_ON_FAULT.load(Ordering::Relaxed) {
        return;
    }
    let code = unsafe { CStr::from_ptr(code_ptr as *const c_char) }
        .to_string_lossy()
        .into_owned();
    let message = unsafe { CStr::from_ptr(msg_ptr as *const c_char) }
        .to_string_lossy()
        .into_owned();
    let Some(shared) = session_shared() else {
        return;
    };
    let snapshot = FRAMES.with_borrow(|frames| frames.clone());
    shared.park(StopReason::Fault { code, message }, snapshot);
}

/// Symbols registered unconditionally into every JIT module. Nothing calls
/// through them unless a debug session instrumented the MIR first, so a
/// registered-but-unused symbol costs nothing at runtime.
pub fn jit_symbols() -> [(&'static str, *const u8); 5] {
    [
        (
            "__olive_debug_should_check_stmt",
            debug_should_check_stmt as *const u8,
        ),
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
