//! Debug hook fast path. Every instrumented program calls these on every
//! statement/assign/enter/return; a program not under a debugger still
//! contains the calls (JIT-only, see `mir::debug_hooks`), so the check
//! against `DEBUGGEE_ENABLED` must be a single load-and-branch. These are
//! registered directly into the JIT via `builder.symbol`, never through
//! `SYMBOL_MAP` or olive_std, so a plain run never links against them.
//!
//! Process-global state lives only in this file; `engine`/`launch` reach it
//! through the `pub(crate)` functions below, never through a bare static.

use super::engine::{EngineShared, StopReason, ThreadControl, any_force_check};
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

thread_local! {
    /// Set only on a traced thread (the main debuggee thread, or an aio
    /// worker that's called `enable_debuggee_thread`) for the lifetime of a
    /// debug session. Untraced runtime threads never set it, so hooks on
    /// those threads are a load, compare, return.
    static DEBUGGEE_ENABLED: Cell<bool> = const { Cell::new(false) };
    /// This thread's own `ThreadControl`, set once by
    /// `enable_debuggee_thread`/`attach_debuggee_thread` and cleared by
    /// `disable_debuggee_thread`. Reached on every statement hook, so it's a
    /// direct handle rather than a lookup into `EngineShared::threads` --
    /// the whole reason a session keeps that registry keyed by id at all is
    /// so the *other* side (the server thread) can reach a specific thread
    /// by the id it announced, not so this thread has to search for itself.
    static MY_THREAD_CTL: RefCell<Option<Arc<ThreadControl>>> = const { RefCell::new(None) };
    /// Per-thread call stack. Only this thread ever pushes to it; a
    /// snapshot is cloned into the parked engine state on a stop.
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
/// Whether a runtime fault parks the debuggee before the process exits, per
/// the client's `faults` exception-breakpoint filter. Defaults to on so a
/// session that never touches `setExceptionBreakpoints` keeps today's
/// stop-on-fault behavior.
static STOP_ON_FAULT: AtomicBool = AtomicBool::new(true);
/// True when any data breakpoint is active, so `debug_store` skips the
/// tiered watch-key check entirely on the overwhelmingly common case of no
/// watchers. Deliberately NOT consulted by `debug_should_check_stmt` --
/// see that function's doc comment for why forcing it there was reverted.
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
/// Same three-tier shape as `BP_COUNT`/`SINGLE_BP_LINE`/`FAST_BP_SET`,
/// keyed by `pack_watch(fn_id, cell_idx)` instead of a packed line. Without
/// this, every `debug_store` in the program would pay a `session_shared()`
/// lock + `Arc` clone + frame clone + watch-table mutex on every write,
/// whether or not it's the cell actually being watched -- measured 1.6x
/// slower on a hot loop with an unrelated watchpoint set elsewhere.
static WATCH_COUNT: AtomicUsize = AtomicUsize::new(0);
static SINGLE_WATCH_KEY: AtomicI64 = AtomicI64::new(-1);
static FAST_WATCH_SET: OnceLock<RwLock<FxHashSet<i64>>> = OnceLock::new();

fn session_state() -> &'static RwLock<Option<SessionSnapshot>> {
    static STATE: OnceLock<RwLock<Option<SessionSnapshot>>> = OnceLock::new();
    STATE.get_or_init(|| RwLock::new(None))
}

/// Installs the process's one live session. Errors if a session is already
/// active; resets breakpoint state so a prior session's leftover packed keys
/// (file ids are only unique within a single compile) never leak into the
/// new one.
pub(crate) fn install_session(shared: Arc<EngineShared>) -> Result<(), ()> {
    let mut state = session_state().write().unwrap();
    if state.is_some() {
        return Err(());
    }
    set_stop_on_fault(true);
    WATCH_ACTIVE.store(false, Ordering::Relaxed);
    SINGLE_BP_LINE.store(-1, Ordering::Relaxed);
    SINGLE_WATCH_KEY.store(-1, Ordering::Relaxed);
    WATCH_COUNT.store(0, Ordering::Relaxed);
    *FAST_BP_SET
        .get_or_init(|| RwLock::new(FxHashSet::default()))
        .write()
        .unwrap() = FxHashSet::default();
    *FAST_WATCH_SET
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
    WATCH_ACTIVE.store(false, Ordering::Relaxed);
    SINGLE_BP_LINE.store(-1, Ordering::Relaxed);
    SINGLE_WATCH_KEY.store(-1, Ordering::Relaxed);
    WATCH_COUNT.store(0, Ordering::Relaxed);
    if let Some(bp) = FAST_BP_SET.get() {
        *bp.write().unwrap() = FxHashSet::default();
    }
    if let Some(w) = FAST_WATCH_SET.get() {
        *w.write().unwrap() = FxHashSet::default();
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

/// `cell_idx` is realistically always small (a function's own local count),
/// so it fits the low 32 bits the same way `pack`'s line number does.
pub(crate) fn pack_watch(fn_id: u32, cell_idx: usize) -> i64 {
    ((fn_id as i64) << 32) | (cell_idx as i64)
}

/// Same three-tier sync `replace_breakpoints` does for `FAST_BP_SET`,
/// applied to the watch set. Also drives `WATCH_ACTIVE`, so there's a
/// single call site (`EngineShared::set_data_breakpoints`) for the whole
/// active/count/fast-path state instead of a separate toggle.
pub(crate) fn replace_watchpoints(keys: FxHashSet<i64>) {
    WATCH_ACTIVE.store(!keys.is_empty(), Ordering::Relaxed);
    WATCH_COUNT.store(keys.len(), Ordering::Relaxed);
    match keys.len() {
        0 => SINGLE_WATCH_KEY.store(-1, Ordering::Relaxed),
        1 => {
            if let Some(&key) = keys.iter().next() {
                SINGLE_WATCH_KEY.store(key, Ordering::Relaxed);
            }
        }
        _ => SINGLE_WATCH_KEY.store(i64::MAX, Ordering::Relaxed),
    }
    *FAST_WATCH_SET
        .get_or_init(|| RwLock::new(FxHashSet::default()))
        .write()
        .unwrap() = keys;
}

fn fast_watch_contains(key: i64) -> bool {
    FAST_WATCH_SET
        .get()
        .map(|set| set.read().unwrap().contains(&key))
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

pub(crate) fn set_stop_on_fault(v: bool) {
    STOP_ON_FAULT.store(v, Ordering::Relaxed);
}

/// Registers a brand-new traced thread with the active session, attaches
/// this thread to it, and announces its arrival -- the self-registering path
/// an aio worker takes the first time it runs instrumented code, since
/// nothing outside it could reach a threadId it hasn't announced yet. A
/// no-op outside a debug session (`session_shared()` is `None`, so
/// `DEBUGGEE_ENABLED`/`MY_THREAD_CTL` stay unset and every hook on this
/// thread keeps taking its cheap early return).
pub(crate) fn enable_debuggee_thread(name: &str) {
    if let Some(shared) = session_shared() {
        let ctl = shared.register_thread(name);
        shared.announce_thread_started(ctl.id);
        attach_debuggee_thread(ctl);
    }
}

/// Attaches this thread to an already-registered `ThreadControl` -- the path
/// `launch.rs` takes for the main debuggee thread, which is registered by
/// the *spawning* side before the thread starts so a `cont()` racing this
/// thread's own startup always has a real thread to resume.
pub(crate) fn attach_debuggee_thread(ctl: Arc<ThreadControl>) {
    DEBUGGEE_ENABLED.set(true);
    MY_THREAD_CTL.with_borrow_mut(|slot| *slot = Some(ctl));
}

/// Counterpart to `enable_debuggee_thread`: deregisters this aio worker
/// thread, announces its exit, and clears its debuggee state. Safe to call
/// even after the session has already torn down (`session_shared()` returns
/// `None`): there's simply nothing left to deregister from or announce to.
pub(crate) fn disable_debuggee_thread() {
    if let Some(ctl) = MY_THREAD_CTL.with_borrow_mut(|slot| slot.take())
        && let Some(shared) = session_shared()
    {
        shared.deregister_thread(ctl.id);
        shared.announce_thread_exited(ctl.id);
    }
    DEBUGGEE_ENABLED.set(false);
}

/// Counterpart to `attach_debuggee_thread`: clears this thread's debuggee
/// state without deregistering or announcing anything -- the main debuggee
/// thread's own shutdown, where the whole session is tearing down a moment
/// later anyway, so neither a registry entry nor a `ThreadExited` event
/// outlives it long enough to matter to anyone.
pub(crate) fn detach_main_thread() {
    MY_THREAD_CTL.with_borrow_mut(|slot| *slot = None);
    DEBUGGEE_ENABLED.set(false);
}

/// Raw C-ABI thread-start hook, installed into `olive_std` at launch
/// (`olive_debug_set_thread_hooks`) so `aio`'s executor pool, spawned tasks,
/// and `pool_run`(`_sync`) -- all of which run real olive-compiled code on
/// their own OS thread -- become visible to the debugger the same way the
/// main thread always has been. `name_ptr` is a short-lived C string owned
/// by the caller (`olive_std::debug::spawn_traced`); copied before use.
pub extern "C" fn debug_thread_start(name_ptr: i64) {
    let name = if name_ptr == 0 {
        String::new()
    } else {
        unsafe { CStr::from_ptr(name_ptr as *const c_char) }
            .to_string_lossy()
            .into_owned()
    };
    enable_debuggee_thread(&name);
}

/// Raw C-ABI thread-exit hook, the `debug_thread_start` counterpart called
/// right before a traced aio thread's closure returns.
pub extern "C" fn debug_thread_end() {
    disable_debuggee_thread();
}

/// Inline stmt-check helper. Called from JIT'd code before every
/// `debug_stmt` call. Returns 0 when this specific statement can be
/// skipped (no breakpoint on this line and no stepping is active on this
/// thread). Combined zero-check + per-line breakpoint check in one function
/// so the MIR conditional ONLY dispatches to `debug_stmt` when there's
/// genuinely a reason to stop.
///
/// Uses three-tier fast path:
/// 1. No breakpoints + not stepping → return 0 (2 relaxed atomics, no lock,
///    no thread-local touch)
/// 2. Singe breakpoint → compare `SINGLE_BP_LINE` directly (1 atomic load)
/// 3. Multi breakpoint → fall back to `FAST_BP_SET` RwLock
///
/// The stepping/pause half of the check is this thread's own
/// `ThreadControl::force_check`, not a process-global flag: a step or pause
/// targeting one thread must not force every other thread's statement hook
/// onto the slow path too. That per-thread flag lives behind a thread-local
/// `RefCell`, so it's gated behind `any_force_check()` (a single flat atomic
/// counter of how many threads are currently armed) rather than read
/// unconditionally -- the overwhelmingly common case across a whole session
/// is zero threads stepping, and touching `MY_THREAD_CTL` on every one of a
/// program's statements to learn that measured ~25% slower on a
/// call-heavy benchmark (fibonacci) than the flat-atomic check this
/// replaces. Deliberately NOT forced by `WATCH_ACTIVE`:
/// `mir::debug_hooks` pairs every `debug_stmt` call with an unconditional
/// reload of *every* named local from the frame mirror (`make_stmt_conditional`'s
/// doc comment), built and tested only for the rare, deliberate frequency of
/// an actual breakpoint or step hit. Forcing it on every statement so a
/// data-breakpoint-only stop's line stays current was tried and reverted: it
/// reassigns a reassigned/moved local (a fresh `let` inside a loop, a
/// container append) from a stale mirror value outside the ownership
/// system's own writes, and reliably corrupts a real program (`matrix_mult`
/// benchmark, E0707 "stale reference" within a few hundred iterations). A
/// data-breakpoint stop's reported line is therefore only as current as the
/// last real `debug_stmt` hit in that function -- accurate when a line
/// breakpoint or step is active alongside the watch, otherwise it lags to
/// that point.
pub extern "C" fn debug_should_check_stmt(packed: i64) -> i64 {
    if bp_fast_match(packed) {
        return 1;
    }
    if any_force_check() {
        let force =
            MY_THREAD_CTL.with_borrow(|slot| slot.as_ref().is_some_and(|t| t.force_check()));
        if force {
            return 1;
        }
    }
    0
}

/// The same lock-free tiered breakpoint check `debug_should_check_stmt`
/// uses, factored out so `debug_stmt`'s own gate (below) can reuse it
/// without a second copy of the three-tier logic.
fn bp_fast_match(packed: i64) -> bool {
    match BP_COUNT.load(Ordering::Relaxed) {
        0 => false,
        1 => SINGLE_BP_LINE.load(Ordering::Relaxed) == packed,
        _ => fast_bp_contains(packed),
    }
}

/// Called from JIT code when the inline check above determines this
/// statement actually needs a stop. Only called for lines that are either
/// a breakpoint or being stepped through, so it goes directly to the full
/// check without re-checking.
pub extern "C" fn debug_stmt(packed: i64) {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
    FRAMES.with_borrow_mut(|frames| {
        if let Some(top) = frames.last_mut() {
            top.line = packed;
        }
    });
    stmt_full_check(packed);
}

fn stmt_full_check(packed: i64) {
    let Some(thread) = my_thread_ctl() else {
        return;
    };
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

    let Some(reason) = shared.stop_reason_for(&thread, packed, depth, &top) else {
        return;
    };
    let snapshot = FRAMES.with_borrow(|frames| frames.clone());
    shared.park_on(&thread, reason, snapshot);
    apply_pending_patches(&thread);
}

/// This thread's own `ThreadControl`, if it's currently a traced debuggee
/// thread. `None` covers both "no debug session" and "an untraced thread
/// somehow reached an instrumented hook" identically -- both are a no-op.
fn my_thread_ctl() -> Option<Arc<ThreadControl>> {
    MY_THREAD_CTL.with_borrow(|slot| slot.clone())
}

/// Applies writes queued by `EngineShared::set_local_cell` (a `setVariable`/
/// `setExpression` request handled while this frame was parked) into the
/// mirror the frame's own `debug_load` calls read from right after. Those
/// calls only ever target the innermost frame -- the one that was actually
/// blocked inside `park_on()` -- so the frame that just resumed here is
/// always the right one; no cell/frame identity check is needed.
fn apply_pending_patches(thread: &ThreadControl) {
    let patches = thread.take_pending_patches();
    if patches.is_empty() {
        return;
    }
    FRAMES.with_borrow_mut(|frames| {
        if let Some(top) = frames.last_mut() {
            for (cell_idx, raw) in patches {
                if let Some(slot) = top.cells.get_mut(cell_idx) {
                    *slot = raw;
                }
            }
        }
    });
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

/// The frame write always runs, since variable inspection needs current
/// cell values regardless of whether anything is watching this cell. The
/// tiered key check below only runs after the write lands, so a data
/// breakpoint's condition sees the new value, not the one being overwritten.
pub extern "C" fn debug_store(cell_idx: i64, value: i64) {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
    let fn_id = FRAMES.with_borrow_mut(|frames| {
        let top = frames.last_mut()?;
        if let Some(slot) = top.cells.get_mut(cell_idx as usize) {
            *slot = value;
        }
        Some(top.fn_id)
    });
    let Some(fn_id) = fn_id else {
        return;
    };
    if WATCH_ACTIVE.load(Ordering::Relaxed) && watch_matches(fn_id, cell_idx) {
        watch_slow_path(cell_idx as usize);
    }
}

/// Same three-tier shape as `debug_should_check_stmt`: a program with a
/// watchpoint set somewhere else entirely must not pay more than a couple
/// of relaxed loads on every other cell's write.
fn watch_matches(fn_id: u32, cell_idx: i64) -> bool {
    let key = pack_watch(fn_id, cell_idx as usize);
    match WATCH_COUNT.load(Ordering::Relaxed) {
        0 => false,
        1 => SINGLE_WATCH_KEY.load(Ordering::Relaxed) == key,
        _ => fast_watch_contains(key),
    }
}

/// Mirrors `stmt_slow_path`'s shape, keyed by `(fn_id, cell_idx)` instead of
/// a packed line -- a write can trigger a stop between statement
/// boundaries, which `debug_stmt` never sees. Only reached once
/// `watch_matches` has already confirmed this cell is actually watched, so
/// the `session_shared()`/frame-clone cost here is paid only on a real hit.
fn watch_slow_path(cell_idx: usize) {
    let Some(shared) = session_shared() else {
        return;
    };
    let Some(thread) = my_thread_ctl() else {
        return;
    };
    let top: Option<Frame> = FRAMES.with_borrow(|frames| frames.last().cloned());
    let Some(top) = top else {
        return;
    };
    let Some(reason) = shared.stop_reason_for_watch(top.fn_id, cell_idx, &top) else {
        return;
    };
    let snapshot = FRAMES.with_borrow(|frames| frames.clone());
    shared.park_on(&thread, reason, snapshot);
    apply_pending_patches(&thread);
}

pub extern "C" fn debug_exit() {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
    FRAMES.with_borrow_mut(|frames| {
        frames.pop();
    });
}

/// Reload hook emitted right after every `__olive_debug_stmt` call
/// (`mir::debug_hooks::make_stmt_conditional`): returns the mirror's current
/// value for `cell_idx`, which `apply_pending_patches` just updated if a
/// `setVariable`/`setExpression` request landed while this frame was
/// parked. `debug_store` already keeps the mirror equal to the real local at
/// every hook point, so reassigning the local from it here is a no-op
/// whenever nothing patched it -- correct unconditionally, not just in the
/// patched case.
pub extern "C" fn debug_load(cell_idx: i64) -> i64 {
    if !DEBUGGEE_ENABLED.get() {
        return 0;
    }
    FRAMES.with_borrow(|frames| {
        frames
            .last()
            .and_then(|f| f.cells.get(cell_idx as usize))
            .copied()
            .unwrap_or(0)
    })
}

/// Installed once at launch via `olive_debug_set_fault_hook`, called from
/// inside `abort_with` on whatever thread panicked -- the main debuggee
/// thread or a traced aio worker, either way `MY_THREAD_CTL` names it. Parks
/// like a breakpoint hit, using the same `FRAMES` snapshot `debug_stmt`
/// would: nothing was popped, since a fault aborts mid-statement rather than
/// returning cleanly.
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
    let Some(thread) = my_thread_ctl() else {
        return;
    };
    let snapshot = FRAMES.with_borrow(|frames| frames.clone());
    shared.park_on(&thread, StopReason::Fault { code, message }, snapshot);
    apply_pending_patches(&thread);
}

/// Symbols registered unconditionally into every JIT module. Nothing calls
/// through them unless a debug session instrumented the MIR first, so a
/// registered-but-unused symbol costs nothing at runtime.
pub fn jit_symbols() -> [(&'static str, *const u8); 6] {
    [
        (
            "__olive_debug_should_check_stmt",
            debug_should_check_stmt as *const u8,
        ),
        ("__olive_debug_stmt", debug_stmt as *const u8),
        ("__olive_debug_enter", debug_enter as *const u8),
        ("__olive_debug_store", debug_store as *const u8),
        ("__olive_debug_exit", debug_exit as *const u8),
        ("__olive_debug_load", debug_load as *const u8),
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
