//! Per-thread debuggee control state. Single-thread session runs entirely on
//! one `ThreadControl` (the main debuggee thread, always id 1); a session
//! whose program spawns aio workers gets one more per traced OS thread. Each
//! carries its own park/resume machinery and step state so stepping or
//! pausing one thread never touches another's -- only the breakpoint and
//! watch tables stay process-wide, since those fire the same regardless of
//! which thread hits them.
//!
//! Held behind `Arc` so both the owning thread's thread-local handle
//! (`hooks::MY_THREAD_CTL`, read on every statement) and `EngineShared`'s
//! registry (read by the server thread handling `threads`/`next`/`stepIn`/
//! etc.) reach the same state without a lock on the hot path.

use super::super::hooks::Frame;
use super::RunMode;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

pub(super) const MODE_CONTINUE: u8 = 0;
pub(super) const MODE_STEP_OVER: u8 = 1;
pub(super) const MODE_STEP_IN: u8 = 2;
pub(super) const MODE_STEP_OUT: u8 = 3;
pub(super) const MODE_PAUSE: u8 = 4;

/// Count of threads currently `force_check`-armed (stepping or paused).
/// `hooks::debug_should_check_stmt` gates its thread-local `MY_THREAD_CTL`
/// lookup behind this: zero (the overwhelmingly common case) means every
/// thread's statement hook skips straight past the per-thread check with
/// one flat relaxed load, the same cost the old process-global `FORCE_CHECK`
/// had before per-thread stepping existed. Nonzero only while some thread
/// somewhere is genuinely stepping/paused, at which point the hot path
/// falls through to the real per-thread check to see if it's *this* thread.
static FORCE_CHECK_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn any_force_check() -> bool {
    FORCE_CHECK_COUNT.load(Ordering::Relaxed) != 0
}

/// An in-flight step captured off one thread when an `async fn` frame
/// suspends, re-armed on whichever thread resumes that frame. Opaque outside
/// this module -- `hooks` only stores and hands it back, never reads the mode.
#[derive(Clone, Copy)]
pub(crate) struct StepStash {
    mode: u8,
    depth: usize,
    line: i64,
}

struct ParkState {
    parked: bool,
    parked_frames: Vec<Frame>,
    run_token: u64,
    /// Set by `detach()`, so a `park()` call that's already past its stop
    /// decision, racing the teardown's own resume signal, never blocks:
    /// the check happens under the same lock `detach()` writes under, so
    /// one side or the other always wins cleanly.
    detached: bool,
    /// Queued `(cell_idx, raw)` writes from `set_local_cell`, drained by
    /// this thread into its own TLS mirror right after `park()` returns
    /// (`hooks::apply_pending_patches`). Lives here, not on `EngineShared`,
    /// because two threads parked at once must never see each other's
    /// patches applied to the wrong frame stack.
    pending_patches: Vec<(usize, i64)>,
    /// `(fn_id, packed line, cells)` of the synthesized async-caller frames
    /// the last `stack()` walked up the executor's await graph, so a
    /// follow-up `variables` request naming one of those frame ids resolves
    /// its cells here rather than from the live shadow stack (which never
    /// held them). Rebuilt on every `stack()`; empty when the stop wasn't in
    /// an `async fn` with suspended callers.
    async_frames: Vec<(u32, i64, Vec<i64>)>,
}

pub(crate) struct ThreadControl {
    pub(crate) id: i64,
    pub(crate) name: String,
    park: Mutex<ParkState>,
    resume_cv: Condvar,
    mode_variant: AtomicU8,
    step_depth: AtomicUsize,
    step_line: AtomicI64,
    /// Per-thread analogue of the old process-global `FORCE_CHECK`: true
    /// while this specific thread is stepping or paused, so a step/pause
    /// targeting one thread never forces every other thread's statement
    /// hook onto the slow path.
    force_check: AtomicBool,
}

impl ThreadControl {
    pub(super) fn new(id: i64, name: String) -> Self {
        ThreadControl {
            id,
            name,
            park: Mutex::new(ParkState {
                parked: false,
                parked_frames: Vec::new(),
                run_token: 0,
                detached: false,
                pending_patches: Vec::new(),
                async_frames: Vec::new(),
            }),
            resume_cv: Condvar::new(),
            mode_variant: AtomicU8::new(MODE_CONTINUE),
            step_depth: AtomicUsize::new(0),
            step_line: AtomicI64::new(0),
            force_check: AtomicBool::new(false),
        }
    }

    pub(crate) fn force_check(&self) -> bool {
        self.force_check.load(Ordering::Relaxed)
    }

    /// Stores `v` into this thread's own flag and adjusts the process-wide
    /// `FORCE_CHECK_COUNT` on an actual transition, so the counter always
    /// equals the number of threads currently armed regardless of which of
    /// `resume`/`set_force_check`/`detach` did the arming/disarming.
    fn store_force_check(&self, v: bool) {
        let old = self.force_check.swap(v, Ordering::Relaxed);
        if old != v {
            if v {
                FORCE_CHECK_COUNT.fetch_add(1, Ordering::Relaxed);
            } else {
                FORCE_CHECK_COUNT.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// Sets `force_check` without touching the run token -- used once, at
    /// registration, to arm a stop-on-entry session's main thread before it
    /// starts running (a real `resume()` would bump the token and let
    /// `wait_for_start`'s barrier fall through prematurely).
    pub(crate) fn set_force_check(&self, v: bool) {
        self.store_force_check(v);
    }

    pub(super) fn mode_variant(&self) -> u8 {
        self.mode_variant.load(Ordering::Relaxed)
    }

    pub(super) fn step_depth(&self) -> usize {
        self.step_depth.load(Ordering::Relaxed)
    }

    pub(super) fn step_line(&self) -> i64 {
        self.step_line.load(Ordering::Relaxed)
    }

    fn set_mode_atomic(&self, mode: RunMode) {
        match mode {
            RunMode::Continue => self.mode_variant.store(MODE_CONTINUE, Ordering::Relaxed),
            RunMode::StepOver { depth, line } => {
                self.mode_variant.store(MODE_STEP_OVER, Ordering::Relaxed);
                self.step_depth.store(depth, Ordering::Relaxed);
                self.step_line.store(line, Ordering::Relaxed);
            }
            RunMode::StepIn { depth, line } => {
                self.mode_variant.store(MODE_STEP_IN, Ordering::Relaxed);
                self.step_depth.store(depth, Ordering::Relaxed);
                self.step_line.store(line, Ordering::Relaxed);
            }
            RunMode::StepOut { depth } => {
                self.mode_variant.store(MODE_STEP_OUT, Ordering::Relaxed);
                self.step_depth.store(depth, Ordering::Relaxed);
            }
            RunMode::Pause => self.mode_variant.store(MODE_PAUSE, Ordering::Relaxed),
        }
    }

    /// Sets the run mode, bumps the token so a parked wait loop wakes, and
    /// arms this thread's `force_check` so its stmt hook takes the slow path
    /// even with zero breakpoints set.
    pub(crate) fn resume(&self, mode: RunMode, force_check: bool) {
        self.set_mode_atomic(mode);
        let mut ps = self.park.lock().unwrap();
        ps.run_token = ps.run_token.wrapping_add(1);
        drop(ps);
        self.store_force_check(force_check);
        self.resume_cv.notify_all();
    }

    /// Guarantees this thread never parks again, waking it immediately if
    /// it's currently blocked. Not `cont()`: a plain resume only wakes a
    /// thread already parked, and one that hits a fresh stop moments later
    /// would park again with nobody left to send the next resume.
    pub(crate) fn detach(&self) {
        self.mode_variant.store(MODE_CONTINUE, Ordering::Relaxed);
        self.step_depth.store(0, Ordering::Relaxed);
        self.step_line.store(0, Ordering::Relaxed);
        self.store_force_check(false);
        let mut ps = self.park.lock().unwrap();
        ps.detached = true;
        ps.run_token = ps.run_token.wrapping_add(1);
        drop(ps);
        self.resume_cv.notify_all();
    }

    /// Captures and clears this thread's active step, if it's mid-step, so a
    /// suspending `async fn` frame can carry the step with it and re-arm it
    /// on whichever thread next resumes that frame (`hooks::debug_sm_suspend`).
    /// Returns `None` when the thread isn't stepping -- a plain continue,
    /// pause, or breakpoint hit has nothing to carry across the suspension.
    /// Clearing the thread's own step here is what stops the step from
    /// spuriously firing in an unrelated task the same worker picks up next.
    pub(crate) fn take_step_stash(&self) -> Option<StepStash> {
        let mode = self.mode_variant.load(Ordering::Relaxed);
        if !matches!(mode, MODE_STEP_OVER | MODE_STEP_IN | MODE_STEP_OUT) {
            return None;
        }
        let stash = StepStash {
            mode,
            depth: self.step_depth.load(Ordering::Relaxed),
            line: self.step_line.load(Ordering::Relaxed),
        };
        self.mode_variant.store(MODE_CONTINUE, Ordering::Relaxed);
        self.store_force_check(false);
        Some(stash)
    }

    /// Re-arms a step carried across a suspension (`take_step_stash`) onto
    /// this thread, the counterpart `hooks::debug_sm_resume` calls when the
    /// frame it belongs to is polled again. `depth` was captured at the
    /// `async fn` frame's own level, which is exactly the level it re-enters
    /// at (the executor always polls a task with an otherwise-empty shadow
    /// stack), so it needs no adjustment.
    pub(crate) fn restore_step_stash(&self, stash: StepStash) {
        self.mode_variant.store(stash.mode, Ordering::Relaxed);
        self.step_depth.store(stash.depth, Ordering::Relaxed);
        self.step_line.store(stash.line, Ordering::Relaxed);
        self.store_force_check(true);
    }

    /// The depth/line of the frame this thread is currently parked in,
    /// source for every step mode's starting point.
    pub(crate) fn stopped_depth_line(&self) -> (usize, i64) {
        let ps = self.park.lock().unwrap();
        let depth = ps.parked_frames.len();
        let line = ps.parked_frames.last().map(|f| f.line).unwrap_or(0);
        (depth, line)
    }

    /// Valid only while parked; the empty stack otherwise. Index 0 is the
    /// innermost (currently executing) frame.
    pub(crate) fn stack_frames(&self) -> Vec<Frame> {
        let ps = self.park.lock().unwrap();
        if !ps.parked {
            return Vec::new();
        }
        ps.parked_frames.iter().rev().cloned().collect()
    }

    /// `local_idx` 0 is the innermost frame, matching `stack_frames`'s order.
    pub(crate) fn frame_at(&self, local_idx: usize) -> Option<(u32, Vec<i64>)> {
        let ps = self.park.lock().unwrap();
        if !ps.parked {
            return None;
        }
        let n = ps.parked_frames.len();
        let f = ps
            .parked_frames
            .get(n.checked_sub(1)?.checked_sub(local_idx)?)?;
        Some((f.fn_id, f.cells.clone()))
    }

    /// `setVariable`/`setExpression` write path for `local_idx == 0` (the
    /// frame this thread is actually parked inside). Updates the
    /// inspector-visible snapshot immediately, then queues the mirror write
    /// this thread applies to its own TLS frame stack on resume.
    pub(crate) fn set_local_cell(&self, cell_idx: usize, raw: i64) -> bool {
        let mut ps = self.park.lock().unwrap();
        if !ps.parked {
            return false;
        }
        let Some(slot) = ps
            .parked_frames
            .last_mut()
            .and_then(|f| f.cells.get_mut(cell_idx))
        else {
            return false;
        };
        *slot = raw;
        ps.pending_patches.push((cell_idx, raw));
        true
    }

    pub(crate) fn take_pending_patches(&self) -> Vec<(usize, i64)> {
        std::mem::take(&mut self.park.lock().unwrap().pending_patches)
    }

    /// Records the async-caller frames the current `stack()` reconstructed, so
    /// a later `variables` request on one of them (`async_frame_at`) can read
    /// its cells back.
    pub(crate) fn set_async_frames(&self, frames: Vec<(u32, i64, Vec<i64>)>) {
        self.park.lock().unwrap().async_frames = frames;
    }

    /// `(fn_id, cells)` of the `idx`-th async-caller frame from the last
    /// `stack()`. `None` if `idx` is past what that walk produced.
    pub(crate) fn async_frame_at(&self, idx: usize) -> Option<(u32, Vec<i64>)> {
        let ps = self.park.lock().unwrap();
        ps.async_frames
            .get(idx)
            .map(|(fn_id, _, cells)| (*fn_id, cells.clone()))
    }

    /// Begins a park: records the frame snapshot and returns the run token
    /// to wait on, or `None` if the session already detached (a stop
    /// decision racing teardown loses cleanly instead of blocking forever).
    pub(crate) fn begin_park(&self, frames: Vec<Frame>) -> Option<u64> {
        let mut ps = self.park.lock().unwrap();
        if ps.detached {
            return None;
        }
        let token = ps.run_token;
        ps.parked = true;
        ps.parked_frames = frames;
        Some(token)
    }

    /// Blocks until `resume()`/`detach()` bumps the run token past `token`.
    pub(crate) fn wait_until_resumed(&self, token: u64) {
        let mut ps = self.park.lock().unwrap();
        while ps.run_token == token {
            ps = self.resume_cv.wait(ps).unwrap();
        }
        ps.parked = false;
    }
}
