//! Fenced-code-block extraction shared by two consumers: the E10.3 test that
//! compiles every block in `docs/*.md` (`tests/doc_blocks.rs`), and E13.4's
//! doc-comment blocks (`///` text is markdown, so the same fence syntax
//! applies verbatim -- see `tooling::doc_comments`). Only the pure
//! extraction/classification lives here; the E10.3 test's process-spawning
//! (`pit run`/`pit build` on the extracted code) stays test-side, since it
//! needs the built `pit` binary's path, which only a `cargo test` binary has
//! via `env!("CARGO_BIN_EXE_pit")`.

/// One fenced block: its language tag and raw contents.
pub struct Block {
    pub lang: String,
    pub code: String,
}

pub fn extract_blocks(md: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines = md.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(lang) = line.strip_prefix("```") else {
            continue;
        };
        let lang = lang.trim().to_string();
        let mut code = String::new();
        for body_line in lines.by_ref() {
            if body_line.trim_start() == "```" {
                break;
            }
            code.push_str(body_line);
            code.push('\n');
        }
        blocks.push(Block { lang, code });
    }
    blocks
}

/// A standalone block genuinely can't know a name, trait, or enum only
/// introduced in the surrounding prose or an earlier block ("assume `s:
/// str`", `impl Drawable for Circle` shown after `trait Drawable` was
/// already defined higher up in the file); every other diagnostic code (a
/// real type error, a syntax error, a linker failure, an ICE) means the
/// example itself is wrong, not merely incomplete.
pub fn is_elided_context_error(code: &str, stderr: &str) -> bool {
    if stderr.contains("panicked") {
        return false;
    }
    if stderr.contains("[E0001]") || stderr.contains("[E0416]") || stderr.contains("[E0419]") {
        return true;
    }
    // `impl X:` / `impl T for X:` with no local `struct X:`/`enum X:` --
    // whatever the resulting diagnostic code, `X` is a type shown earlier.
    for line in code.lines() {
        let l = line.trim_start();
        let Some(rest) = l.strip_prefix("impl ") else {
            continue;
        };
        let name = rest.rsplit(" for ").next().unwrap_or(rest);
        let name = name.split([':', '[']).next().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let defined = code.contains(&format!("struct {name}"))
            || code.contains(&format!("struct {name}["))
            || code.contains(&format!("enum {name}"))
            || code.contains(&format!("trait {name}"));
        if !defined {
            return true;
        }
    }
    false
}
