//! Debug session state shared between the debuggee thread (parking on a
//! stop) and whichever thread drives the session (DAP server, headless
//! frontend, or a test). One instance per live session, held behind `Arc`
//! so both sides can reach it without the process-global statics in
//! `hooks` leaking any mutable state outside that file.

mod breakpoints;

use super::hooks::{self, Frame};
use super::values::VarStore;
use crate::mir::debug_hooks::DebugProgramInfo;
use crate::semantic::type_descriptor::type_descriptor;
use crate::semantic::types::Type;
use breakpoints::BpTable;
use rustc_hash::{FxHashMap, FxHashSet};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Condvar, Mutex, OnceLock};

pub use breakpoints::BpSpec;

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
    /// A logpoint firing, or a one-time conditional-breakpoint evaluation
    /// error -- text for an `output` event, never a stop.
    Output(String),
}

struct Control {
    mode: RunMode,
    parked: bool,
    parked_frames: Vec<Frame>,
    run_token: u64,
    /// Set by `detach()` on session teardown so a `park()` call that's
    /// already past its stop decision, racing the teardown's own resume
    /// signal, still never blocks: the check happens under the same lock
    /// `detach()` writes under, so one side or the other always wins cleanly.
    detached: bool,
}

// D13c: Atomic-encoded RunMode variant, checked in `stop_reason_for`
// before touching the control mutex. The full `Control` struct still holds
// the authoritative mode + parked_frames for the rare multi-field write.
const MODE_CONTINUE: u8 = 0;
const MODE_STEP_OVER: u8 = 1;
const MODE_STEP_IN: u8 = 2;
const MODE_STEP_OUT: u8 = 3;
const MODE_PAUSE: u8 = 4;

pub struct EngineShared {
    control: Mutex<Control>,
    resume_cv: Condvar,
    /// D13c: Atomic-mode fields so `stop_reason_for` reads the mode
    /// without acquiring the control mutex on every statement.
    mode_variant: AtomicU8,
    step_depth: AtomicUsize,
    step_line: AtomicI64,
    entry_pending: AtomicBool,
    breakpoints: Mutex<BpTable>,
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
    /// Runtime function addresses resolved once at launch (`launch.rs`,
    /// right after the JIT module finalizes) so the stmt hook can decode
    /// values for a condition/logpoint without reaching into the session's
    /// `CraneliftCodegen`, which only `DebugSession` owns.
    runtime_syms: OnceLock<FxHashMap<&'static str, usize>>,
}

/// Names resolved once at launch into `EngineShared::runtime_syms`.
const RUNTIME_SYM_NAMES: [&str; 9] = [
    "olive_format_typed",
    "olive_debug_seq_len",
    "olive_debug_seq_get",
    "olive_debug_dict_len",
    "olive_debug_dict_key",
    "olive_debug_dict_val",
    "olive_debug_enum_tag",
    "olive_debug_enum_payload",
    "olive_debug_str_bytes",
];

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
                detached: false,
            }),
            resume_cv: Condvar::new(),
            mode_variant: AtomicU8::new(MODE_CONTINUE),
            step_depth: AtomicUsize::new(0),
            step_line: AtomicI64::new(0),
            entry_pending: AtomicBool::new(stop_on_entry),
            breakpoints: Mutex::new(FxHashMap::default()),
            program,
            file_names,
            events_tx: Mutex::new(events_tx),
            struct_fields,
            field_types,
            enum_defs,
            cell_descs,
            var_store: VarStore::new(),
            runtime_syms: OnceLock::new(),
        })
    }

    pub(crate) fn wants_stop_on_entry(&self) -> bool {
        self.entry_pending.load(Ordering::Relaxed)
    }

    /// Resolves and caches the runtime symbols value decoding needs, once,
    /// from the session's own `CraneliftCodegen`. Must run before the
    /// debuggee thread starts: after this, the hook path never touches
    /// `resolve` again, only the cached addresses.
    pub(crate) fn install_runtime_symbols(&self, resolve: impl Fn(&str) -> Option<*const u8>) {
        let map = RUNTIME_SYM_NAMES
            .iter()
            .filter_map(|&name| resolve(name).map(|p| (name, p as usize)))
            .collect();
        let _ = self.runtime_syms.set(map);
    }

    pub(crate) fn runtime_symbol(&self, name: &str) -> Option<*const u8> {
        self.runtime_syms
            .get()?
            .get(name)
            .map(|&addr| addr as *const u8)
    }

    /// D13c: Encode `RunMode` into the atomic mode fields so
    /// `stop_reason_for` can read them without locking the control mutex.
    fn set_mode_atomic(&self, mode: &RunMode) {
        match *mode {
            RunMode::Continue => {
                self.mode_variant.store(MODE_CONTINUE, Ordering::Relaxed);
            }
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
            RunMode::Pause => {
                self.mode_variant.store(MODE_PAUSE, Ordering::Relaxed);
            }
        }
    }

    /// Sets the run mode, bumps the token so a parked debuggee's wait loop
    /// wakes, and arms `force_check` so the stmt hook takes the slow path
    /// even with zero breakpoints set.
    fn resume(&self, mode: RunMode, force_check: bool) {
        self.set_mode_atomic(&mode);
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

    /// Session teardown (`disconnect`/`quit`, `DebugSession::drop`): clears
    /// every stop condition and marks the session detached so the debuggee
    /// is guaranteed to run to completion, never parking again even if it's
    /// mid-decision on a stop nobody will resume from once the caller moves
    /// on to `join()`-ing the debuggee thread.
    pub(crate) fn detach(&self) {
        self.entry_pending.store(false, Ordering::Relaxed);
        self.mode_variant.store(MODE_CONTINUE, Ordering::Relaxed);
        self.step_depth.store(0, Ordering::Relaxed);
        self.step_line.store(0, Ordering::Relaxed);
        hooks::replace_breakpoints(FxHashSet::default());
        let mut ctl = self.control.lock().unwrap();
        ctl.detached = true;
        ctl.mode = RunMode::Continue;
        ctl.run_token = ctl.run_token.wrapping_add(1);
        drop(ctl);
        hooks::set_force_check(false);
        self.var_store.clear();
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

    pub fn pause(&self) {
        self.resume(RunMode::Pause, true);
    }

    /// Toggles whether a runtime fault parks the debuggee before the process
    /// exits, per the client's `faults` exception-breakpoint filter.
    pub(crate) fn set_stop_on_faults(&self, v: bool) {
        hooks::set_stop_on_fault(v);
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
    /// to be passed in rather than read here) at the point of this hit);
    /// `top` is the frame the hook just captured, the source for any
    /// condition or logpoint evaluation this line's breakpoint needs.
    pub(crate) fn stop_reason_for(
        &self,
        packed: i64,
        depth: usize,
        top: &Frame,
    ) -> Option<StopReason> {
        if self.entry_pending.swap(false, Ordering::AcqRel) {
            return Some(StopReason::Entry);
        }
        // D13c: Check atomic mode first, avoiding the control mutex on
        // every statement. Only fall back to the mutex when the atomic
        // fast path can't decide (step modes need depth/line).
        let variant = self.mode_variant.load(Ordering::Relaxed);
        if variant == MODE_PAUSE {
            return Some(StopReason::Pause);
        }
        if variant == MODE_CONTINUE {
            // Fast path: no stepping, check breakpoints only.
            if hooks::contains_breakpoint(packed) && self.check_breakpoint(packed, top) {
                return Some(StopReason::Breakpoint);
            }
            return None;
        }
        // Step modes need depth/line from the atomic fields.
        let step_depth = self.step_depth.load(Ordering::Relaxed);
        let step_line = self.step_line.load(Ordering::Relaxed);
        if hooks::contains_breakpoint(packed) && self.check_breakpoint(packed, top) {
            return Some(StopReason::Breakpoint);
        }
        match variant {
            MODE_STEP_OVER if depth <= step_depth && packed != step_line => Some(StopReason::Step),
            MODE_STEP_IN if depth > step_depth || packed != step_line => Some(StopReason::Step),
            MODE_STEP_OUT if depth < step_depth => Some(StopReason::Step),
            _ => None,
        }
    }

    /// Called from the debuggee thread on a stop condition. Records the
    /// frame snapshot, emits `Stopped`, then blocks until `cont()`/`pause()`
    /// bumps the run token. A no-op once `detach()` has run: that call and
    /// this one's first lock acquisition race on the same mutex, so whichever
    /// wins, the debuggee never blocks with no one left to resume it.
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
        if ctl.detached {
            return;
        }
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
