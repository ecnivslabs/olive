//! Launches a debug session: lean-pipeline compile, MIR instrumentation,
//! JIT, and a spawned debuggee thread. Mirrors `compile::run_jit_to_exit_code`
//! minus PGO, tier-up, and shadow-stack instrumentation, which the debugger's
//! own frames and fault-hook support supersede.
//!
//! `main.rs` doesn't call into this subsystem yet, so most of it is
//! unreachable from the bin target's `main`; `tests.rs` already exercises
//! it in full.
#![cfg_attr(not(test), allow(dead_code))]

use super::engine::{DebugEvent, EngineShared};
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
    events_rx: Receiver<DebugEvent>,
    _codegen: CraneliftCodegen<JITModule>,
    debuggee: Option<JoinHandle<()>>,
}

impl Deref for DebugSession {
    type Target = EngineShared;
    fn deref(&self) -> &Self::Target {
        &self.shared
    }
}

impl DebugSession {
    pub fn events(&self) -> &Receiver<DebugEvent> {
        &self.events_rx
    }
}

impl Drop for DebugSession {
    fn drop(&mut self) {
        // Force any parked debuggee to resume and run to completion before
        // this session's slot frees up for the next `launch()`, so two
        // sessions never execute JIT'd code concurrently in the same process.
        self.shared.cont();
        if let Some(handle) = self.debuggee.take() {
            let _ = handle.join();
        }
        hooks::clear_session();
    }
}

pub fn launch(program: &str, stop_on_entry: bool) -> Result<DebugSession, LaunchError> {
    let mut out =
        run_pipeline_opt(program, false, None, false).map_err(|_| LaunchError::Compile)?;
    let program_info = debug_hooks::instrument(&mut out.functions);

    let mut codegen = CraneliftCodegen::new_jit(
        out.functions,
        out.struct_fields.clone(),
        out.field_types.clone(),
        out.enum_defs.clone(),
        out.vtables.clone(),
        out.global_vars.clone(),
        out.file_names.clone(),
        &out.native_libs,
        false,
    );
    codegen.generate();
    codegen.finalize();

    let Some(main_ptr) = codegen.get_function("__main__") else {
        return Err(LaunchError::NoMain);
    };

    let (tx, rx) = mpsc::channel();
    let shared = EngineShared::new(program_info, out.file_names.clone(), stop_on_entry, tx);
    hooks::install_session(shared.clone()).map_err(|_| LaunchError::SessionActive)?;

    let main_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(main_ptr) };
    let debuggee_shared = shared.clone();
    let debuggee = std::thread::Builder::new()
        .name("olive-debuggee".to_string())
        .spawn(move || {
            hooks::enable_debuggee();
            debuggee_shared.wait_for_start();
            let exit_code = main_fn();
            hooks::disable_debuggee();
            hooks::clear_session();
            debuggee_shared.send_exited(exit_code as i32);
        })
        .expect("failed to spawn debuggee thread");

    Ok(DebugSession {
        shared,
        events_rx: rx,
        _codegen: codegen,
        debuggee: Some(debuggee),
    })
}
