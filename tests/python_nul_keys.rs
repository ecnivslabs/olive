//! Embedded-NUL dict keys across the Python boundary.
//!
//! Python strings can hold NULs; Olive-native strings cannot create them,
//! but Python-derived ones arrive intact. Ingest must not collapse distinct
//! keys (`"ab\0cd"` vs `"ab"` used to merge into one entry via a `strlen`
//! key copy); export skips NUL keys like the deep exporter does rather
//! than truncating them into collisions.

#[path = "support/program.rs"]
mod program;
use program::assert_both;

#[test]
fn nul_key_survives_ingest_distinct() {
    assert_both(
        "import py \"json\" as json\n\nfn main():\n    let d = dict(json.loads(\"{\\\"ab\\\\u0000cd\\\": 1, \\\"ab\\\": 2}\"))\n    print(len(d))\n",
        "2\n",
    );
}
