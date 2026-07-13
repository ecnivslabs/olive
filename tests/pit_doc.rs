//! Roadmap E13.4: `pit doc <file>` renders that module's public signatures
//! and `///` doc comments into `target/doc/<module>.md`.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

/// A throwaway working directory with the given `.liv` content, so
/// `target/doc/` output doesn't collide across parallel test runs.
fn write_project(name: &str, body: &str) -> PathBuf {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_pit_doc_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut f = std::fs::File::create(dir.join(format!("{name}.liv"))).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    dir
}

const SAMPLE_SRC: &str = "\
/// A point in 2D space.
struct Point:
    x: float
    y: float

/// Adds two points component-wise.
fn add(a: Point, b: Point) -> Point:
    return Point(a.x + b.x, a.y + b.y)

fn _internal() -> int:
    return 0

fn main():
    print(add(Point(1.0, 2.0), Point(3.0, 4.0)).x)
";

#[test]
fn renders_signatures_and_doc_comments() {
    let dir = write_project("sample", SAMPLE_SRC);

    let out = Command::new(pit_bin())
        .arg("doc")
        .arg("sample.liv")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit doc");
    assert!(
        out.status.success(),
        "pit doc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let md_path = dir.join("target/doc/sample.md");
    let md = std::fs::read_to_string(&md_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", md_path.display()));

    assert!(md.contains("# sample"), "{md}");
    assert!(
        md.contains("### `fn add(a: Point, b: Point) -> Point`"),
        "{md}"
    );
    assert!(md.contains("Adds two points component-wise."), "{md}");
    assert!(md.contains("### `struct Point`"), "{md}");
    assert!(md.contains("A point in 2D space."), "{md}");
    assert!(md.contains("- `x: float`"), "{md}");
    // Private (leading `_`) items don't appear in the public docs.
    assert!(!md.contains("_internal"), "{md}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn compile_error_blocks_doc_generation() {
    let dir = write_project("broken", "fn f() -> int:\n    return \"not an int\"\n");

    let out = Command::new(pit_bin())
        .arg("doc")
        .arg("broken.liv")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit doc");
    assert!(!out.status.success());
    assert!(!dir.join("target/doc/broken.md").exists());

    std::fs::remove_dir_all(&dir).ok();
}

/// A fenced Olive block inside a doc comment is compile-checked (E10.3's
/// machinery, reused): a genuine error in the embedded example is a warning
/// on stderr, and the docs are still written (one bad snippet shouldn't
/// block every other item's documentation).
#[test]
fn broken_embedded_example_warns_but_still_writes_docs() {
    let src = "\
/// Broken example.
///
/// ```olive
/// fn main():
///     let x: int = \"not an int\"
/// ```
fn noop():
    pass
";
    let dir = write_project("hasbadexample", src);

    let out = Command::new(pit_bin())
        .arg("doc")
        .arg("hasbadexample.liv")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit doc");
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not compile"), "{stderr}");
    assert!(dir.join("target/doc/hasbadexample.md").exists());

    std::fs::remove_dir_all(&dir).ok();
}
