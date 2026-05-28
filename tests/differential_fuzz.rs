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

fn main():
    print(moves_and_borrows())
    print(struct_escape())
    print(loop_reassign({loop_n}))
    print(any_mixing())
    print(nested_closure())
    print(branch_pick({branch_pick}))
    print(opt_items_len(maybe_box({opt_pick_a})))
    print(guard_len(maybe_list({opt_pick_b})))
"#,
            list_lit = list_lit,
            struct_val = struct_val,
            loop_n = loop_n,
            closure_add = closure_add,
            base = base,
            branch_pick = branch_pick,
            opt_pick_a = opt_pick_a,
            opt_pick_b = opt_pick_b,
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

/// Single test, sequential: avoids cross-test contention on the AOT output path.
#[test]
fn differential_fuzz() {
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
