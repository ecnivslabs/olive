//! Debug session state shared between the debuggee thread (parking on a
//! stop) and whichever thread drives the session (DAP server, headless
//! frontend, or a test). One instance per live session, held behind `Arc`
//! so both sides can reach it without the process-global statics in
//! `hooks` leaking any mutable state outside that file.

use super::hooks::{self, Frame};
use super::values::VarStore;
use crate::mir::debug_hooks::DebugProgramInfo;
use crate::semantic::type_descriptor::type_descriptor;
use crate::semantic::types::Type;
use rustc_hash::{FxHashMap, FxHashSet};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Condvar, Mutex};

type StructFields = FxHashMap<String, Vec<String>>;
type FieldTypes = FxHashMap<(String, String), Type>;
type EnumDefs = FxHashMap<String, Vec<(String, Vec<Type>)>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Continue,
    /// `line` is the packed line active when the step started; a hit on the
    /// same line at the same or deeper frame isn't a stop.
    StepOver {
        depth: usize,
        line: i64,
    },
    StepIn {
        depth: usize,
        line: i64,
    },
    StepOut {
        depth: usize,
    },
    Pause,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Entry,
    Breakpoint,
    Step,
    Pause,
    Fault { code: String, message: String },
}

#[derive(Debug, Clone)]
pub struct FrameSnapshot {
    /// Read by frame-identity tests (recursion showing repeated frames);
    /// neither frontend's wire schema needs it.
    #[cfg_attr(not(test), allow(dead_code))]
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
    struct_fields: StructFields,
    field_types: FieldTypes,
    enum_defs: EnumDefs,
    /// One descriptor per named cell, built once at launch so `values.rs`
    /// never re-encodes a top-level cell's type on every variable request.
    cell_descs: FxHashMap<(u32, usize), CString>,
    pub(crate) var_store: VarStore,
}

fn pack(file_id: usize, line: u32) -> i64 {
    ((file_id as i64) << 32) | (line as i64)
}

impl EngineShared {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        program: DebugProgramInfo,
        file_names: FxHashMap<usize, String>,
        stop_on_entry: bool,
        events_tx: Sender<DebugEvent>,
        struct_fields: StructFields,
        field_types: FieldTypes,
        enum_defs: EnumDefs,
    ) -> std::sync::Arc<Self> {
        let mut cell_descs = FxHashMap::default();
        for func in &program.functions {
            for (cell_idx, cell) in func.cells.iter().enumerate() {
                let bytes = type_descriptor(&cell.ty, &struct_fields, &field_types, &enum_defs);
                let desc = CString::new(bytes.into_bytes())
                    .expect("type descriptor bytes are non-zero by construction");
                cell_descs.insert((func.fn_id, cell_idx), desc);
            }
        }
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
            struct_fields,
            field_types,
            enum_defs,
            cell_descs,
            var_store: VarStore::new(),
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

    /// Sets the run mode, bumps the token so a parked debuggee's wait loop
    /// wakes, and arms `force_check` so the stmt hook takes the slow path
    /// even with zero breakpoints set.
    fn resume(&self, mode: RunMode, force_check: bool) {
        let mut ctl = self.control.lock().unwrap();
        ctl.mode = mode;
        ctl.run_token = ctl.run_token.wrapping_add(1);
        drop(ctl);
        hooks::set_force_check(force_check);
        self.var_store.clear();
        self.resume_cv.notify_all();
    }

    /// The depth/line of the frame the debuggee is currently parked in,
    /// source for every step mode's starting point.
    fn stopped_depth_line(&self) -> (usize, i64) {
        let ctl = self.control.lock().unwrap();
        let depth = ctl.parked_frames.len();
        let line = ctl.parked_frames.last().map(|f| f.line).unwrap_or(0);
        (depth, line)
    }

    pub fn cont(&self) {
        // A pending stop-on-entry must still force the next `debug_stmt`
        // slow path even though we're leaving Pause/the initial barrier;
        // clearing this unconditionally would let entry_pending go unread
        // and the program would run straight past it.
        self.resume(
            RunMode::Continue,
            self.entry_pending.load(Ordering::Relaxed),
        );
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

    pub fn pause(&self) {
        self.resume(RunMode::Pause, true);
    }

    /// Runs to the next stmt hook at the same or a shallower frame on a
    /// different line: steps over a call without stopping inside it.
    pub fn next(&self) {
        let (depth, line) = self.stopped_depth_line();
        self.resume(RunMode::StepOver { depth, line }, true);
    }

    /// Runs to the next stmt hook that's either a different line at the
    /// same depth or any line at a deeper frame: follows into a call.
    pub fn step_in(&self) {
        let (depth, line) = self.stopped_depth_line();
        self.resume(RunMode::StepIn { depth, line }, true);
    }

    /// Runs until the frame stack drops below the depth it started at:
    /// finishes the current frame and stops back in the caller.
    pub fn step_out(&self) {
        let (depth, _) = self.stopped_depth_line();
        self.resume(RunMode::StepOut { depth }, true);
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

    /// Named cells of `fn_id` in the same order `debug_store`'s `cell_idx`
    /// indexes into a frame's raw `cells` vector.
    pub(crate) fn fn_cells(&self, fn_id: u32) -> &[crate::mir::debug_hooks::CellInfo] {
        self.program
            .functions
            .iter()
            .find(|f| f.fn_id == fn_id)
            .map(|f| f.cells.as_slice())
            .unwrap_or(&[])
    }

    /// `(fn_id, raw cell bits)` for the frame at `frame_idx`, `0` being the
    /// innermost frame -- the same indexing `stack()` returns. `None` while
    /// not parked or past the bottom of the stack.
    pub(crate) fn frame_cells(&self, frame_idx: usize) -> Option<(u32, Vec<i64>)> {
        let ctl = self.control.lock().unwrap();
        if !ctl.parked {
            return None;
        }
        let n = ctl.parked_frames.len();
        let f = ctl
            .parked_frames
            .get(n.checked_sub(1)?.checked_sub(frame_idx)?)?;
        Some((f.fn_id, f.cells.clone()))
    }

    pub(crate) fn cell_desc(&self, fn_id: u32, cell_idx: usize) -> &std::ffi::CStr {
        self.cell_descs
            .get(&(fn_id, cell_idx))
            .expect("cell_desc requested for an uninstrumented cell")
    }

    pub(crate) fn struct_fields(&self) -> &StructFields {
        &self.struct_fields
    }

    pub(crate) fn field_types(&self) -> &FieldTypes {
        &self.field_types
    }

    pub(crate) fn enum_defs(&self) -> &EnumDefs {
        &self.enum_defs
    }

    pub(crate) fn file_names(&self) -> &FxHashMap<usize, String> {
        &self.file_names
    }

    /// `depth` is the caller's live frame-stack length (TLS-local, so it has
    /// to be passed in rather than read here) at the point of this hit.
    pub(crate) fn stop_reason_for(&self, packed: i64, depth: usize) -> Option<StopReason> {
        if self.entry_pending.swap(false, Ordering::AcqRel) {
            return Some(StopReason::Entry);
        }
        let mode = self.control.lock().unwrap().mode;
        if matches!(mode, RunMode::Pause) {
            return Some(StopReason::Pause);
        }
        if hooks::is_breakpoint(packed) {
            return Some(StopReason::Breakpoint);
        }
        match mode {
            RunMode::StepOver { depth: saved, line } if depth <= saved && packed != line => {
                Some(StopReason::Step)
            }
            RunMode::StepIn { depth: saved, line } if depth > saved || packed != line => {
                Some(StopReason::Step)
            }
            RunMode::StepOut { depth: saved } if depth < saved => Some(StopReason::Step),
            _ => None,
        }
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
