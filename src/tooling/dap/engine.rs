//! Debug session state shared between the debuggee thread (parking on a
//! stop) and whichever thread drives the session (DAP server, headless
//! frontend, or a test). One instance per live session, held behind `Arc`
//! so both sides can reach it without the process-global statics in
//! `hooks` leaking any mutable state outside that file.

mod breakpoints;
mod threads;
mod variants;
mod watch;

use super::hooks::{self, Frame};
use super::values::VarStore;
use crate::mir::debug_hooks::DebugProgramInfo;
use crate::semantic::type_descriptor::type_descriptor;
use crate::semantic::types::Type;
use breakpoints::BpTable;
use rustc_hash::{FxHashMap, FxHashSet};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use threads::{MODE_CONTINUE, MODE_PAUSE, MODE_STEP_IN, MODE_STEP_OUT, MODE_STEP_OVER};
use watch::WatchTable;

pub use breakpoints::BpSpec;
pub(crate) use threads::{ThreadControl, any_force_check};
pub(crate) use variants::DebugVariantTable;
pub use watch::WatchSpec;

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
    DataBreakpoint,
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
    /// Packed `(thread_id, local frame index)`, opaque to both frontends --
    /// the same value comes back as a request's `frameId` and is decoded
    /// only by `EngineShared::frame_cells`/`set_local_cell`. Zero for a
    /// snapshot built outside `EngineShared::stack` (a fault's frame, before
    /// any stack request assigns it a real id).
    pub frame_id: usize,
}

#[derive(Debug, Clone)]
pub enum DebugEvent {
    Stopped {
        reason: StopReason,
        frame: FrameSnapshot,
        thread_id: i64,
    },
    Exited(i32),
    /// A logpoint firing, or a one-time conditional-breakpoint evaluation
    /// error -- text for an `output` event, never a stop.
    Output(String),
    /// Carries only the id: the DAP `thread` event body has no room for a
    /// name either, and anything that wants one already has `threads_snapshot`.
    ThreadStarted {
        id: i64,
    },
    ThreadExited {
        id: i64,
    },
}

pub struct EngineShared {
    /// Every traced thread for the life of this session, keyed by its
    /// stable DAP id. The main debuggee thread is always id 1, pre-created
    /// in `launch.rs` before that thread spawns; aio worker threads
    /// self-register the first time they call an instrumented function
    /// (`hooks::enable_debuggee_thread`).
    threads: Mutex<FxHashMap<i64, Arc<ThreadControl>>>,
    next_thread_id: AtomicI64,
    entry_pending: AtomicBool,
    breakpoints: Mutex<BpTable>,
    watchpoints: Mutex<WatchTable>,
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
    /// Dual-variant dispatch cells, set once at launch. `None` when the
    /// session's codegen didn't compile dual variants (shouldn't happen for
    /// a real `pit debug` launch, but keeps every call site a graceful no-op
    /// rather than a panic if it ever does).
    variant_table: OnceLock<DebugVariantTable>,
}

/// Packs a stable frame identity into the single opaque `usize` both DAP
/// frontends pass around as `frameId`. Subtracting 1 from `thread_id` makes
/// the main thread's (always id 1) frame ids identical to the plain local
/// index scheme this used to be -- `frameId: 0` still means "innermost frame
/// of the main thread" exactly as it did before threads existed, so every
/// existing single-thread request (most callers never send a real threadId
/// at all) keeps working unchanged.
const fn pack_frame(thread_id: i64, local_idx: usize) -> usize {
    (((thread_id - 1) as u64) << 32 | (local_idx as u64 & 0xFFFF_FFFF)) as usize
}

const fn unpack_frame(frame_id: usize) -> (i64, usize) {
    let raw = frame_id as u64;
    ((raw >> 32) as i64 + 1, (raw & 0xFFFF_FFFF) as usize)
}

/// Names resolved once at launch into `EngineShared::runtime_syms`.
const RUNTIME_SYM_NAMES: [&str; 19] = [
    "olive_format_typed",
    "olive_debug_seq_len",
    "olive_debug_seq_get",
    "olive_debug_dict_len",
    "olive_debug_dict_key",
    "olive_debug_dict_val",
    "olive_debug_enum_tag",
    "olive_debug_enum_payload",
    "olive_debug_str_bytes",
    "olive_debug_seq_set",
    "olive_debug_dict_set",
    "olive_debug_enum_set",
    "olive_debug_str_new",
    // Whole-aggregate `setVariable`/`setExpression` construction
    // (`setvar.rs::build_aggregate`): the same allocators codegen itself
    // uses for a list/dict/struct/enum literal, reached by name instead of
    // by direct linkage.
    "olive_list_new",
    "olive_obj_new",
    "olive_obj_set",
    "olive_obj_set_typed",
    "olive_struct_alloc",
    "olive_enum_new",
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
            threads: Mutex::new(FxHashMap::default()),
            next_thread_id: AtomicI64::new(1),
            entry_pending: AtomicBool::new(stop_on_entry),
            breakpoints: Mutex::new(FxHashMap::default()),
            watchpoints: Mutex::new(FxHashMap::default()),
            program,
            file_names,
            events_tx: Mutex::new(events_tx),
            struct_fields,
            field_types,
            enum_defs,
            cell_descs,
            var_store: VarStore::new(),
            runtime_syms: OnceLock::new(),
            variant_table: OnceLock::new(),
        })
    }

    /// Registers a new traced thread and assigns it a stable DAP id (the
    /// first-ever registration, always the main debuggee thread, gets id 1).
    /// Purely bookkeeping -- no `ThreadStarted` event, since the main
    /// thread's own registration (`launch.rs`, before that thread starts,
    /// so `wait_for_start`'s first `cont()` never races an unregistered
    /// thread 1) isn't a "new thread appeared mid-session" a client needs
    /// telling about; a self-registering aio worker announces itself
    /// separately (`announce_thread_started`), right where that's true.
    pub(crate) fn register_thread(&self, name: &str) -> Arc<ThreadControl> {
        let id = self.next_thread_id.fetch_add(1, Ordering::Relaxed);
        let ctl = Arc::new(ThreadControl::new(id, name.to_string()));
        self.threads.lock().unwrap().insert(id, ctl.clone());
        ctl
    }

    /// Removes a thread from the registry once it's run to completion. Pure
    /// bookkeeping, same as `register_thread` -- see `announce_thread_exited`
    /// for the event a client actually cares about. `detach()`s the outgoing
    /// control first: a thread that exits mid-step (its function returns
    /// past the frame a step was armed in, with no further stop to clear
    /// `force_check` naturally) would otherwise leak an armed count into
    /// `threads::FORCE_CHECK_COUNT` forever, forcing every other thread's
    /// statement hook onto the slow path for the rest of the session.
    pub(crate) fn deregister_thread(&self, id: i64) {
        if let Some(ctl) = self.threads.lock().unwrap().remove(&id) {
            ctl.detach();
        }
    }

    /// A traced thread appeared mid-session (an aio worker, never the main
    /// thread -- that one exists before any client could have asked about
    /// threads at all, so announcing it would just be noise ahead of the
    /// first real event).
    pub(crate) fn announce_thread_started(&self, id: i64) {
        let _ = self
            .events_tx
            .lock()
            .unwrap()
            .send(DebugEvent::ThreadStarted { id });
    }

    /// The aio-worker counterpart to `announce_thread_started`.
    pub(crate) fn announce_thread_exited(&self, id: i64) {
        let _ = self
            .events_tx
            .lock()
            .unwrap()
            .send(DebugEvent::ThreadExited { id });
    }

    pub(crate) fn thread_control(&self, id: i64) -> Option<Arc<ThreadControl>> {
        self.threads.lock().unwrap().get(&id).cloned()
    }

    /// `(id, name)` per currently-registered thread, sorted by id -- the
    /// `threads` DAP request's whole answer.
    pub fn threads_snapshot(&self) -> Vec<(i64, String)> {
        let mut v: Vec<(i64, String)> = self
            .threads
            .lock()
            .unwrap()
            .values()
            .map(|t| (t.id, t.name.clone()))
            .collect();
        v.sort_unstable_by_key(|&(id, _)| id);
        v
    }

    /// Installs the session's dual-variant dispatch table, once, at launch.
    /// A stop-on-entry session activates every variant immediately: the
    /// debuggee is still blocked in `wait_for_start`, nothing has run yet
    /// to narrow down to a "current" breakpoint set, and the entry stop
    /// itself needs `__main__` on its `$debug` body to have a `debug_stmt`
    /// call to catch it at all.
    pub(crate) fn install_variant_table(&self, table: DebugVariantTable) {
        if self.wants_stop_on_entry() {
            table.activate_all();
        }
        let _ = self.variant_table.set(table);
    }

    /// Every fn_id with at least one currently-set breakpoint or data
    /// breakpoint. Empty when nothing is set, in which case a plain
    /// `continue` should leave every function on its clean body. A watched
    /// function has to stay on the list too -- `debug_store` only exists on
    /// the `$debug` variant, so a function that's never independently
    /// breakpointed would revert to clean on its next call and the watch
    /// would silently stop firing the moment that call happened.
    fn bp_owning_fn_ids(&self) -> FxHashSet<u32> {
        let bps = self.breakpoints.lock().unwrap();
        let mut ids: FxHashSet<u32> = bps
            .keys()
            .filter_map(|packed| self.program.line_to_fn.get(packed).copied())
            .collect();
        drop(bps);
        ids.extend(
            self.watchpoints
                .lock()
                .unwrap()
                .keys()
                .map(|&(fn_id, _)| fn_id),
        );
        ids
    }

    /// Reverts every function to "instrumented only if it currently owns a
    /// breakpoint" -- the steady state a plain `continue` runs under.
    pub(crate) fn sync_variants_to_breakpoints(&self) {
        if let Some(table) = self.variant_table.get() {
            table.sync(&self.bp_owning_fn_ids());
        }
    }

    /// Every function instrumented, regardless of breakpoints -- required
    /// before any step/pause, since either can land anywhere.
    fn activate_all_variants(&self) {
        if let Some(table) = self.variant_table.get() {
            table.activate_all();
        }
    }

    /// Every function back to its clean body, ignoring whatever's still in
    /// `self.breakpoints` -- `detach()`'s "run to completion" no longer
    /// cares about any of them.
    fn deactivate_all_variants(&self) {
        if let Some(table) = self.variant_table.get() {
            table.sync(&FxHashSet::default());
        }
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

    /// Resolves `main`'s `ThreadControl` -- always id 1, pre-registered by
    /// `launch.rs` before the debuggee thread spawns.
    fn main_thread(&self) -> Option<Arc<ThreadControl>> {
        self.thread_control(1)
    }

    pub fn cont(&self, thread_id: i64) {
        // A pending stop-on-entry must still force the next `debug_stmt`
        // slow path even though we're leaving Pause/the initial barrier;
        // clearing this unconditionally would let entry_pending go unread
        // and the program would run straight past it.
        let entry_pending = self.entry_pending.load(Ordering::Relaxed);
        // The special first resume of a stop-on-entry session: `launch.rs`
        // already activated every variant so the entry stop can fire at
        // all, and nothing has run yet to have accumulated a "current"
        // breakpoint set worth narrowing down to. Every later `cont()`
        // (entry_pending now false) is a genuine free-run: only functions
        // that currently own a breakpoint stay instrumented.
        if !entry_pending {
            self.sync_variants_to_breakpoints();
        }
        self.var_store.clear();
        if let Some(t) = self.thread_control(thread_id) {
            t.resume(RunMode::Continue, entry_pending);
        }
    }

    /// Session teardown (`disconnect`/`quit`, `DebugSession::drop`): clears
    /// every stop condition and detaches every registered thread, so each
    /// is guaranteed to run to completion, never parking again even if one
    /// is mid-decision on a stop nobody will resume from once the caller
    /// moves on to `join()`-ing the debuggee thread.
    pub(crate) fn detach(&self) {
        self.entry_pending.store(false, Ordering::Relaxed);
        hooks::replace_breakpoints(FxHashSet::default());
        self.deactivate_all_variants();
        for t in self.threads.lock().unwrap().values() {
            t.detach();
        }
        self.var_store.clear();
    }

    /// Blocks the main debuggee thread until the first `cont()`, so
    /// breakpoints and stop-on-entry set up between `launch()` returning and
    /// the caller's first `cont()` are guaranteed to be in place before any
    /// user code runs -- without this, a short program can race past a
    /// breakpoint the caller hasn't finished installing yet.
    pub(crate) fn wait_for_start(&self) {
        if let Some(main) = self.main_thread() {
            main.wait_until_resumed(0);
        }
    }

    pub fn pause(&self, thread_id: i64) {
        self.activate_all_variants();
        if let Some(t) = self.thread_control(thread_id) {
            t.resume(RunMode::Pause, true);
        }
    }

    /// Toggles whether a runtime fault parks the debuggee before the process
    /// exits, per the client's `faults` exception-breakpoint filter.
    pub(crate) fn set_stop_on_faults(&self, v: bool) {
        hooks::set_stop_on_fault(v);
    }

    /// Runs to the next stmt hook at the same or a shallower frame on a
    /// different line: steps over a call without stopping inside it.
    pub fn next(&self, thread_id: i64) {
        self.activate_all_variants();
        if let Some(t) = self.thread_control(thread_id) {
            let (depth, line) = t.stopped_depth_line();
            t.resume(RunMode::StepOver { depth, line }, true);
        }
    }

    /// Runs to the next stmt hook that's either a different line at the
    /// same depth or any line at a deeper frame: follows into a call.
    pub fn step_in(&self, thread_id: i64) {
        self.activate_all_variants();
        if let Some(t) = self.thread_control(thread_id) {
            let (depth, line) = t.stopped_depth_line();
            t.resume(RunMode::StepIn { depth, line }, true);
        }
    }

    /// Runs until the frame stack drops below the depth it started at:
    /// finishes the current frame and stops back in the caller.
    pub fn step_out(&self, thread_id: i64) {
        self.activate_all_variants();
        if let Some(t) = self.thread_control(thread_id) {
            let (depth, _) = t.stopped_depth_line();
            t.resume(RunMode::StepOut { depth }, true);
        }
    }

    /// Valid only while `thread_id` is parked; the empty stack otherwise.
    /// Each frame's `frame_id` packs `thread_id` with its position so a
    /// later `scopes`/`evaluate`/`setVariable` request naming that id routes
    /// straight back to this thread's storage, without a threadId of its own.
    pub fn stack(&self, thread_id: i64) -> Vec<FrameSnapshot> {
        let Some(thread) = self.thread_control(thread_id) else {
            return Vec::new();
        };
        thread
            .stack_frames()
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let mut snap = self.snapshot(f);
                snap.frame_id = pack_frame(thread_id, i);
                snap
            })
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
            frame_id: 0,
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

    /// `(fn_id, raw cell bits)` for the packed `frame_id` -- the same value
    /// `stack()` handed back. `None` while that frame's owning thread isn't
    /// parked or the local index is past the bottom of its stack.
    pub(crate) fn frame_cells(&self, frame_id: usize) -> Option<(u32, Vec<i64>)> {
        let (thread_id, local_idx) = unpack_frame(frame_id);
        self.thread_control(thread_id)?.frame_at(local_idx)
    }

    /// Whether `frame_id` names the innermost frame of its thread -- the one
    /// frame whose real storage that thread will reload from the mirror
    /// before it next uses that local (`debug_load` fires right after the
    /// exact `park()` call this frame is blocked in; an outer frame isn't
    /// parked inside its own `debug_stmt` call at all, it's mid-call into
    /// whatever's currently innermost, so it has no reload point to apply a
    /// patch at until it becomes innermost again).
    pub(crate) fn is_innermost_frame(&self, frame_id: usize) -> bool {
        unpack_frame(frame_id).1 == 0
    }

    /// `setVariable`/`setExpression` write path for a top-level named local.
    /// Updates the inspector-visible snapshot immediately, then queues the
    /// mirror write for that frame's owning thread to pick up on resume.
    /// Callers must have already checked `is_innermost_frame`.
    pub(crate) fn set_local_cell(&self, frame_id: usize, cell_idx: usize, raw: i64) -> bool {
        let (thread_id, local_idx) = unpack_frame(frame_id);
        if local_idx != 0 {
            return false;
        }
        let Some(thread) = self.thread_control(thread_id) else {
            return false;
        };
        thread.set_local_cell(cell_idx, raw)
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
    /// `thread` is the hitting thread's own control -- its step state, never
    /// another thread's, decides whether this is a step stop.
    pub(crate) fn stop_reason_for(
        &self,
        thread: &ThreadControl,
        packed: i64,
        depth: usize,
        top: &Frame,
    ) -> Option<StopReason> {
        if self.entry_pending.swap(false, Ordering::AcqRel) {
            return Some(StopReason::Entry);
        }
        // Check the atomic mode first, avoiding the park mutex on every
        // statement. Only fall back to depth/line when the atomic fast path
        // can't decide (step modes need them).
        let variant = thread.mode_variant();
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
        let step_depth = thread.step_depth();
        let step_line = thread.step_line();
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

    /// Called from a traced thread on a stop condition. Records the frame
    /// snapshot, emits `Stopped` tagged with `thread`'s id, then blocks that
    /// thread until its own `cont()`/`pause()` bumps its run token. A no-op
    /// once `detach()` has run on this thread: that call and this one's
    /// first lock acquisition race on the same mutex, so whichever wins,
    /// the thread never blocks with no one left to resume it.
    pub(crate) fn park_on(&self, thread: &ThreadControl, reason: StopReason, frames: Vec<Frame>) {
        let top = frames
            .last()
            .map(|f| self.snapshot(f))
            .unwrap_or(FrameSnapshot {
                fn_id: 0,
                name: String::new(),
                file: String::new(),
                line: 0,
                frame_id: 0,
            });

        let Some(token) = thread.begin_park(frames) else {
            return;
        };

        let _ = self.events_tx.lock().unwrap().send(DebugEvent::Stopped {
            reason,
            frame: top,
            thread_id: thread.id,
        });

        thread.wait_until_resumed(token);
    }

    pub(crate) fn send_exited(&self, code: i32) {
        let _ = self
            .events_tx
            .lock()
            .unwrap()
            .send(DebugEvent::Exited(code));
    }
}
