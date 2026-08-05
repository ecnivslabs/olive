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

