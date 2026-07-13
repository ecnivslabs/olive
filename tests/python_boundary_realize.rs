//! R2a: the olive-to-Python boundary always realizes a genuine `list`/`dict`,
//! never a zero-copy proxy. `isinstance` and library functions that
//! `isinstance`-check (like `json.dumps`) must succeed against an
//! Olive-passed collection, both pipelines.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
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

fn write_src(src: &str) -> PathBuf {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "olive_boundary_realize_{}_{id}.liv",
        std::process::id()
    ));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(src.as_bytes()).unwrap();
    path
}

const SRC: &str = r#"import py "json" as json
import py "builtins" as b

fn main():
    let mut d = {"name": "olive", "count": 41}
    let is_dict = b.isinstance(d, b.dict)
    print(is_dict)
    let s = json.dumps(d)
    print(s)

    let xs = [1, 3, 5]
    let is_list = b.isinstance(xs, b.list)
    print(is_list)

main()
"#;

const EXPECTED: &str = "True\n{\"name\": \"olive\", \"count\": 41}\nTrue\n";

#[test]
fn realized_collections_pass_isinstance_under_jit() {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let path = write_src(SRC);

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "pit run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stdout,
        EXPECTED,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn realized_collections_pass_isinstance_under_aot_release() {
    if !python_available() {
        eprintln!("Python not available, skipping test");
        return;
    }
    let path = write_src(SRC);
    let out_bin = path.with_extension("bin");

    let build = Command::new(pit_bin())
        .arg("build")
        .arg("--release")
        .arg(&path)
        .arg("-o")
        .arg(&out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit build");
    assert!(
        build.status.success(),
        "AOT build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let out = Command::new(&out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn built binary");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "built binary failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stdout,
        EXPECTED,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    std::fs::remove_file(&path).ok();
    std::fs::remove_file(&out_bin).ok();
}
