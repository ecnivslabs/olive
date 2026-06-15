//! The breakpoint table: plain line breakpoints plus conditions, hit
//! counts, and logpoints. Split out of `engine.rs` to keep that file under
//! the line-count cap; `check_breakpoint` runs inside the stmt hook's stop
//! path, called from `EngineShared::stop_reason_for`, only after the
//! packed-line hash has already matched a set breakpoint.

use super::{DebugEvent, EngineShared, pack};
use crate::tooling::dap::conditions::{self, Expr, HitCondition, LogTemplate};
use crate::tooling::dap::hooks::{self, Frame};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A requested breakpoint line plus its optional condition, hit condition,
/// and log message, exactly as a frontend sent them.
pub struct BpSpec {
    pub line: u32,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
}

/// Parsed, ready-to-evaluate form of a `BpSpec`, indexed by packed
/// `(file_id << 32) | line`. Parsing happens once, at `set_breakpoints_with`
/// time, so a hit never re-parses its own condition.
pub(super) struct BpProps {
    condition: Option<Expr>,
    hit_condition: Option<HitCondition>,
    log_template: Option<LogTemplate>,
    hits: AtomicU64,
    /// A condition evaluation error is reported once, not on every hit.
    errored: AtomicBool,
}

pub(super) type BpTable = FxHashMap<i64, BpProps>;

impl EngineShared {
    /// Replaces every breakpoint for `file_id`, unconditional (no condition,
    /// hit count, or log message). Returns, per requested line, the actual
    /// line used and whether it landed on an instrumented statement: exact
    /// match is verified as-is; otherwise the nearest following instrumented
    /// line in the same file is verified; if none follows, the original line
    /// is returned unverified and nothing is set.
    ///
    /// Both frontends always send full `BpSpec`s now, so only tests (and
    /// this doc comment's simpler description) still reach this directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_breakpoints(&self, file_id: usize, lines: &[u32]) -> Vec<(u32, bool)> {
        let specs: Vec<BpSpec> = lines
            .iter()
            .map(|&line| BpSpec {
                line,
                condition: None,
                hit_condition: None,
                log_message: None,
            })
            .collect();
        self.set_breakpoints_with(file_id, &specs)
    }

    /// Same resolution rule as `set_breakpoints`, but each line carries its
    /// own optional condition/hit-condition/log-message, parsed here so a
    /// hit never re-parses its own breakpoint. A spec that fails to parse
    /// reports the error once via an output event and falls back to an
    /// unconditional breakpoint for that piece.
    pub fn set_breakpoints_with(&self, file_id: usize, specs: &[BpSpec]) -> Vec<(u32, bool)> {
        let mut out = Vec::with_capacity(specs.len());
        let mut entries = FxHashMap::default();
        for spec in specs {
            match self.resolve_line(file_id, spec.line) {
                Some(snapped) => {
                    entries.insert(pack(file_id, snapped), self.build_props(spec));
                    out.push((snapped, true));
                }
                None => out.push((spec.line, false)),
            }
        }
        let mut bps = self.breakpoints.lock().unwrap();
        bps.retain(|&k, _| (k >> 32) as usize != file_id);
        bps.extend(entries);
        drop(bps);
        self.rebuild_breakpoint_index();
        out
    }

    fn build_props(&self, spec: &BpSpec) -> BpProps {
        let condition = spec.condition.as_deref().and_then(|c| {
            conditions::parse_condition(c)
                .inspect_err(|msg| self.emit_output(format!("breakpoint condition error: {msg}\n")))
                .ok()
        });
        let hit_condition = spec.hit_condition.as_deref().and_then(|h| {
            conditions::parse_hit_condition(h)
                .inspect_err(|msg| {
                    self.emit_output(format!("breakpoint hit condition error: {msg}\n"))
                })
                .ok()
        });
        let log_template = spec.log_message.as_deref().and_then(|l| {
            conditions::parse_log_template(l)
                .inspect_err(|msg| self.emit_output(format!("logpoint error: {msg}\n")))
                .ok()
        });
        BpProps {
            condition,
            hit_condition,
            log_template,
            hits: AtomicU64::new(0),
            errored: AtomicBool::new(false),
        }
    }

    /// Every instrumented line in `file_id` within `[start, end]`, for the
    /// `breakpointLocations` request -- lets a client snap a breakpoint onto
    /// a line that actually has code before ever calling `setBreakpoints`.
    pub(crate) fn lines_in_range(&self, file_id: usize, start: u32, end: u32) -> Vec<u32> {
        self.program
            .lines
            .iter()
            .filter_map(|&packed| {
                let f = (packed >> 32) as usize;
                let l = (packed & 0xFFFF_FFFF) as u32;
                (f == file_id && l >= start && l <= end).then_some(l)
            })
            .collect()
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

    pub(super) fn rebuild_breakpoint_index(&self) {
        let keys: FxHashSet<i64> = self.breakpoints.lock().unwrap().keys().copied().collect();
        hooks::replace_breakpoints(keys);
    }

    /// Whether a line already known to carry a breakpoint should actually
    /// park the debuggee: a plain breakpoint always does; a false condition
    /// or an unmet hit count vetoes it (falling through to step-mode checks
    /// in the caller); a logpoint never stops, it only emits output.
    pub(super) fn check_breakpoint(&self, packed: i64, top: &Frame) -> bool {
        let bps = self.breakpoints.lock().unwrap();
        let Some(props) = bps.get(&packed) else {
            return false;
        };
        if let Some(cond) = &props.condition {
            match conditions::eval_expr(self, top, cond) {
                Ok(true) => {}
                Ok(false) => return false,
                Err(msg) => {
                    if !props.errored.swap(true, Ordering::Relaxed) {
                        self.emit_output(format!("breakpoint condition error: {msg}\n"));
                    }
                }
            }
        }
        if let Some(template) = &props.log_template {
            let text = conditions::render_log(self, top, template);
            self.emit_output(text);
            return false;
        }
        match &props.hit_condition {
            Some(hit) => hit.matches(props.hits.fetch_add(1, Ordering::Relaxed) + 1),
            None => true,
        }
    }

    pub(super) fn emit_output(&self, text: String) {
        let _ = self
            .events_tx
            .lock()
            .unwrap()
            .send(DebugEvent::Output(text));
    }
}
