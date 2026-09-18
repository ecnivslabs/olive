//! Maybe-borrow returns and union-held resource structs.
//!
//! A function whose return may alias a parameter (a parameter reassigned in a
//! loop before return, an identity return, a container read) used to strand
//! fresh objects: the caller classified the result as a view and dropped the
//! word on the floor (one object per call), while the union drop-hook path
//! for resource structs (`__drop__`) never reached a zero gate because the
//! construction temp held its allocation reference without ever releasing it.
//! These tests pin the value-correctness half; the leak half is covered by
//! RSS probes (1KB payloads, 200K iterations, flat ~279MB post-fix).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

fn run_src(src: &str) -> String {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("olive_maybe_borrow_{}_{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.liv");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(src.as_bytes()).unwrap();

    let out = Command::new(pit_bin())
        .arg("run")
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "pit run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Parameter reassigned in a loop before return: the fresh object must reach
/// the caller intact (pre-fix it stranded one object per call and the caller
/// kept a copy of the right value, so this also guards the value path).
#[test]
fn loop_reassigned_param_returns_fresh_value() {
    let out = run_src(
        "fn pad(s: str) -> str:\n    \
             let mut r = s\n    \
             while len(r) < 16:\n        \
             r = r + \"ab\"\n    \
             return r\n\n\
         fn main():\n    \
             print(pad(\"v\"))\n    \
             print(pad(\"0123456789abcdef\"))\n\n\
         main()\n",
    );
    assert!(out.contains("vababababababab"), "unexpected output: {out}");
    assert!(out.contains("0123456789abcdef"), "unexpected output: {out}");
}

/// Zero-trip path still aliases: no copy is observable, value intact, no
/// double free at scope end.
#[test]
fn zero_trip_alias_return_stays_valid() {
    let out = run_src(
        "fn pad(s: str) -> str:\n    \
             let mut r = s\n    \
             while len(r) < 4:\n        \
             r = r + \"ab\"\n    \
             return r\n\n\
         fn big() -> str:\n    \
             let mut r = \"v\"\n    \
             while len(r) < 8:\n        \
             r = r + \"ab\"\n    \
             return r\n\n\
         fn main():\n    \
             let b = big()\n    \
             let a = pad(b)\n    \
             print(len(a))\n    \
             print(a == b)\n\n\
         main()\n",
    );
    assert!(out.contains("9"), "unexpected output: {out}");
    assert!(out.contains("True"), "unexpected output: {out}");
}

/// A struct with `__drop__` held through a union runs its hook exactly once
/// at scope end (pre-fix the gate never reached zero: no hook, leaked box).
#[test]
fn union_held_struct_runs_drop_hook() {
    let out = run_src(
        "struct Loud:\n    \
             s: str\n\n\
         impl Loud:\n    \
             fn __drop__(self):\n        \
                 print(\"dropped\")\n\n\
         fn mk(bad: int) -> Loud | int:\n    \
             if bad == 1:\n        \
                 return 0\n    \
             return Loud(\"hi\")\n\n\
         fn main():\n    \
             let m = mk(0)\n    \
             if m == 0:\n        \
                 print(\"zero\")\n        \
                 return\n    \
             print(\"live\")\n\n\
         main()\n",
    );
    assert!(out.contains("live"), "unexpected output: {out}");
    assert_eq!(
        out.matches("dropped").count(),
        1,
        "unexpected output: {out}"
    );
}

/// Borrow-param identity return of a struct: caller keeps its value, hook
/// runs once for the single allocation (pre-fix E0707/gate miscounts).
#[test]
fn borrow_param_struct_identity() {
    let out = run_src(
        "struct Loud:\n    \
             s: str\n\n\
         impl Loud:\n    \
             fn __drop__(self):\n        \
                 print(\"dropped\")\n\n\
         fn box_it(x: Loud) -> Loud | int:\n    \
             return x\n\n\
         fn main():\n    \
             let t = Loud(\"hi\")\n    \
             let u = box_it(t)\n    \
             match u:\n        \
                 0:\n            \
                 print(\"zero\")\n        \
                 v:\n            \
                 print(\"live\")\n    \
             print(\"end\")\n\n\
         main()\n",
    );
    assert!(out.contains("live"), "unexpected output: {out}");
    assert!(out.contains("end"), "unexpected output: {out}");
    assert_eq!(
        out.matches("dropped").count(),
        1,
        "unexpected output: {out}"
    );
}

/// Mutation through a returned struct must not alias the caller's value
/// (value semantics): the ownership split gives the caller an independent
/// copy, so writing through `u` leaves `t` intact.
#[test]
fn returned_struct_mutation_does_not_alias() {
    let out = run_src(
        "struct Box2:\n    \
             s: str\n\n\
         fn idb(x: Box2) -> Box2:\n    \
             return x\n\n\
         fn main():\n    \
             let t = Box2(\"orig\")\n    \
             let u = idb(t)\n    \
             u.s = \"mutated\"\n    \
             print(t.s)\n    \
             print(u.s)\n\n\
         main()\n",
    );
    assert!(out.contains("orig"), "unexpected output: {out}");
    assert!(out.contains("mutated"), "unexpected output: {out}");
    assert!(
        out.lines().filter(|l| l.trim() == "orig").count() == 1,
        "aliasing violation in output: {out}"
    );
}

/// Match with an `int` arm narrows the catch-all the same way a `== 0`
/// guard does: the fallible-constructor union flows into member slots
/// without an explicit guard.
#[test]
fn match_int_arm_narrows_fallible_constructor() {
    let out = run_src(
        "import aio\n\n\
         async fn producer(ch: aio.Chan[str]) -> bool:\n    \
             return ch.send(\"hi\")\n\n\
         fn main():\n    \
             let ch = aio.chan[str]()\n    \
             match ch:\n        \
                 0:\n            \
                 print(\"alloc-fail\")\n        \
                 c:\n            \
                 print(await producer(c))\n\n\
         main()\n",
    );
    assert!(out.contains("True"), "unexpected output: {out}");
}

/// Mutex through the fallible-constructor union: lock/unlock/drop round-trips
/// values correctly (slot release covered by the M1 RSS probe).
#[test]
fn mutex_union_lock_unlock_drop() {
    let out = run_src(
        "import aio\n\n\
         fn main():\n    \
             let mut total = 0\n    \
             let mut i = 0\n    \
             while i < 200:\n        \
                 let m = aio.mutex[str](\"v\" + str(i))\n        \
                 if m == 0:\n            \
                     print(\"alloc-fail\")\n            \
                     return\n        \
                 let v = m.lock()\n        \
                 m.unlock(\"w\" + str(i))\n        \
                 total = total + len(v)\n        \
                 i = i + 1\n    \
             print(total)\n\n\
         main()\n",
    );
    assert!(out.contains("690"), "unexpected output: {out}");
}
