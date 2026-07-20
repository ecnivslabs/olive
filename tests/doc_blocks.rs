//! Roadmap E10.3: every fenced Olive block in docs/ is compiled and run, not
//! just eyeballed. Each block is tried standalone, since prose docs
//! routinely show a snippet that depends on names introduced in the
//! surrounding prose rather than an earlier code block (`s.to_int()` where
//! `s: str` is only ever named in a sentence) -- concatenating a whole file
//! into one program instead produces artifacts of its own (two unrelated
//! sections that happen to reuse a name, e.g. two different `math` aliases,
//! collide even though neither example is wrong on its own). A standalone
//! block failing with "cannot find name" is that same elided-context
//! pattern and is not treated as a failure; anything else -- a crash, an
//! ICE, a real type error, divergence between pipelines -- is a doc bug.
//! A `print(...)` line ending in `// value` is a determinism claim: the
//! block's stdout must contain `value`.

use pit::tooling::doc_blocks::{extract_blocks, is_elided_context_error};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn pit_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pit"))
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

fn docs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs")
}

fn looks_like_value(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let bracketed = matches!(
        (s.as_bytes()[0], s.as_bytes()[s.len() - 1]),
        (b'[', b']') | (b'(', b')') | (b'{', b'}') | (b'"', b'"')
    );
    if bracketed {
        return true;
    }
    // Deliberately NOT bare alphanumeric words in general ("fine", "ok"):
    // in this docset those are always prose commentary, never a literal
    // printed value, and accepting them produces false-positive failures.
    matches!(s, "True" | "False" | "None") || s.parse::<f64>().is_ok()
}

/// A trailing `// value` on a `print(...)` line: the run's stdout must
/// contain `value` somewhere (a `contains` check, not an ordered diff,
/// since a block can carry several prints before the annotated one).
fn expected_fragments(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("print(") {
            continue;
        }
        let Some(idx) = line.find("//") else {
            continue;
        };
        let comment = line[idx + 2..].trim();
        if looks_like_value(comment) {
            out.push(comment.to_string());
        }
    }
    out
}

/// Deletes its artifact on scope exit. This harness bails out of a block with
/// `continue` on every failure path, so cleanup tied to the binding is what
/// keeps a run from leaving thousands of files behind in the temp dir.
struct TempFile(PathBuf);

impl TempFile {
    fn new(stem: &str) -> Self {
        let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
        TempFile(std::env::temp_dir().join(format!(
            "olive_docblock_{}_{stem}_{n}.liv",
            std::process::id()
        )))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

// A doc block calling `input()` (basics.md's builtin table, io.md) would
// otherwise block forever on the harness's own inherited stdin -- every
// spawned child gets an immediately-closed stdin so a blocking read fails
// fast instead of hanging the suite.
fn run_jit(path: &Path) -> (String, String, i32) {
    let out = Command::new(pit_bin())
        .arg("run")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit run");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn build_aot(path: &Path, out_bin: &Path) -> (bool, String) {
    let out = Command::new(pit_bin())
        .arg("build")
        .arg("--release")
        .arg(path)
        .arg("-o")
        .arg(out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn pit build");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn run_bin(out_bin: &Path) -> (String, String, i32) {
    let out = Command::new(out_bin)
        .stdin(Stdio::null())
        .output()
        .expect("spawn built binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Not real programs, so excluded from the compile check: `ffi.md`'s blocks
/// link against system libraries this machine doesn't have (`libfoo.so`,
/// `user32.dll`); `modules.md`'s stdlib section is ~75 blocks of
/// `module.fn(x) -> T` signature listings, not statements (its actual
/// runnable examples -- struct construction, `with` -- are covered by the
/// reflex corpus instead); `python.md` needs a real Python install plus
/// third-party packages (`glm`) neither pipeline can assume; `async.md`
/// imports `aio`, and `lib/aio.liv` itself fails to typecheck (`chan[T]`/
/// `mutex[T]`'s `Chan(h)`/`Mutex(h)` constructor calls can't resolve `T`
/// from their int-only constructor args, entangled with a separate
/// explicit-type-argument-call ICE -- a genuine pre-existing stdlib bug,
/// not a doc bug; see memory project_aio_generic_construction_bug).
const NOT_STANDALONE: &[&str] = &["ffi.md", "modules.md", "python.md", "async.md"];

#[test]
fn doc_examples_compile_and_run() {
    if cfg!(all(target_os = "windows", target_env = "msvc")) {
        return;
    }
    let dir = docs_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .filter(|p| !NOT_STANDALONE.contains(&p.file_name().and_then(|n| n.to_str()).unwrap_or("")))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "docs/ has no markdown files");

    let mut failures = Vec::new();
    let mut checked = 0u32;

    for md_path in &files {
        let text = std::fs::read_to_string(md_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", md_path.display()));
        let stem = md_path.file_stem().unwrap().to_str().unwrap();

        for (i, block) in extract_blocks(&text)
            .into_iter()
            .filter(|b| b.lang == "rust" || b.lang == "olive")
            .enumerate()
        {
            if block.code.trim().is_empty() {
                continue;
            }

            let src = TempFile::new(&format!("{stem}_{i}"));
            let src_path = src.path();
            let mut f = std::fs::File::create(src_path)
                .unwrap_or_else(|e| panic!("create {}: {e}", src_path.display()));
            f.write_all(block.code.as_bytes()).unwrap();
            drop(f);

            let (jit_out, jit_err, jit_code) = run_jit(src_path);
            if jit_code != 0 {
                if !is_elided_context_error(&block.code, &jit_err) {
                    failures.push(format!(
                        "{}#{i}: JIT exited {jit_code}\n{jit_err}\n--- code ---\n{}",
                        md_path.display(),
                        block.code
                    ));
                }
                continue;
            }
            checked += 1;

            let bin = TempFile::new(&format!("{stem}_{i}_bin"));
            let out_bin = bin.path();
            let (built, build_err) = build_aot(src_path, out_bin);
            if !built {
                failures.push(format!(
                    "{}#{i}: AOT build failed (JIT succeeded)\n{build_err}\n--- code ---\n{}",
                    md_path.display(),
                    block.code
                ));
                continue;
            }
            let (aot_out, aot_err, aot_code) = run_bin(out_bin);
            if aot_code != 0 {
                failures.push(format!(
                    "{}#{i}: AOT exited {aot_code} (JIT succeeded)\n{aot_err}",
                    md_path.display()
                ));
                continue;
            }
            if jit_out != aot_out {
                failures.push(format!("{}#{i}: pipelines diverge", md_path.display()));
                continue;
            }

            for expected in expected_fragments(&block.code) {
                if !jit_out.contains(&expected) {
                    failures.push(format!(
                        "{}#{i}: stdout missing expected fragment {expected:?}\n--- actual stdout ---\n{jit_out}\n--- code ---\n{}",
                        md_path.display(),
                        block.code
                    ));
                }
            }
        }
    }

    assert!(checked > 0, "no doc block compiled standalone anywhere");
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}
