//! Generates small Olive programs, runs debug JIT and release AOT, diffs
//! stdout. Divergence = soundness bug in one pipeline.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Fixed for reproducible repro; program text is deterministic per seed.
const FIXED_SEEDS: &[u64] = &[1, 2, 3, 4, 5, 8, 13, 21, 34, 55, 89, 144];

const ROTATING_SEED_COUNT: u64 = 8;

struct Gen {
    rng: StdRng,
}

impl Gen {
    fn new(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
        }
    }

    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        self.rng.random_range(lo..hi)
    }

    /// Covers moves, borrows, container escapes, loops, reassignment, Any
    /// mixing, nested fns. No oracle needed -- just diffed vs the other pipeline.
    fn program(&mut self) -> String {
        let list_len = self.range(3, 9);
        let base = self.range(1, 50);
        let loop_n = self.range(3, 10);
        let struct_val = self.range(1, 100);
        let branch_pick = self.range(0, 3);
        let closure_add = self.range(1, 20);
        let opt_pick_a = self.range(-10, 10);
        let opt_pick_b = self.range(-10, 10);
        let leaf_n = self.range(-6, 16);
        let pair_a = self.range(-3, 9);
        let pair_b = self.range(-3, 9);
        let trio_a = self.range(-5, 5);
        let trio_b = self.range(-5, 5);
        let trio_c = self.range(-5, 5);
        let list_pat_n = self.range(0, 12);

        let list_lit = (0..list_len)
            .map(|i| (base + i).to_string())
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            r#"struct Box:
    items: [int]

fn make_box(xs: [int]) -> Box:
    return Box(xs)

fn double_all(xs: [int]) -> [int]:
    let mut out = []
    for x in &xs:
        out.append(x * 2)
    return out

fn sum_and_last(xs: [int]) -> int:
    let last = xs[len(xs) - 1]
    return sum(xs) + last

fn moves_and_borrows() -> int:
    let mut a = [{list_lit}]
    let b = a
    let doubled = double_all(b)
    let total = sum_and_last(doubled)
    return total

fn struct_escape() -> int:
    let mut src = [{struct_val}, {struct_val}, {struct_val}]
    let boxed = make_box(src)
    src[0] = -1
    return boxed.items[0] + src[0]

fn loop_reassign(n: int) -> int:
    let mut acc = []
    let mut i = 0
    while i < n:
        acc = acc + [i]
        i += 1
    return sum(acc)

fn any_mixing() -> str:
    let row: [Any] = [1, "olive", True, None]
    let mut out = ""
    for v in &row:
        if type(v) == "int":
            out = out + "i"
        elif type(v) == "str":
            out = out + "s"
        elif type(v) == "bool":
            out = out + "b"
        else:
            out = out + "n"
    return out

fn nested_closure() -> int:
    let delta = {closure_add}
    fn adder(x: int) -> int:
        return x + delta
    return adder({base})

struct Callback:
    f: fn(int) -> int

fn make_adder(n: int) -> fn(int) -> int:
    return lambda x: x + n

fn escaping_closures() -> int:
    let add = make_adder({closure_add})
    let cb = Callback(add)
    let fns = [add, cb.f]
    return fns[0]({base}) + fns[1]({base}) + cb.f({base})

fn branch_pick(n: int) -> int:
    if n == 0:
        return moves_and_borrows()
    elif n == 1:
        return struct_escape()
    else:
        return loop_reassign({loop_n})

fn maybe_box(n: int) -> Box | None:
    if n > 0:
        return make_box([n, n, n])
    return None

fn maybe_list(n: int) -> [int] | None:
    if n > 0:
        return [n, n, n]
    return None

fn guard_len(xs: [int] | None) -> int:
    // None-union guard: narrowing must let `len` see the plain `[int]`.
    if xs == None:
        return -1
    return len(xs)

fn opt_items_len(b: Box | None) -> int:
    // `?.` plus `??`: absent receiver or absent field both fall to the default.
    let items = b?.items ?? []
    return len(items)

enum Node:
    Leaf(int)
    Pair(int, int)
    Trio(int, int, int)

fn classify(n: Node) -> str:
    // Nested range/wildcard/or/guard patterns inside variant payloads.
    match n:
        Leaf(0):
            return "leaf-zero"
        Leaf(x) if x < 0:
            return "leaf-neg"
        Leaf(1..10):
            return "leaf-small"
        Pair(0, 0) | Pair(0, _) | Pair(_, 0):
            return "pair-has-zero"
        Pair(a, b) if a == b:
            return "pair-equal"
        Trio(a, b, c):
            return "trio:" + str(a + b + c)
        _:
            return "other"

fn nested_list_pattern(xs: [(int, int)]) -> int:
    // List-with-rest whose elements are themselves tuple patterns.
    match xs:
        []:
            return 0
        [(a, b)]:
            return a + b
        [(a, b), *rest]:
            return a - b + len(rest)

fn pattern_playground(n: int, pa: int, pb: int, ta: int, tb: int, tc: int, ln: int) -> str:
    let leaf = classify(Leaf(n))
    let pair = classify(Pair(pa, pb))
    let trio = classify(Trio(ta, tb, tc))
    let listed = nested_list_pattern([(ln, ln + 1), (ln + 2, ln + 3), (ln + 4, ln + 5)])
    return leaf + "," + pair + "," + trio + "," + str(listed)

fn main():
    print(moves_and_borrows())
    print(struct_escape())
    print(loop_reassign({loop_n}))
    print(any_mixing())
    print(nested_closure())
    print(escaping_closures())
    print(branch_pick({branch_pick}))
    print(opt_items_len(maybe_box({opt_pick_a})))
    print(guard_len(maybe_list({opt_pick_b})))
    print(pattern_playground({leaf_n}, {pair_a}, {pair_b}, {trio_a}, {trio_b}, {trio_c}, {list_pat_n}))
"#,
            list_lit = list_lit,
            struct_val = struct_val,
            loop_n = loop_n,
            closure_add = closure_add,
            base = base,
            branch_pick = branch_pick,
            opt_pick_a = opt_pick_a,
            opt_pick_b = opt_pick_b,
            leaf_n = leaf_n,
            pair_a = pair_a,
            pair_b = pair_b,
            trio_a = trio_a,
            trio_b = trio_b,
            trio_c = trio_c,
            list_pat_n = list_pat_n,
        )
    }
}

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

fn write_program(src: &str, seed: u64) -> PathBuf {
    let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "olive_fuzz_{}_{}_{}.liv",
        std::process::id(),
        seed,
        n
    ));
    let mut f = std::fs::File::create(&path).expect("create fuzz program file");
    f.write_all(src.as_bytes()).expect("write fuzz program");
    path
}

fn run_jit(path: &std::path::Path) -> (String, i32) {
    let out = Command::new(pit_bin())
        .arg("run")
        .arg(path)
        .output()
        .expect("spawn pit run");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Builds to a unique binary and runs it directly, so the "Finished build"
/// banner never lands in captured stdout.
fn run_aot(path: &std::path::Path, out_bin: &std::path::Path) -> (String, i32) {
    let build = Command::new(pit_bin())
        .arg("build")
        .arg("--release")
        .arg("-o")
        .arg(out_bin)
        .arg(path)
        .output()
        .expect("spawn pit build");
    assert!(
        build.status.success(),
        "pit build --release failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(out_bin).output().expect("exec aot binary");
    std::fs::remove_file(out_bin).ok();
    (
        String::from_utf8_lossy(&run.stdout).into_owned(),
        run.status.code().unwrap_or(-1),
    )
}

fn check_seed(seed: u64) {
    let src = Gen::new(seed).program();
    if std::env::var("OLIVE_FUZZ_DUMP").is_ok() {
        eprintln!("=== seed {seed} ===\n{src}");
    }
    let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let path = write_program(&src, seed);
    let out_bin = std::env::temp_dir().join(format!(
        "olive_fuzz_bin_{}_{}_{}",
        std::process::id(),
        seed,
        n
    ));

    let (jit_out, jit_code) = run_jit(&path);
    let (aot_out, aot_code) = run_aot(&path, &out_bin);

    std::fs::remove_file(&path).ok();

    if jit_out != aot_out || jit_code != aot_code {
        let repro = std::env::temp_dir().join(format!("olive_fuzz_FAILURE_seed_{seed}.liv"));
        std::fs::write(&repro, &src).ok();
        panic!(
            "differential fuzz divergence at seed {seed}\n\
             JIT (debug): exit={jit_code} stdout={jit_out:?}\n\
             AOT (release): exit={aot_code} stdout={aot_out:?}\n\
             repro saved to {}",
            repro.display()
        );
    }
    assert_eq!(
        jit_code, 0,
        "seed {seed} program crashed (both pipelines agree, but nonzero exit): {jit_out}"
    );
}

/// Roadmap E10.1: `tests/reflex/*.liv` is the executable definition of "the
/// reflexes work" -- one probe per landed defect/feature, run through both
/// pipelines and diffed against a pinned golden. Each `.liv` needs exactly
/// one sibling: `.stdout` (must compile and run clean, output pinned),
/// `.error` (must fail to compile on both pipelines, stderr has the code),
/// or `.fault` (compiles, faults at runtime on both pipelines, stderr has
/// the code). An optional `.stdin` feeds captured input.
mod reflex_corpus {
    use super::{Command, UNIQUE, pit_bin};
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    fn corpus_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/reflex")
    }

    fn spawn_capture(cmd: &mut Command, stdin: Option<&[u8]>) -> (String, String, i32) {
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("spawn process");
        if let Some(bytes) = stdin {
            child
                .stdin
                .take()
                .expect("child stdin")
                .write_all(bytes)
                .expect("write stdin");
        } else {
            drop(child.stdin.take());
        }
        let out = child.wait_with_output().expect("wait for process");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    }

    fn run_jit(path: &Path, stdin: Option<&[u8]>) -> (String, String, i32) {
        spawn_capture(Command::new(pit_bin()).arg("run").arg(path), stdin)
    }

    fn build_aot(path: &Path, out_bin: &Path) -> (bool, String) {
        let build = Command::new(pit_bin())
            .arg("build")
            .arg("--release")
            .arg("-o")
            .arg(out_bin)
            .arg(path)
            .output()
            .expect("spawn pit build");
        (
            build.status.success(),
            String::from_utf8_lossy(&build.stderr).into_owned(),
        )
    }

    fn run_bin(out_bin: &Path, stdin: Option<&[u8]>) -> (String, String, i32) {
        let r = spawn_capture(&mut Command::new(out_bin), stdin);
        fs::remove_file(out_bin).ok();
        r
    }

    /// Deletes the built binary on scope exit, so an assert firing between the
    /// build and the end of a check does not leave it behind.
    struct TempBin(PathBuf);

    impl TempBin {
        fn new(stem: &str) -> Self {
            let n = UNIQUE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            TempBin(
                std::env::temp_dir()
                    .join(format!("olive_reflex_{}_{stem}_{n}", std::process::id())),
            )
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempBin {
        fn drop(&mut self) {
            fs::remove_file(&self.0).ok();
        }
    }

    fn read_stdin_fixture(liv: &Path) -> Option<Vec<u8>> {
        fs::read(liv.with_extension("stdin")).ok()
    }

    fn check_clean(liv: &Path, expected: &str) {
        let stdin = read_stdin_fixture(liv);
        let (jit_out, jit_err, jit_code) = run_jit(liv, stdin.as_deref());
        assert_eq!(
            jit_code,
            0,
            "{}: JIT exited {jit_code}: {jit_err}",
            liv.display()
        );
        assert_eq!(jit_out, expected, "{}: JIT stdout mismatch", liv.display());

        let out_bin_guard = TempBin::new(liv.file_stem().unwrap().to_str().unwrap());
        let out_bin = out_bin_guard.path();
        let (built, build_err) = build_aot(liv, out_bin);
        assert!(built, "{}: AOT build failed: {build_err}", liv.display());
        let (aot_out, aot_err, aot_code) = run_bin(out_bin, stdin.as_deref());
        assert_eq!(
            aot_code,
            0,
            "{}: AOT exited {aot_code}: {aot_err}",
            liv.display()
        );
        assert_eq!(aot_out, expected, "{}: AOT stdout mismatch", liv.display());
        assert_eq!(jit_out, aot_out, "{}: pipelines diverge", liv.display());
    }

    fn check_error(liv: &Path, code: &str) {
        let marker = format!("[{code}]");
        let (_, jit_err, jit_code) = run_jit(liv, None);
        assert_ne!(
            jit_code,
            0,
            "{}: JIT accepted a program it should reject",
            liv.display()
        );
        assert!(
            jit_err.contains(&marker),
            "{}: JIT stderr missing {marker}: {jit_err}",
            liv.display()
        );

        let out_bin_guard = TempBin::new(liv.file_stem().unwrap().to_str().unwrap());
        let out_bin = out_bin_guard.path();
        let (built, build_err) = build_aot(liv, out_bin);
        assert!(
            !built,
            "{}: AOT accepted a program it should reject",
            liv.display()
        );
        assert!(
            build_err.contains(&marker),
            "{}: AOT stderr missing {marker}: {build_err}",
            liv.display()
        );
    }

    fn check_fault(liv: &Path, code: &str) {
        let marker = format!("[{code}]");
        let stdin = read_stdin_fixture(liv);
        let (jit_out, jit_err, jit_code) = run_jit(liv, stdin.as_deref());
        assert_ne!(jit_code, 0, "{}: JIT should have faulted", liv.display());
        assert!(
            jit_err.contains(&marker),
            "{}: JIT stderr missing {marker}: {jit_err}",
            liv.display()
        );

        let out_bin_guard = TempBin::new(liv.file_stem().unwrap().to_str().unwrap());
        let out_bin = out_bin_guard.path();
        let (built, build_err) = build_aot(liv, out_bin);
        assert!(built, "{}: AOT build failed: {build_err}", liv.display());
        let (aot_out, aot_err, aot_code) = run_bin(out_bin, stdin.as_deref());
        assert_ne!(aot_code, 0, "{}: AOT should have faulted", liv.display());
        assert!(
            aot_err.contains(&marker),
            "{}: AOT stderr missing {marker}: {aot_err}",
            liv.display()
        );
        assert_eq!(
            jit_out,
            aot_out,
            "{}: pipelines diverge before fault",
            liv.display()
        );
    }

    /// One test per corpus file, so a single bad probe fails by name instead
    /// of sinking the whole lane.
    #[test]
    fn corpus_is_conformant() {
        if cfg!(all(target_os = "windows", target_env = "msvc")) {
            return;
        }
        let dir = corpus_dir();
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("liv"))
            .collect();
        entries.sort();
        assert!(!entries.is_empty(), "tests/reflex is empty");

        let mut failures = Vec::new();
        for liv in &entries {
            let stdout_p = liv.with_extension("stdout");
            let error_p = liv.with_extension("error");
            let fault_p = liv.with_extension("fault");
            let present = [stdout_p.exists(), error_p.exists(), fault_p.exists()];
            let count = present.iter().filter(|b| **b).count();
            assert_eq!(
                count,
                1,
                "{}: needs exactly one of .stdout/.error/.fault, found {count}",
                liv.display()
            );

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if stdout_p.exists() {
                    check_clean(liv, &fs::read_to_string(&stdout_p).unwrap());
                } else if error_p.exists() {
                    check_error(liv, fs::read_to_string(&error_p).unwrap().trim());
                } else {
                    check_fault(liv, fs::read_to_string(&fault_p).unwrap().trim());
                }
            }));
            if let Err(e) = result {
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "panic".to_string());
                failures.push(format!("{}: {msg}", liv.display()));
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }
}

/// Single test, sequential: avoids cross-test contention on the AOT output path.
#[test]
fn differential_fuzz() {
    // AOT linking on Windows MSVC requires MinGW but cranelift emits COFF
    // objects while MinGW's ld expects a compatible format. The current
    // `cc`-based linker only works under MinGW-w64 (GNU target). Skip this
    // test on MSVC Windows to avoid `collect2.exe: ld returned 1 exit status`.
    if cfg!(all(target_os = "windows", target_env = "msvc")) {
        return;
    }
    for &seed in FIXED_SEEDS {
        check_seed(seed);
    }
    let base = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for i in 0..ROTATING_SEED_COUNT {
        check_seed(base.wrapping_mul(2654435761).wrapping_add(i));
    }
}
