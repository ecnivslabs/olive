//! Diagnostic metadata that lives alongside, but separately from, the codes the
//! compiler emits. `explain` holds the long-form prose behind every stable
//! `E####`/`W####` code, surfaced by `pit explain`.

pub mod explain;
