//! Long-form explanations for every diagnostic code Olive can emit. Each entry
//! says what the code means, why it fires, and shows the offending code next to
//! the corrected version. This is the `rustc --explain` equivalent, served by
//! `pit explain <CODE>`. Entries are split by category so no single file grows
//! unwieldy; this module aggregates them and answers lookups.

mod borrow;
mod control;
mod lints;
mod names;
mod python;
mod runtime;
mod types;

/// One code's explanation. All fields are static so the whole registry is a
/// compile-time table with no per-lookup allocation.
pub struct Explanation {
    pub code: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub wrong: &'static str,
    pub fixed: &'static str,
    pub notes: &'static [&'static str],
}

const CATEGORIES: &[&[Explanation]] = &[
    names::ENTRIES,
    types::ENTRIES,
    borrow::ENTRIES,
    control::ENTRIES,
    python::ENTRIES,
    runtime::ENTRIES,
    lints::ENTRIES,
];

/// Every explanation, in category then declaration order.
pub fn all() -> impl Iterator<Item = &'static Explanation> {
    CATEGORIES.iter().flat_map(|c| c.iter())
}

/// The explanation for `code`, matched case-insensitively (`e0400` == `E0400`).
pub fn lookup(code: &str) -> Option<&'static Explanation> {
    let code = code.trim().to_ascii_uppercase();
    all().find(|e| e.code == code)
}

/// Every known code, for nearest-match suggestions on an unknown one.
pub fn codes() -> Vec<&'static str> {
    all().map(|e| e.code).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every code the compiler and runtime can emit. The completeness test below
    /// fails the build if any of these lacks an explanation, so a newly added
    /// code cannot ship without one.
    const KNOWN_CODES: &[&str] = &[
        "E0001", "E0002", "E0003", "E0004", "E0006", "E0100", "E0200", "E0300", "E0301", "E0400",
        "E0401", "E0402", "E0403", "E0404", "E0405", "E0406", "E0407", "E0408", "E0409", "E0410",
        "E0411", "E0412", "E0413", "E0414", "E0415", "E0416", "E0417", "E0418", "E0419", "E0420",
        "E0421", "E0422", "E0423", "E0424", "E0425", "E0500", "E0501", "E0502", "E0503", "E0504",
        "E0505", "E0506", "E0507", "E0600", "E0601", "E0602", "E0700", "E0701", "E0702", "E0703",
        "E0704", "E0705", "E0706", "E0707", "E0708", "W0601", "W0602", "W0610", "W0620", "W0630",
        "W0640", "W0650", "W0660",
    ];

    #[test]
    fn every_known_code_has_an_explanation() {
        for code in KNOWN_CODES {
            assert!(
                lookup(code).is_some(),
                "no explanation registered for {code}"
            );
        }
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(lookup("e0400").map(|e| e.code), Some("E0400"));
        assert_eq!(lookup(" E0400 ").map(|e| e.code), Some("E0400"));
    }

    #[test]
    fn no_duplicate_codes() {
        let mut seen = std::collections::HashSet::new();
        for e in all() {
            assert!(seen.insert(e.code), "duplicate explanation for {}", e.code);
        }
    }

    #[test]
    fn registry_matches_known_codes_exactly() {
        let known: std::collections::HashSet<&str> = KNOWN_CODES.iter().copied().collect();
        let registered: std::collections::HashSet<&str> = codes().into_iter().collect();
        assert_eq!(
            registered, known,
            "explanation registry and KNOWN_CODES have drifted"
        );
    }

    #[test]
    fn no_entry_is_empty() {
        for e in all() {
            assert!(!e.title.is_empty(), "{} has an empty title", e.code);
            assert!(!e.summary.is_empty(), "{} has an empty summary", e.code);
            assert!(!e.wrong.is_empty(), "{} has no wrong example", e.code);
            assert!(!e.fixed.is_empty(), "{} has no fixed example", e.code);
        }
    }

    #[test]
    fn unknown_code_is_none() {
        assert!(lookup("E9999").is_none());
        assert!(lookup("nonsense").is_none());
    }

    /// Codes whose `fixed` example depends on something outside a self-contained
    /// compile: an external Olive module, the Python interpreter, or FFI linkage.
    const SKIP_FIXED: &[&str] = &[
        "E0300", "E0004", "E0408", "E0409", "E0600", "E0601", "E0602", "E0705", "E0706", "W0601",
        "W0602", "W0630",
    ];
    /// Codes whose `wrong` example does not fail this in-process compile: runtime
    /// faults that still type-check (E0700-E0707); interpreter-dependent Python
    /// checks; FFI linkage (E0408/E0409); defensive checks unreachable from source
    /// (E0420 recursive type, E0502 borrow-before-init, E0406 result propagation);
    /// and E0504/E0507, whose conflicts the optimizer folds away before the borrow
    /// pass here, though `pit build` still reports them on the unoptimized program.
    /// E0501 is defensive: ownership inference only moves a value at its last
    /// use, so source code cannot reach a use-after-move; the check remains as
    /// a net against compiler bugs. E0708 is defensive for the same reason: a
    /// value escaping across a call boundary is always conservatively unknown,
    /// and a value escaping into a container is copied rather than aliased, so
    /// ordinary source cannot reach the every-path-stale shape the compile-time
    /// check exists to reject; it stays as a net against a future regression in
    /// those protections (verified instead by dedicated gencheck unit tests).
    /// E0301 and E0004 need a second file (an imported module / a cross-module
    /// private name), so they cannot fail as a lone entry. E0503 and E0505 share a
    /// condition and message with an earlier pass (the unsafe-deref check and
    /// E0411), which reports first on the minimal example. All `W####` warnings
    /// compile by design, so their `wrong` form never fails.
    const SKIP_WRONG: &[&str] = &[
        "E0700", "E0701", "E0702", "E0703", "E0704", "E0705", "E0706", "E0707", "E0708", "E0301",
        "E0004", "E0406", "E0420", "E0501", "E0502", "E0503", "E0504", "E0505", "E0507", "E0408",
        "E0409", "E0600", "E0601", "E0602", "W0601", "W0602", "W0610", "W0620", "W0630", "W0640",
        "W0650", "W0660",
    ];

    /// Every `fixed` example must compile and every `wrong` example must fail to,
    /// so `pit explain` can never show invalid Olive as the answer. This pins the
    /// examples to the real compiler, not the author's memory of the syntax.
    #[test]
    fn examples_match_their_codes() {
        use crate::compile::pipeline::run_pipeline;
        // Hold CWD_LOCK for the duration: some fixed examples (e.g. E0406, E0704)
        // import the stdlib via a cwd-relative path, and other tests call
        // set_current_dir while holding this lock. Without it the two can race on
        // Windows, causing the stdlib lookup to fail.
        let _cwd_guard = crate::commands::utils::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("olive_explain_verify_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for e in all() {
            if !SKIP_FIXED.contains(&e.code) {
                let p = dir.join(format!("{}_fixed.liv", e.code));
                std::fs::write(&p, e.fixed).unwrap();
                assert!(
                    run_pipeline(p.to_str().unwrap()).is_ok(),
                    "{} fixed example must compile",
                    e.code
                );
            }
            if !SKIP_WRONG.contains(&e.code) {
                let p = dir.join(format!("{}_wrong.liv", e.code));
                std::fs::write(&p, e.wrong).unwrap();
                assert!(
                    run_pipeline(p.to_str().unwrap()).is_err(),
                    "{} wrong example must fail to compile",
                    e.code
                );
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
