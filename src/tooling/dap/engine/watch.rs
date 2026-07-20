//! Data breakpoints: write-only, one entry per `(fn_id, cell_idx)` -- the
//! same key `debug_store`'s hook indexes by. Read/readWrite access types
//! aren't supported: `debug_load` is a once-per-statement mirror reload,
//! not a per-expression read tracker, so a real read watch would need a
//! new hook at every MIR load site rather than reusing this checkpoint.
//! Condition/hit-condition evaluation reuses `conditions.rs` verbatim, the
//! same machinery `breakpoints.rs` uses for a line breakpoint's condition.

use super::{EngineShared, StopReason};
use crate::tooling::dap::conditions::{self, Expr, HitCondition};
use crate::tooling::dap::hooks::{self, Frame};
use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A requested data breakpoint: `data_id` is whatever `dataBreakpointInfo`
/// handed back (`"{fn_id}:{cell_idx}"`, see `EngineShared::data_id_for`).
pub struct WatchSpec {
    pub data_id: String,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
}

pub(super) struct WatchProps {
    condition: Option<Expr>,
    hit_condition: Option<HitCondition>,
    hits: AtomicU64,
    /// A condition evaluation error is reported once, not on every hit.
    errored: AtomicBool,
}

pub(super) type WatchTable = FxHashMap<(u32, usize), WatchProps>;

fn parse_data_id(id: &str) -> Option<(u32, usize)> {
    let (fn_part, cell_part) = id.split_once(':')?;
    Some((fn_part.parse().ok()?, cell_part.parse().ok()?))
}

impl EngineShared {
    /// Resolves a top-level local in `frame_idx` to a stable `dataId`.
    /// `None` when the name isn't a named local of that frame -- nested
    /// container fields aren't watchable, `debug_store` only ever fires for
    /// a named cell's own slot, never a field/element write inside it.
    pub(crate) fn data_id_for(&self, frame_idx: usize, name: &str) -> Option<String> {
        let (fn_id, _) = self.frame_cells(frame_idx)?;
        let idx = self.fn_cells(fn_id).iter().position(|c| c.name == name)?;
        Some(format!("{fn_id}:{idx}"))
    }

    /// Replaces the entire active data breakpoint set -- the same
    /// replace-all contract `setDataBreakpoints` has: the client resends
    /// everything it wants active on every call, nothing is diffed against
    /// a prior set. Returns `(verified, message)` per spec, in request order.
    pub fn set_data_breakpoints(&self, specs: &[WatchSpec]) -> Vec<(bool, Option<String>)> {
        let mut out = Vec::with_capacity(specs.len());
        let mut entries = WatchTable::default();
        let mut packed_keys = rustc_hash::FxHashSet::default();
        for spec in specs {
            match parse_data_id(&spec.data_id) {
                Some((fn_id, cell_idx)) => {
                    entries.insert((fn_id, cell_idx), self.build_watch_props(spec));
                    packed_keys.insert(hooks::pack_watch(fn_id, cell_idx));
                    out.push((true, None));
                }
                None => out.push((false, Some(format!("invalid dataId '{}'", spec.data_id)))),
            }
        }
        *self.watchpoints.lock().unwrap() = entries;
        hooks::replace_watchpoints(packed_keys);
        out
    }

    fn build_watch_props(&self, spec: &WatchSpec) -> WatchProps {
        let condition = spec.condition.as_deref().and_then(|c| {
            conditions::parse_condition(c)
                .inspect_err(|msg| {
                    self.emit_output(format!("data breakpoint condition error: {msg}\n"))
                })
                .ok()
        });
        let hit_condition = spec.hit_condition.as_deref().and_then(|h| {
            conditions::parse_hit_condition(h)
                .inspect_err(|msg| {
                    self.emit_output(format!("data breakpoint hit condition error: {msg}\n"))
                })
                .ok()
        });
        WatchProps {
            condition,
            hit_condition,
            hits: AtomicU64::new(0),
            errored: AtomicBool::new(false),
        }
    }

    /// Called from `debug_store`'s slow path, after the write already
    /// landed in the frame mirror -- a condition sees the new value, not
    /// the one being overwritten. Mirrors `check_breakpoint`'s exact
    /// condition/hit-condition control flow (an evaluation error is
    /// reported once but doesn't veto the stop).
    pub(crate) fn stop_reason_for_watch(
        &self,
        fn_id: u32,
        cell_idx: usize,
        top: &Frame,
    ) -> Option<StopReason> {
        let watches = self.watchpoints.lock().unwrap();
        let props = watches.get(&(fn_id, cell_idx))?;
        if let Some(cond) = &props.condition {
            match conditions::eval_expr(self, top, cond) {
                Ok(true) => {}
                Ok(false) => return None,
                Err(msg) => {
                    if !props.errored.swap(true, Ordering::Relaxed) {
                        self.emit_output(format!("data breakpoint condition error: {msg}\n"));
                    }
                }
            }
        }
        let stop = match &props.hit_condition {
            Some(hit) => hit.matches(props.hits.fetch_add(1, Ordering::Relaxed) + 1),
            None => true,
        };
        stop.then_some(StopReason::DataBreakpoint)
    }
}
