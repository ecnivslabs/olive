//! Debug session state shared between the debuggee thread (parking on a
//! stop) and whichever thread drives the session (DAP server, headless
//! frontend, or a test). One instance per live session, held behind `Arc`
//! so both sides can reach it without the process-global statics in
//! `hooks` leaking any mutable state outside that file.
//!
//! `main.rs` doesn't call into this subsystem yet, so most of it is
//! unreachable from the bin target's `main`; `tests.rs` already exercises
//! it in full.
#![cfg_attr(not(test), allow(dead_code))]

use super::hooks::{self, Frame};
use crate::mir::debug_hooks::DebugProgramInfo;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Condvar, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Continue,
    #[allow(dead_code)] // stepping isn't wired up yet
    StepOver {
        depth: usize,
    },
    #[allow(dead_code)] // stepping isn't wired up yet
    StepIn,
    #[allow(dead_code)] // stepping isn't wired up yet
    StepOut {
        depth: usize,
    },
    #[allow(dead_code)] // nothing calls pause() yet
    Pause,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Entry,
    Breakpoint,
    #[allow(dead_code)] // stepping isn't wired up yet
    Step,
    Pause,
    #[allow(dead_code)] // fault stops aren't wired up yet
    Fault {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct FrameSnapshot {
    pub fn_id: u32,
    pub name: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub enum DebugEvent {
    Stopped {
        reason: StopReason,
        frame: FrameSnapshot,
    },
    Exited(i32),
}

struct Control {
    mode: RunMode,
    parked: bool,
    parked_frames: Vec<Frame>,
    run_token: u64,
}

pub struct EngineShared {
    control: Mutex<Control>,
    resume_cv: Condvar,
    entry_pending: AtomicBool,
    breakpoints_by_file: Mutex<FxHashMap<usize, FxHashSet<u32>>>,
    program: DebugProgramInfo,
    file_names: FxHashMap<usize, String>,
    events_tx: Mutex<Sender<DebugEvent>>,
}

fn pack(file_id: usize, line: u32) -> i64 {
    ((file_id as i64) << 32) | (line as i64)
}

impl EngineShared {
    pub(crate) fn new(
        program: DebugProgramInfo,
        file_names: FxHashMap<usize, String>,
        stop_on_entry: bool,
        events_tx: Sender<DebugEvent>,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            control: Mutex::new(Control {
                mode: RunMode::Continue,
                parked: false,
                parked_frames: Vec::new(),
                run_token: 0,
            }),
            resume_cv: Condvar::new(),
            entry_pending: AtomicBool::new(stop_on_entry),
            breakpoints_by_file: Mutex::new(FxHashMap::default()),
            program,
            file_names,
            events_tx: Mutex::new(events_tx),
        })
    }

    pub(crate) fn wants_stop_on_entry(&self) -> bool {
        self.entry_pending.load(Ordering::Relaxed)
    }

    /// Replaces every breakpoint for `file_id`. Returns, per requested line,
    /// the actual line used and whether it landed on an instrumented
    /// statement: exact match is verified as-is; otherwise the nearest
    /// following instrumented line in the same file is verified; if none
    /// follows, the original line is returned unverified and nothing is set.
    pub fn set_breakpoints(&self, file_id: usize, lines: &[u32]) -> Vec<(u32, bool)> {
        let mut resolved = FxHashSet::default();
        let mut out = Vec::with_capacity(lines.len());
        for &line in lines {
            match self.resolve_line(file_id, line) {
                Some(snapped) => {
                    resolved.insert(snapped);
                    out.push((snapped, true));
                }
                None => out.push((line, false)),
            }
        }
        self.breakpoints_by_file
            .lock()
            .unwrap()
            .insert(file_id, resolved);
        self.rebuild_breakpoint_index();
        out
    }

    fn resolve_line(&self, file_id: usize, line: u32) -> Option<u32> {
        if self.program.lines.contains(&pack(file_id, line)) {
            return Some(line);
        }
        self.program
            .lines
            .iter()
            .filter_map(|&packed| {
                let f = (packed >> 32) as usize;
                let l = (packed & 0xFFFF_FFFF) as u32;
                (f == file_id && l > line).then_some(l)
            })
            .min()
    }

    fn rebuild_breakpoint_index(&self) {
        let by_file = self.breakpoints_by_file.lock().unwrap();
        let mut keys = FxHashSet::default();
        for (&file_id, lines) in by_file.iter() {
            for &line in lines {
                keys.insert(pack(file_id, line));
            }
        }
        drop(by_file);
        hooks::replace_breakpoints(keys);
    }

    pub fn cont(&self) {
        let mut ctl = self.control.lock().unwrap();
        ctl.mode = RunMode::Continue;
        ctl.run_token = ctl.run_token.wrapping_add(1);
        drop(ctl);
        // A pending stop-on-entry must still force the next `debug_stmt`
        // slow path even though we're leaving Pause/the initial barrier;
        // clearing this unconditionally would let entry_pending go unread
        // and the program would run straight past it.
        hooks::set_force_check(self.entry_pending.load(Ordering::Relaxed));
        self.resume_cv.notify_all();
    }

    /// Blocks the debuggee thread until the first `cont()`, so breakpoints
    /// and stop-on-entry set up between `launch()` returning and the
    /// caller's first `cont()` are guaranteed to be in place before any
    /// user code runs -- without this, a short program can race past a
    /// breakpoint the caller hasn't finished installing yet.
    pub(crate) fn wait_for_start(&self) {
        let mut ctl = self.control.lock().unwrap();
        while ctl.run_token == 0 {
            ctl = self.resume_cv.wait(ctl).unwrap();
        }
    }

    #[allow(dead_code)] // nothing calls pause() yet
    pub fn pause(&self) {
        let mut ctl = self.control.lock().unwrap();
        ctl.mode = RunMode::Pause;
        drop(ctl);
        hooks::set_force_check(true);
    }

    /// Valid only while stopped; returns the empty stack otherwise. Index 0
    /// is the innermost (currently executing) frame.
    pub fn stack(&self) -> Vec<FrameSnapshot> {
        let ctl = self.control.lock().unwrap();
        if !ctl.parked {
            return Vec::new();
        }
        ctl.parked_frames
            .iter()
            .rev()
            .map(|f| self.snapshot(f))
            .collect()
    }

    fn snapshot(&self, frame: &Frame) -> FrameSnapshot {
        let name = self
            .program
            .functions
            .iter()
            .find(|f| f.fn_id == frame.fn_id)
            .map(|f| f.name.clone())
            .unwrap_or_default();
        let file_id = (frame.line >> 32) as usize;
        let line = (frame.line & 0xFFFF_FFFF) as u32;
        let file = self.file_names.get(&file_id).cloned().unwrap_or_default();
        FrameSnapshot {
            fn_id: frame.fn_id,
            name,
            file,
            line,
        }
    }

    pub(crate) fn cell_count(&self, fn_id: u32) -> Option<usize> {
        self.program
            .functions
            .iter()
            .find(|f| f.fn_id == fn_id)
            .map(|f| f.cells.len())
    }

    pub(crate) fn stop_reason_for(&self, packed: i64) -> Option<StopReason> {
        if self.entry_pending.swap(false, Ordering::AcqRel) {
            return Some(StopReason::Entry);
        }
        let is_pause = matches!(self.control.lock().unwrap().mode, RunMode::Pause);
        if is_pause {
            return Some(StopReason::Pause);
        }
        if hooks::is_breakpoint(packed) {
            return Some(StopReason::Breakpoint);
        }
        None
    }

    /// Called from the debuggee thread on a stop condition. Records the
    /// frame snapshot, emits `Stopped`, then blocks until `cont()`/`pause()`
    /// bumps the run token.
    pub(crate) fn park(&self, reason: StopReason, frames: Vec<Frame>) {
        let top = frames
            .last()
            .map(|f| self.snapshot(f))
            .unwrap_or(FrameSnapshot {
                fn_id: 0,
                name: String::new(),
                file: String::new(),
                line: 0,
            });

        let mut ctl = self.control.lock().unwrap();
        let token = ctl.run_token;
        ctl.parked = true;
        ctl.parked_frames = frames;
        drop(ctl);

        let _ = self
            .events_tx
            .lock()
            .unwrap()
            .send(DebugEvent::Stopped { reason, frame: top });

        let mut ctl = self.control.lock().unwrap();
        while ctl.run_token == token {
            ctl = self.resume_cv.wait(ctl).unwrap();
        }
        ctl.parked = false;
    }

    pub(crate) fn send_exited(&self, code: i32) {
        let _ = self
            .events_tx
            .lock()
            .unwrap()
            .send(DebugEvent::Exited(code));
    }
}
