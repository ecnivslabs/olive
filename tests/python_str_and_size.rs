//! R18: single-pass, length-carrying string crossing in both directions
//! (`PyUnicode_FromStringAndSize`/`PyUnicode_AsUTF8AndSize`), proven from
//! real Olive source on both pipelines (JIT `pit run`, AOT release). Covers
//! a multibyte round-trip (CJK, emoji) and the embedded-NUL fix (a string
//! sourced from Python with an interior NUL byte, which the old
//! strlen-based crossing silently truncated).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

const STRHELPER_PY: &str = r#"
def echo(s):
    return s

def cjk_and_emoji():
    return "héllo 世界 🎉 — done"

def embedded_nul():
    return "ab\x00cd"

def check_len_and_content(s, expected_len, expected):
    return len(s) == expected_len and s == expected
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_str_and_size_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("strhelper.py"), STRHELPER_PY).unwrap();
    let liv_path = dir.join("main.liv");
    let mut f = std::fs::File::create(&liv_path).unwrap();
    f.write_all(src.as_bytes()).unwrap();
    (dir, liv_path)
}

fn run_jit(dir: &Path, liv_path: &Path) -> Output {
    Command::new(pit_bin())
        .arg("run")
        .arg(liv_path)
        .env("PYTHONPATH", dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run")
}

fn run_aot(dir: &Path, liv_path: &Path) -> Output {
    let out_bin = liv_path.with_extension("bin");
    let build = Command::new(pit_bin())
        .arg("build")
        .arg("--release")
        .arg(liv_path)
        .arg("-o")
        .arg(&out_bin)
        .env("PYTHONPATH", dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit build");
    assert!(
        build.status.success(),
        "AOT build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(&out_bin)
        .env("PYTHONPATH", dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn built binary");
    std::fs::remove_file(&out_bin).ok();
    out
}

fn assert_identical_on_both_pipelines(src: &str, expected: &str) {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let (dir, liv_path) = write_case(src);

    let jit = run_jit(&dir, &liv_path);
    assert!(
        jit.status.success(),
        "pit run failed: {}",
        String::from_utf8_lossy(&jit.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&jit.stdout),
        expected,
        "pit run stderr: {}",
        String::from_utf8_lossy(&jit.stderr)
    );

    let aot = run_aot(&dir, &liv_path);
    assert!(
        aot.status.success(),
        "AOT failed: {}",
        String::from_utf8_lossy(&aot.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&aot.stdout),
        expected,
        "AOT stderr: {}",
        String::from_utf8_lossy(&aot.stderr)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn multibyte_cjk_and_emoji_round_trips_through_python_and_back() {
    assert_identical_on_both_pipelines(
        r#"import py "strhelper" as h

fn main():
    let original: str = h.cjk_and_emoji()
    let echoed: str = h.echo(original)
    print(echoed)

main()
"#,
        "héllo 世界 🎉 — done\n",
    );
}

#[test]
fn multibyte_literal_crosses_to_python_and_back_unchanged() {
    assert_identical_on_both_pipelines(
        r#"import py "strhelper" as h

fn main():
    let s: str = "café 日本語 🚀"
    let echoed: str = h.echo(s)
    print(echoed)

main()
"#,
        "café 日本語 🚀\n",
    );
}

#[test]
fn embedded_nul_from_python_survives_the_full_round_trip() {
    assert_identical_on_both_pipelines(
        r#"import py "strhelper" as h

fn main():
    let s: str = h.embedded_nul()
    print(len(s))
    let ok = h.check_len_and_content(s, 5, h.embedded_nul())
    print(ok)

main()
"#,
        "5\nTrue\n",
    );
}

#[test]
fn empty_string_round_trips_both_directions() {
    assert_identical_on_both_pipelines(
        r#"import py "strhelper" as h

fn main():
    let s: str = h.echo("")
    print(len(s))
    let back: str = h.echo(s)
    print(len(back))

main()
"#,
        "0\n0\n",
    );
}

#[test]
fn multibyte_dict_key_and_value_survive_the_boundary() {
    assert_identical_on_both_pipelines(
        r#"import py "strhelper" as h

fn main():
    let d = {"世界": "🎉"}
    let back = h.echo(d)
    print(back["世界"])

main()
"#,
        "🎉\n",
    );
}
