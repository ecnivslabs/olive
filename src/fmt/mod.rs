mod comments;
mod diff;
mod doc;
mod lower;
mod lower_decl;
mod lower_expr;
mod syntax;

#[cfg(test)]
mod tests;

use crate::lexer::Lexer;
use crate::parser::Parser;
use doc::render;
use lower::Lowerer;
use std::path::Path;

/// Default line width before groups break. Matches rustfmt's convention.
pub const DEFAULT_WIDTH: usize = 100;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    /// Rewrite files in place.
    Write,
    /// Exit non-zero if any file is not already formatted; write nothing.
    Check,
    /// Print a unified diff of the proposed changes; write nothing.
    Diff,
    /// Format stdin to stdout.
    Stdin,
}

#[derive(Clone, Copy)]
pub struct Options {
    pub max_width: usize,
    pub mode: Mode,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_width: DEFAULT_WIDTH,
            mode: Mode::Write,
        }
    }
}

/// Format Olive source, returning the formatted text. Errors carry a `line:col`
/// prefixed message and leave the caller free to skip the file untouched.
pub fn format_source(source: &str, max_width: usize) -> Result<String, String> {
    if source.trim().is_empty() {
        return Ok(String::new());
    }
    let mut lexer = Lexer::new(source, 0);
    let tokens = lexer
        .tokenise()
        .map_err(|e| format!("{}:{}: {}", e.line, e.col, e.message))?;
    let comments = lexer.comments().to_vec();
    let program = Parser::new(tokens)
        .parse_program()
        .map_err(|e| e.to_string())?;

    let chars: Vec<char> = source.chars().collect();
    let mut lowerer = Lowerer::new(&chars, &comments);
    let rendered = render(&lowerer.program(&program), max_width);
    Ok(normalize(&rendered))
}

/// Strip trailing whitespace from every line and guarantee a single final newline.
/// Blank lines produced by the layout carry indentation that must not survive.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 1);
    for line in s.lines() {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// CLI entry. Dispatches across the requested mode and returns the process exit code.
pub fn execute(file: Option<&String>, opts: Options) -> i32 {
    match opts.mode {
        Mode::Stdin => run_stdin(opts),
        _ => {
            let mut changed = false;
            let mut failed = false;
            match file {
                Some(f) => {
                    let path = Path::new(f);
                    if path.is_dir() {
                        walk(path, opts, &mut changed, &mut failed);
                    } else {
                        process_file(path, opts, &mut changed, &mut failed);
                    }
                }
                None => walk(Path::new("."), opts, &mut changed, &mut failed),
            }
            if failed || (opts.mode == Mode::Check && changed) {
                1
            } else {
                0
            }
        }
    }
}

fn run_stdin(opts: Options) -> i32 {
    use std::io::Read;
    let mut source = String::new();
    if std::io::stdin().read_to_string(&mut source).is_err() {
        eprintln!("error reading stdin");
        return 1;
    }
    match format_source(&source, opts.max_width) {
        Ok(formatted) => {
            print!("{formatted}");
            0
        }
        Err(e) => {
            eprintln!("error formatting stdin: {e}");
            1
        }
    }
}

fn walk(path: &Path, opts: Options, changed: &mut bool, failed: &mut bool) {
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            paths.sort();
            for p in paths {
                walk(&p, opts, changed, failed);
            }
        }
    } else if path.extension().is_some_and(|ext| ext == "liv") {
        process_file(path, opts, changed, failed);
    }
}

fn process_file(path: &Path, opts: Options, changed: &mut bool, failed: &mut bool) {
    let display = path.display().to_string();
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error reading {display}: {e}");
            *failed = true;
            return;
        }
    };
    let formatted = match format_source(&source, opts.max_width) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error formatting {display}: {e}");
            *failed = true;
            return;
        }
    };
    if formatted == source {
        return;
    }
    *changed = true;
    match opts.mode {
        Mode::Write => {
            if let Err(e) = std::fs::write(path, &formatted) {
                eprintln!("error writing {display}: {e}");
                *failed = true;
                return;
            }
            println!("\x1b[1;32mFormatted\x1b[0m {display}");
        }
        Mode::Check => println!("\x1b[1;33mwould reformat\x1b[0m {display}"),
        Mode::Diff => print!("{}", diff::unified(&source, &formatted, &display)),
        Mode::Stdin => unreachable!(),
    }
}
