//! Roadmap E13.3: `#[bench]` discovery mirrors `#[test]`'s, and `pit bench
//! --json` has a schema a tool can parse without eyeballing.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

/// A throwaway pit project: `pit.toml` plus a `src/main.liv` carrying the
/// given body. `pit bench` (like `pit test`) only ever operates on a project,
/// never a standalone file.
fn write_project(body: &str) -> PathBuf {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_bench_cmd_{}_{id}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let mut toml = std::fs::File::create(dir.join("pit.toml")).unwrap();
    writeln!(toml, "[pod]\nname = \"benchtest\"\nversion = \"0.1.0\"\n").unwrap();
    let mut main = std::fs::File::create(dir.join("src/main.liv")).unwrap();
    main.write_all(body.as_bytes()).unwrap();
    dir
}

const PROJECT_SRC: &str = "\
fn add(a: int, b: int) -> int:
    return a + b

#[bench]
fn add_bench() -> int:
    return add(2, 3)

fn main():
    print(add(1, 1))
";

/// Minimal JSON scanner for `[{...}, {...}]` -- avoids pulling a JSON crate
/// into the test suite for one shape. Returns each top-level object's raw
/// text.
fn split_json_array_objects(json: &str) -> Vec<String> {
    let inner = json.trim().trim_start_matches('[').trim_end_matches(']');
    let mut objs = Vec::new();
    let mut depth = 0;
    let mut start = None;
    for (i, c) in inner.char_indices() {
        match c {
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0
                    && let Some(s) = start
                {
                    objs.push(inner[s..=i].to_string());
                }
            }
            _ => {}
        }
    }
    objs
}

fn field_f64(obj: &str, field: &str) -> f64 {
    let key = format!("\"{field}\": ");
    let start = obj
        .find(&key)
        .unwrap_or_else(|| panic!("missing {field} in {obj}"))
        + key.len();
    let rest = &obj[start..];
    let end = rest.find([',', '}']).unwrap();
    rest[..end]
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("bad {field} in {obj}"))
}

fn field_str(obj: &str, field: &str) -> String {
    let key = format!("\"{field}\": \"");
    let start = obj
        .find(&key)
        .unwrap_or_else(|| panic!("missing {field} in {obj}"))
        + key.len();
    let rest = &obj[start..];
    let end = rest.find('"').unwrap();
    rest[..end].to_string()
}

/// `pit bench --json` emits one object per `#[bench]` function with
/// `name`/`mean_ns`/`stddev_ns`/`min_ns`/`samples`, all sane (non-negative,
/// `min_ns <= mean_ns`, `samples` matches the fixed sample count).
#[test]
fn json_schema_is_well_formed() {
    let dir = write_project(PROJECT_SRC);

    let out = Command::new(pit_bin())
        .arg("bench")
        .arg("--json")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit bench");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "pit bench --json failed: stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let objs = split_json_array_objects(&stdout);
    assert_eq!(objs.len(), 1, "expected exactly 1 bench entry: {stdout}");

    let obj = &objs[0];
    assert_eq!(field_str(obj, "name"), "add_bench");
    let mean = field_f64(obj, "mean_ns");
    let stddev = field_f64(obj, "stddev_ns");
    let min = field_f64(obj, "min_ns");
    let samples = field_f64(obj, "samples");
    assert!(mean >= 0.0, "{obj}");
    assert!(stddev >= 0.0, "{obj}");
    assert!(min >= 0.0, "{obj}");
    assert!(min <= mean + 1.0, "min must not exceed mean: {obj}");
    assert_eq!(samples, 30.0, "sample count: {obj}");

    std::fs::remove_dir_all(&dir).ok();
}

/// A bench with no `#[bench]` functions produces an empty array, not an
/// error -- a project with only tests is a legitimate `pit bench` target.
#[test]
fn no_bench_functions_produces_empty_array() {
    let dir = write_project("fn main():\n    print(1)\n");

    let out = Command::new(pit_bin())
        .arg("bench")
        .arg("--json")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit bench");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout={stdout}");
    assert_eq!(split_json_array_objects(&stdout).len(), 0, "{stdout}");

    std::fs::remove_dir_all(&dir).ok();
}

/// The human-readable form (no `--json`) names the bench and reports all
/// three statistics.
#[test]
fn human_output_names_bench_and_stats() {
    let dir = write_project(PROJECT_SRC);

    let out = Command::new(pit_bin())
        .arg("bench")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit bench");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout={stdout}");
    assert!(stdout.contains("add_bench"), "{stdout}");
    assert!(stdout.contains("mean:"), "{stdout}");
    assert!(stdout.contains("stddev:"), "{stdout}");
    assert!(stdout.contains("min:"), "{stdout}");
    assert!(stdout.contains("30 samples"), "{stdout}");

    std::fs::remove_dir_all(&dir).ok();
}
