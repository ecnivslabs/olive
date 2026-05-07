//! Runtime panic rendering. A native fault prints the failing Olive source
//! location, the line itself with a caret under the offending column, and the
//! message, so a crash reads like a compile diagnostic rather than a dump of
//! mangled host-language frames. Set `OLIVE_BACKTRACE=1` to additionally print
//! the host backtrace when debugging the runtime itself.

use crate::{olive_str_from_ptr, run_exit_hooks};
use std::io::{IsTerminal, Write};

const RED: &str = "\x1b[31m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[38;5;246m";
const RESET: &str = "\x1b[0m";

/// A parsed `file:line:col` (or `line:col`) source location.
struct Location {
    file: Option<String>,
    line: usize,
    col: usize,
}

thread_local! {
    /// Tagged Olive string pointer to the `file:line:col` of the most recent
    /// fault-prone operation (an explicit `panic`, an `unwrap`). Recorded just
    /// before the call so an abort with no location of its own can still point
    /// at the caller. Only the pointer is stored, so a successful `unwrap` costs
    /// a single thread-local write and never allocates.
    static FAULT_LOC: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
}

/// Records the Olive source location about to execute a fault-prone operation.
/// Emitted by the MIR builder immediately before `panic`/`unwrap`/`unwrap_err`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_set_fault_loc(ptr: i64) {
    FAULT_LOC.with(|c| c.set(ptr));
}

/// Splits a location string from the right so Windows drive prefixes such as
/// `C:\path\x.liv` survive intact. Returns `None` unless both a line and a
/// column parse as integers.
fn parse_loc(loc: &str) -> Option<Location> {
    let mut parts = loc.rsplitn(3, ':');
    let col: usize = parts.next()?.trim().parse().ok()?;
    let line: usize = parts.next()?.trim().parse().ok()?;
    let file = parts.next().map(|s| s.to_string());
    Some(Location { file, line, col })
}

fn use_color() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal()
}

/// Renders the source line at `loc` with a caret under the offending column,
/// when the file is still readable on disk.
fn render_source(out: &mut impl Write, loc: &Location, color: bool) {
    let Some(file) = &loc.file else { return };
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    let Some(src_line) = text.lines().nth(loc.line.saturating_sub(1)) else {
        return;
    };

    let (dim, reset, red) = if color {
        (DIM, RESET, RED)
    } else {
        ("", "", "")
    };
    let gutter = loc.line.to_string();
    let pad = " ".repeat(gutter.len());
    let chars: Vec<char> = src_line.chars().collect();
    let caret_col = loc.col.saturating_sub(1).min(chars.len());
    let lead: usize = chars[..caret_col]
        .iter()
        .map(|c| if *c == '\t' { 4 } else { 1 })
        .sum();
    let width = underline_width(&chars, caret_col);

    let _ = writeln!(out, "{dim}{pad} │{reset}");
    let _ = writeln!(out, "{dim}{gutter} │{reset} {src_line}");
    let _ = writeln!(
        out,
        "{dim}{pad} │{reset} {}{red}{}{reset}",
        " ".repeat(lead),
        "^".repeat(width)
    );
}

/// Width of the underline to draw under the offending column. A fault location
/// is a single `line:col`, so the token boundary is recovered from the source
/// text: a name or number is underlined whole, anything else gets a single
/// caret. This keeps a runtime panic reading like a compile diagnostic instead
/// of pointing a lone `^` at the start of a multi-character token.
fn underline_width(chars: &[char], start: usize) -> usize {
    let Some(&first) = chars.get(start) else {
        return 1;
    };
    if !(first.is_alphanumeric() || first == '_') {
        return 1;
    }
    chars[start..]
        .iter()
        .take_while(|c| c.is_alphanumeric() || **c == '_')
        .count()
        .max(1)
}

/// Prints the diagnostic, runs exit hooks, and terminates the process. Never
/// returns; the `!` type lets callers that must yield a value defer to it.
pub fn abort(msg: &str, loc: Option<&str>) -> ! {
    run_exit_hooks();

    let color = use_color();
    let mut out = std::io::stderr().lock();
    let (red, bold, dim, reset) = if color {
        (RED, BOLD, DIM, RESET)
    } else {
        ("", "", "", "")
    };

    let fb_ptr = FAULT_LOC.with(|c| c.get());
    let fallback = (fb_ptr != 0).then(|| olive_str_from_ptr(fb_ptr));
    let parsed = loc.or(fallback.as_deref()).and_then(parse_loc);
    let _ = writeln!(out, "{red}{bold}panic{reset}{bold}: {msg}{reset}");
    if let Some(parsed) = &parsed {
        let where_ = match &parsed.file {
            Some(f) => format!("{f}:{}:{}", parsed.line, parsed.col),
            None => format!("{}:{}", parsed.line, parsed.col),
        };
        let _ = writeln!(out, "{dim}  ╭─[{reset} {where_} {dim}]{reset}");
        render_source(&mut out, parsed, color);
    }

    if std::env::var_os("OLIVE_BACKTRACE").is_some() {
        let _ = writeln!(out, "{dim}backtrace:{reset}");
        let _ = writeln!(out, "{}", std::backtrace::Backtrace::force_capture());
    }
    let _ = out.flush();

    std::process::exit(1);
}

/// Raised when an index is outside `0..len`. Reports the length and the
/// offending index, mirroring how a compile diagnostic would read.
#[unsafe(no_mangle)]
pub extern "C" fn olive_bounds_fail(index: i64, len: i64, loc: i64) -> i64 {
    let loc = (loc != 0).then(|| olive_str_from_ptr(loc));
    let msg = if index < 0 {
        format!(
            "index out of bounds: the length is {len} but the index is {index}; negative indices are not supported"
        )
    } else {
        format!("index out of bounds: the length is {len} but the index is {index}")
    };
    abort(&msg, loc.as_deref())
}

/// Raised when indexing a value that is null (an uninitialised or `None`
/// container).
#[unsafe(no_mangle)]
pub extern "C" fn olive_nil_index_fail(loc: i64) -> i64 {
    let loc = (loc != 0).then(|| olive_str_from_ptr(loc));
    abort("cannot index into a null value", loc.as_deref())
}

/// Raised when the divisor of an integer `/` or `%` is zero. Hardware would
/// otherwise trap with no context; this reports the operation that failed and
/// points at the source.
#[unsafe(no_mangle)]
pub extern "C" fn olive_div_zero_fail(is_mod: i64, loc: i64) -> i64 {
    let loc = (loc != 0).then(|| olive_str_from_ptr(loc));
    let msg = if is_mod != 0 {
        "remainder by zero: the right-hand side of `%` is 0"
    } else {
        "divide by zero: the right-hand side of `/` is 0"
    };
    abort(msg, loc.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_file_line_col() {
        let l = parse_loc("src/main.liv:12:5").unwrap();
        assert_eq!(l.file.as_deref(), Some("src/main.liv"));
        assert_eq!(l.line, 12);
        assert_eq!(l.col, 5);
    }

    #[test]
    fn parses_line_col_only() {
        let l = parse_loc("12:5").unwrap();
        assert!(l.file.is_none());
        assert_eq!(l.line, 12);
        assert_eq!(l.col, 5);
    }

    #[test]
    fn keeps_windows_drive_prefix() {
        let l = parse_loc("C:\\proj\\main.liv:3:9").unwrap();
        assert_eq!(l.file.as_deref(), Some("C:\\proj\\main.liv"));
        assert_eq!(l.line, 3);
        assert_eq!(l.col, 9);
    }

    #[test]
    fn rejects_non_numeric() {
        assert!(parse_loc("just a message").is_none());
        assert!(parse_loc("file:notaline:col").is_none());
    }

    #[test]
    fn renders_caret_under_column() {
        let dir = std::env::temp_dir().join(format!("olive_panic_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("snippet.liv");
        std::fs::write(&path, "let x = 1\nprint(arr[5])\n").unwrap();
        let loc = Location {
            file: Some(path.to_string_lossy().into_owned()),
            line: 2,
            col: 11,
        };
        let mut buf = Vec::new();
        render_source(&mut buf, &loc, false);
        let rendered = String::from_utf8(buf).unwrap();
        assert!(rendered.contains("print(arr[5])"));
        assert!(rendered.contains('^'));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn underlines_whole_identifier_token() {
        let dir = std::env::temp_dir().join(format!("olive_underline_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("snippet.liv");
        std::fs::write(&path, "let value = compute(total)\n").unwrap();
        let loc = Location {
            file: Some(path.to_string_lossy().into_owned()),
            line: 1,
            col: 21,
        };
        let mut buf = Vec::new();
        render_source(&mut buf, &loc, false);
        let rendered = String::from_utf8(buf).unwrap();
        assert!(rendered.contains("^^^^^"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn underline_width_spans_name() {
        let chars: Vec<char> = "arr[idx]".chars().collect();
        assert_eq!(underline_width(&chars, 0), 3);
        assert_eq!(underline_width(&chars, 3), 1);
        assert_eq!(underline_width(&chars, 4), 3);
    }

    #[test]
    fn fault_loc_stores_and_clears() {
        let p = crate::olive_str_internal("/tmp/x.liv:9:3");
        olive_set_fault_loc(p);
        FAULT_LOC.with(|c| assert_eq!(c.get(), p));
        olive_set_fault_loc(0);
        FAULT_LOC.with(|c| assert_eq!(c.get(), 0));
    }

    #[test]
    fn render_source_missing_file_is_noop() {
        let loc = Location {
            file: Some("/no/such/olive/file.liv".into()),
            line: 1,
            col: 1,
        };
        let mut buf = Vec::new();
        render_source(&mut buf, &loc, false);
        assert!(buf.is_empty());
    }
}
