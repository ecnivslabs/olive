use crate::diagnostics::explain::{self, Explanation};
use crate::semantic::suggest;
use std::io::IsTerminal;

const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[38;5;246m";
const RESET: &str = "\x1b[0m";

const WRAP: usize = 78;

/// Entry point for `pit explain <CODE>`. Prints the long-form explanation for a
/// diagnostic code, or, when the code is unknown, the nearest known code.
pub fn execute_explain(code: &str) {
    match explain::lookup(code) {
        Some(e) => print!("{}", render(e, use_color())),
        None => {
            let normalized = code.trim().to_ascii_uppercase();
            eprintln!("error: no diagnostic code `{normalized}`");
            if let Some(near) = suggest::closest(&normalized, explain::codes()) {
                eprintln!("help: did you mean `{near}`?");
            }
            eprintln!("help: codes look like `E0400` (errors) or `W0610` (warnings)");
            std::process::exit(1);
        }
    }
}

fn use_color() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

/// Renders an explanation as a self-contained block: coded headline, wrapped
/// summary, the wrong and fixed examples in reading order, then any notes.
fn render(e: &Explanation, color: bool) -> String {
    let (red, green, bold, dim, reset) = if color {
        (RED, GREEN, BOLD, DIM, RESET)
    } else {
        ("", "", "", "", "")
    };

    let mut out = String::new();
    out.push_str(&format!(
        "{red}{bold}[{}]{reset} {bold}{}{reset}\n\n",
        e.code, e.title
    ));

    for line in wrap(e.summary, WRAP) {
        out.push_str(&line);
        out.push('\n');
    }

    out.push_str(&format!("\n  {red}✗ this is rejected{reset}\n"));
    push_example(&mut out, e.wrong, dim, reset);
    out.push_str(&format!("\n  {green}✓ this works{reset}\n"));
    push_example(&mut out, e.fixed, dim, reset);

    if !e.notes.is_empty() {
        out.push('\n');
        for note in e.notes {
            for (i, line) in wrap(note, WRAP - 6).into_iter().enumerate() {
                if i == 0 {
                    out.push_str(&format!("{dim}note:{reset} {line}\n"));
                } else {
                    out.push_str(&format!("      {line}\n"));
                }
            }
        }
    }

    out
}

fn push_example(out: &mut String, code: &str, dim: &str, reset: &str) {
    for line in code.lines() {
        out.push_str(&format!("      {dim}│{reset} {line}\n"));
    }
}

/// Greedy word wrap. Never splits a word, so code-like tokens stay intact.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_known_code_plainly() {
        let e = explain::lookup("E0400").unwrap();
        let out = render(e, false);
        assert!(out.contains("[E0400]"));
        assert!(out.contains("mismatched types"));
        assert!(out.contains("✗ this is rejected"));
        assert!(out.contains("✓ this works"));
        assert!(out.contains("note:"));
        assert!(!out.contains("\x1b["));
    }

    #[test]
    fn every_explanation_renders() {
        for e in explain::all() {
            let out = render(e, false);
            assert!(out.contains(e.code));
            assert!(out.contains(e.title));
        }
    }

    #[test]
    fn wrap_respects_width_and_keeps_words() {
        let lines = wrap("the quick brown fox jumps", 9);
        assert!(lines.iter().all(|l| l.len() <= 9 || !l.contains(' ')));
        assert_eq!(lines.join(" "), "the quick brown fox jumps");
    }
}
