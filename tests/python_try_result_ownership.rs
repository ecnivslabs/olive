//! Regression for the try-py result double-ownership defect: `lower_try_py`
//! left the safe call's `Result` wire temp owning, so its scope-exit `Drop`
//! freed the Ok payload through `olive_free_result` *after* `unwrap` had
//! already handed that payload to the unwrapped value. Two owners, one
//! handle: the first free released the Python object, PYOBJ-slab recycling
//! reassigned the slot, and the second free (the payload word's own drop)
//! decrefed whichever unrelated object now occupied it -- corrupting
//! long-lived handles (a shared tiktoken encoder crashed crank after
//! thousands of tool turns). The fix gives every result accessor consume
//! semantics (it frees the slot after handing the payload out) and makes the
//! try-py result temp non-owning so no scope drop is emitted at all. Both
//! repros here segfaulted pre-fix on the JIT and debug-AOT pipelines.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

fn python_available() -> bool {
    for cmd in &["python3", "python"] {
        if Command::new(cmd)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return true;
        }
    }
    false
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

const KEEPER_PY: &str = r#"
class Keeper:
    def __init__(self):
        self.base = [10, 20, 30]

    def make(self, n):
        return [n, n + 1]
"#;

fn write_case(src: &str) -> (PathBuf, PathBuf) {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_try_own_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("keeper.py"), KEEPER_PY).unwrap();
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

fn assert_both_succeed(src: &str, expected: &str) {
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

const ITERATIONS: usize = 5000;

/// The minimal shape: a `try <py-call>() -> PyObject` result flows out of one
/// function, into a local of another, and gets used there -- once per
/// iteration, so the freed-then-recycled PYOBJ slab slot is reoccupied and
/// the stale duplicate's second free lands on an unrelated handle within the
/// loop.
#[test]
fn try_result_payload_survives_scope_drop() {
    assert_both_succeed(
        &format!(
            r#"import py "builtins" as pyb

fn make_str(n: int) -> PyObject:
    return try pyb.repr(n)

fn use_str(n: int) -> PyObject:
    let s = make_str(n)
    return s

fn main():
    let mut i = 0
    let mut total = 0
    while i < {ITERATIONS}:
        let s = use_str(i)
        total = total + len(s)
        i = i + 1
    print(total)

main()
"#
        ),
        "18890\n",
    );
}

/// The consumer shape the crash surfaced through: one long-lived Python
/// object is loaded through a matched try-result into a struct field, then a
/// *second* try-py call runs every iteration. Each iteration's wrongful
/// payload free drops the long-lived object's refcount (safe-call payloads
/// own their reference outright), and once it hits zero the stored handle is
/// dead -- the next method invocation on it dereferences freed memory.
/// Pre-fix this segfaults within the loop on both pipelines.
#[test]
fn matched_try_result_in_shared_struct_survives_reuse() {
    assert_both_succeed(
        r#"import py "keeper" as keeper

struct Wrapper:
    obj: PyObject
    ready: bool

fn _load() -> PyObject | Error:
    return try keeper.Keeper()

fn new_wrapper() -> Wrapper:
    match _load():
        Error(_e):
            return Wrapper(None, False)
        o:
            return Wrapper(o, True)

fn count(w: Wrapper, n: int) -> int:
    if not w.ready:
        return 0
    let res: PyObject | Error = try w.obj.make(n)
    match res:
        Error(_e):
            return 0
        v:
            return len(v)

fn copy_wrapper(w: Wrapper) -> Wrapper:
    return w

fn main():
    let t = new_wrapper()
    let mut i = 0
    let mut total = 0
    while i < 3000:
        let c = copy_wrapper(t)
        total = total + count(c, i)
        let lst = [c]
        let got = lst[0]
        total = total + count(got, i)
        i = i + 1
    print(total)

main()
"#,
        "12000\n",
    );
}
