//! Launches a debug session: lean-pipeline compile, MIR instrumentation,
//! JIT, and a spawned debuggee thread. Mirrors `compile::run_jit_to_exit_code`
//! minus PGO, tier-up, and shadow-stack instrumentation, which the debugger's
//! own frames and fault-hook support supersede.

use super::engine::{DebugEvent, DebugVariantTable, EngineShared};
use super::hooks;
use crate::codegen::cranelift::CraneliftCodegen;
use crate::compile::pipeline::run_pipeline_opt;
use crate::mir::debug_hooks;
use cranelift_jit::JITModule;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

#[derive(Debug)]
pub enum LaunchError {
    Compile,
    NoMain,
    SessionActive,
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::Compile => write!(f, "compilation failed"),
            LaunchError::NoMain => write!(f, "no `main` function found"),
            LaunchError::SessionActive => write!(f, "a debug session is already active"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// Owns everything a live debug session needs: the shared engine state, the
/// event stream, the JIT module (kept alive for the process/session's whole
/// life, same as `run_jit_to_exit_code` never freeing its codegen), and the
/// debuggee thread handle.
pub struct DebugSession {
    shared: Arc<EngineShared>,
    events_rx: Option<Receiver<DebugEvent>>,
    /// Never read again after `launch()` resolves runtime symbols and gets
    /// `__main__`'s pointer (`raw_fn` is the one test-only exception); held
    /// only so the JIT module and its libraries stay mapped for the
    /// session's whole life.
    #[cfg_attr(not(test), allow(dead_code))]
    codegen: CraneliftCodegen<JITModule>,
    debuggee: Option<JoinHandle<()>>,
}

impl Deref for DebugSession {
    type Target = EngineShared;
    fn deref(&self) -> &Self::Target {
        &self.shared
    }
}

impl DebugSession {
    /// Used by tests and by callers that don't hand the session off to a
    /// forwarder thread; `server`/`headless` use `take_events` instead.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn events(&self) -> &Receiver<DebugEvent> {
        self.events_rx
            .as_ref()
            .expect("events receiver already taken")
    }

    /// Hands ownership of the event stream to a dedicated forwarder thread
    /// (`tooling::dap::server`, `tooling::dap::headless`); callers that need
    /// `events()` after this must not call it again. Panics if called twice.
    pub(crate) fn take_events(&mut self) -> Receiver<DebugEvent> {
        self.events_rx
            .take()
            .expect("events receiver already taken")
    }

    /// `name`'s `$debug` variant address -- `get_function` would hand back
    /// the clean body's fixed address instead, since dispatch between the
    /// two lives in each *caller's* compiled call site (an indirect load
    /// through the dispatch cell), not in the callee's own entry point.
    /// Lets a multi-thread test call a second function directly on its own
    /// manually-spawned thread, exactly as `aio`'s `spawn_traced` calls into
    /// a spawned function, without needing a real `async fn`/executor pool
    /// in the fixture program.
    #[cfg(test)]
    pub(crate) fn raw_fn(&mut self, name: &str) -> Option<*const u8> {
        self.codegen
            .debug_variant_addr(name)
            .map(|addr| addr as *const u8)
    }
}

impl Drop for DebugSession {
    fn drop(&mut self) {
        // Force any parked (or about to park) debuggee to run to completion
        // before this session's slot frees up for the next `launch()`, so
        // two sessions never execute JIT'd code concurrently in the same
        // process. `detach()`, not `cont()`: a plain resume only wakes a
        // debuggee already parked -- one that hits a fresh stop condition
        // moments later would park again with nobody left to send the next
        // `cont()`, hanging `join()` below forever.
        self.shared.detach();
        if let Some(handle) = self.debuggee.take() {
            let _ = handle.join();
        }
        hooks::clear_session();
    }
}

pub fn launch(program: &str, stop_on_entry: bool) -> Result<DebugSession, LaunchError> {
    let out = run_pipeline_opt(program, false, None, false).map_err(|_| LaunchError::Compile)?;

    // Two bodies per debug-instrumentable function. `clean_functions`
    // (same per-line safepoints as the full variant, but deferred stores)
    // is what codegen compiles as the *primary* set -- the default a
    // session runs with nothing watching it. `debug_functions` (today's
    // full instrument()) gets compiled separately as each function's
    // `$debug` variant, wired in but not activated until something needs it.
    let mut debug_functions = out.functions.clone();
    let program_info = debug_hooks::instrument(&mut debug_functions);
    let clean_functions = debug_hooks::instrument_clean(&out.functions);

    let mut codegen = CraneliftCodegen::new_jit(
        clean_functions,
        out.struct_fields.clone(),
        out.field_types.clone(),
        out.enum_defs.clone(),
        out.vtables.clone(),
        out.global_vars.clone(),
        out.file_names.clone(),
        &out.native_libs,
        false,
    );
    codegen.debug_dual_variant = true;
    codegen.generate();
    codegen.finalize();

    for func in &debug_functions {
        codegen.install_debug_variant(func);
    }
    codegen.finalize();

    let mut variant_table = DebugVariantTable::new();
    for info in &program_info.functions {
        if let (Some(cell_ptr), Some(clean_addr), Some(debug_addr)) = (
            codegen.dispatch_cell_ptr(&info.name),
            codegen.clean_variant_addr(&info.name),
            codegen.debug_variant_addr(&info.name),
        ) {
            variant_table.insert(info.fn_id, cell_ptr as *mut i64, clean_addr, debug_addr);
        }
    }

    let Some(main_ptr) = codegen.get_function("__main__") else {
        return Err(LaunchError::NoMain);
    };

    let (tx, rx) = mpsc::channel();
    let shared = EngineShared::new(
        program_info,
        out.file_names.clone(),
        stop_on_entry,
        tx,
        out.struct_fields.clone(),
        out.field_types.clone(),
        out.enum_defs.clone(),
    );
    hooks::install_session(shared.clone()).map_err(|_| LaunchError::SessionActive)?;
    shared.install_variant_table(variant_table);
    shared.install_runtime_symbols(|name| codegen.runtime_symbol(name));

    if let Some(setter) = codegen.runtime_symbol("olive_debug_set_fault_hook") {
        let install: extern "C" fn(i64) = unsafe { std::mem::transmute(setter) };
        install(hooks::debug_fault_hook as *const () as i64);
    }

    // Lets `olive_std::aio`'s executor pool, spawned tasks, and pool_run(_sync)
    // -- all of which run real olive-compiled code on their own OS thread --
    // register themselves with this session the same way the main thread
    // does, instead of staying invisible to breakpoints/stepping/inspection.
    if let Some(setter) = codegen.runtime_symbol("olive_debug_set_thread_hooks") {
        let install: extern "C" fn(i64, i64) = unsafe { std::mem::transmute(setter) };
        install(
            hooks::debug_thread_start as *const () as i64,
            hooks::debug_thread_end as *const () as i64,
        );
    }

    let main_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(main_ptr) };
    // Registered here, synchronously, before the debuggee thread exists --
    // not inside its closure. `wait_for_start`'s first `cont()` can arrive
    // the instant `configurationDone` does; if thread 1 weren't already in
    // the registry by then, that `cont()` would find nothing to resume and
    // the debuggee would block forever waiting for a wakeup nobody sends.
    let main_ctl = shared.register_thread("main");
    if shared.wants_stop_on_entry() {
        main_ctl.set_force_check(true);
    }
    let debuggee_shared = shared.clone();
    let debuggee = std::thread::Builder::new()
        .name("olive-debuggee".to_string())
        .spawn(move || {
            hooks::attach_debuggee_thread(main_ctl);
            debuggee_shared.wait_for_start();
            let exit_code = main_fn();
            hooks::detach_main_thread();
            hooks::clear_session();
            debuggee_shared.send_exited(exit_code as i32);
        })
        .expect("failed to spawn debuggee thread");

    Ok(DebugSession {
        shared,
        events_rx: Some(rx),
        codegen,
        debuggee: Some(debuggee),
    })
}
