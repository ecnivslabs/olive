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
const HELP: &str = "\x1b[38;5;115m";
const RESET: &str = "\x1b[0m";

/// A runtime fault: a stable code plus optional `help` and `note`, so a crash
/// reads like an `[E07xx]` diagnostic.
pub struct Fault {
    pub code: &'static str,
    pub help: Option<&'static str>,
    pub note: Option<&'static str>,
}

const PANIC: Fault = Fault {
    code: "E0700",
    help: None,
    note: None,
};
const BOUNDS: Fault = Fault {
    code: "E0701",
    help: Some("use a valid index, or guard the access with a length check"),
    note: None,
};
const NIL_INDEX: Fault = Fault {
    code: "E0702",
    help: Some("initialise the value, or check it against `None` before indexing"),
    note: None,
};
const DIV_ZERO: Fault = Fault {
    code: "E0703",
    help: Some("guard the divisor so it is non-zero before dividing"),
    note: None,
};
const UNWRAP: Fault = Fault {
    code: "E0704",
    help: Some("handle the error case with `?`, `try`, or a match instead of unwrapping"),
    note: None,
};
const PY_UNCAUGHT: Fault = Fault {
    code: "E0705",
    help: Some("wrap the call in `try` or guard the inputs so the exception cannot arise"),
    note: Some("the exception propagated out of Python without being caught"),
};
const PY_COERCE: Fault = Fault {
    code: "E0706",
    help: Some("convert the Python value to the target type or read it into a PyObject first"),
    note: Some("Python value cannot be converted to the required native type"),
};
const STALE_REF: Fault = Fault {
    code: "E0707",
    help: Some(
        "keep the owner alive while the value is in use, or copy the value instead of borrowing it",
    ),
    note: Some("the value this name refers to was freed when its owner went away"),
};
const ZERO_STEP: Fault = Fault {
    code: "E0709",
    help: Some("guard the step so it is non-zero before looping"),
    note: Some("a `by 0` range step would never advance and loop forever"),
};
const STARRED_UNPACK: Fault = Fault {
    code: "E0710",
    help: Some("check the list's length before destructuring it, or use a plain name instead"),
    note: Some("a starred target (`a, *rest = xs`) needs at least as many elements as plain names"),
};
const DICT_KEY: Fault = Fault {
    code: "E0711",
    help: Some(
        "check the key with `in` first, or use `.get(key, default)` for a non-faulting lookup",
    ),
    note: None,
};
const ASSERT_FAILED: Fault = Fault {
    code: "E0712",
    help: Some("check the operands, or fix the logic that produces them"),
    note: None,
};
const OVERFLOW: Fault = Fault {
    code: "E0713",
    help: Some("use a wider type, or guard the operands so the result stays in range"),
    note: Some("i64 arithmetic that would silently wrap instead faults here"),
};

/// Aborts when a Python value cannot be converted to the required native scalar.
pub fn abort_py_coerce(msg: &str) -> ! {
    abort_with(&PY_COERCE, msg, None)
}

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

/// The generic uncoded abort (a bare `panic`). Never returns.
pub fn abort(msg: &str, loc: Option<&str>) -> ! {
    abort_with(&PANIC, msg, loc)
}

/// Renders a fault into `out`: coded headline, source line with caret, help and
/// note. Split from [`abort_with`] so it can be tested without exiting.
fn render_fault(out: &mut impl Write, fault: &Fault, msg: &str, loc: Option<&str>, color: bool) {
    let (red, bold, dim, help_c, reset) = if color {
        (RED, BOLD, DIM, HELP, RESET)
    } else {
        ("", "", "", "", "")
    };

    let parsed = loc.and_then(parse_loc);
    let _ = writeln!(
        out,
        "{red}{bold}[{}] panic{reset}{bold}: {msg}{reset}",
        fault.code
    );
    if let Some(parsed) = &parsed {
        let where_ = match &parsed.file {
            Some(f) => format!("{f}:{}:{}", parsed.line, parsed.col),
            None => format!("{}:{}", parsed.line, parsed.col),
        };
        let _ = writeln!(out, "{dim}  ╭─[{reset} {where_} {dim}]{reset}");
        render_source(out, parsed, color);
        crate::shadow_stack::render(out, dim, reset, color);
        if let Some(help) = fault.help {
            let _ = writeln!(out, "{dim}  │{reset}");
            let _ = writeln!(out, "{dim}  │{reset} {help_c}help{reset}: {help}");
        }
        if let Some(note) = fault.note {
            let _ = writeln!(out, "{dim}  │{reset}");
            let _ = writeln!(out, "{dim}  │{reset} {help_c}note{reset}: {note}");
        }
        let _ = writeln!(out, "{dim}──╯{reset}");
    } else {
        if let Some(help) = fault.help {
            let _ = writeln!(out, "{help_c}help{reset}: {help}");
        }
        if let Some(note) = fault.note {
            let _ = writeln!(out, "{help_c}note{reset}: {note}");
        }
    }
}

/// Renders `fault` and terminates the process. Never returns.
pub fn abort_with(fault: &Fault, msg: &str, loc: Option<&str>) -> ! {
    run_exit_hooks();

    let color = use_color();
    let mut out = std::io::stderr().lock();

    let fb_ptr = FAULT_LOC.with(|c| c.get());
    let fallback = (fb_ptr != 0).then(|| olive_str_from_ptr(fb_ptr));
    let where_ = loc.or(fallback.as_deref());
    render_fault(&mut out, fault, msg, where_, color);

    let dim = if color { DIM } else { "" };
    let reset = if color { RESET } else { "" };
    if std::env::var_os("OLIVE_BACKTRACE").is_some() {
        let _ = writeln!(out, "{dim}backtrace:{reset}");
        let _ = writeln!(out, "{}", std::backtrace::Backtrace::force_capture());
    }
    let _ = out.flush();

    std::process::exit(1);
}

/// Aborts an `unwrap`/`unwrap_err` on the wrong variant.
pub fn abort_unwrap(msg: &str) -> ! {
    abort_with(&UNWRAP, msg, None)
}

/// Aborts on a Python exception that crossed the FFI boundary uncaught.
pub fn abort_python(msg: &str, loc: Option<&str>) -> ! {
    abort_with(&PY_UNCAUGHT, msg, loc)
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
    abort_with(&BOUNDS, &msg, loc.as_deref())
}

/// Raised when indexing a value that is null (an uninitialised or `None`
/// container).
#[unsafe(no_mangle)]
pub extern "C" fn olive_nil_index_fail(loc: i64) -> i64 {
    let loc = (loc != 0).then(|| olive_str_from_ptr(loc));
    abort_with(&NIL_INDEX, "cannot index into a null value", loc.as_deref())
}

/// Raised when a generation check finds a borrowed value whose owner already
/// freed it. Fires before the stale pointer is dereferenced, so the program
/// dies with a source caret instead of corrupting the heap.
#[unsafe(no_mangle)]
pub extern "C" fn olive_stale_ref_fail(name: i64, loc: i64) -> i64 {
    let loc = (loc != 0).then(|| olive_str_from_ptr(loc));
    let msg = if name != 0 {
        format!(
            "stale reference: `{}` points at a value that was already freed",
            olive_str_from_ptr(name)
        )
    } else {
        "stale reference: this value was already freed".to_string()
    };
    abort_with(&STALE_REF, &msg, loc.as_deref())
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
    abort_with(&DIV_ZERO, msg, loc.as_deref())
}

/// Checks a range's `by` step: returns it unchanged when non-zero, aborts
/// otherwise. A literal `by 0` is already a compile error (W3); this covers
/// a step computed at runtime.
#[unsafe(no_mangle)]
pub extern "C" fn olive_check_nonzero_step(step: i64, loc: i64) -> i64 {
    if step == 0 {
        let loc = (loc != 0).then(|| olive_str_from_ptr(loc));
        abort_with(
            &ZERO_STEP,
            "range step is 0: this range would loop forever",
            loc.as_deref(),
        );
    }
    step
}

/// Raised when a starred destructure's source has fewer elements than the
/// plain names on the left require (`a, b, *rest = xs` needs `len(xs) >= 2`).
#[unsafe(no_mangle)]
pub extern "C" fn olive_starred_unpack_fail(got: i64, need: i64, loc: i64) -> i64 {
    let loc = (loc != 0).then(|| olive_str_from_ptr(loc));
    let msg = format!("not enough values to unpack: needed at least {need}, got {got}");
    abort_with(&STARRED_UNPACK, &msg, loc.as_deref())
}

/// Separate code from E0701: a dict key isn't a range, just present/absent.
#[unsafe(no_mangle)]
pub extern "C" fn olive_key_fail(key: i64, loc: i64) -> i64 {
    let loc = (loc != 0).then(|| olive_str_from_ptr(loc));
    let msg = match crate::classify_key(key) {
        crate::KeyClass::Str(s) => format!("key not found: \"{s}\""),
        crate::KeyClass::Scalar(_, v) => format!("key not found: {v}"),
        crate::KeyClass::Raw(_) => "key not found".to_string(),
    };
    abort_with(&DICT_KEY, &msg, loc.as_deref())
}

/// Raised when an `assert` condition is false. `msg` is fully built by the
/// MIR builder (`assertion failed`, plus `left`/`right` operand values for a
/// top-level comparison, plus the user's own message if given); this only
/// renders and aborts.
#[unsafe(no_mangle)]
pub extern "C" fn olive_assert_fail(msg: i64, loc: i64) -> i64 {
    let loc = (loc != 0).then(|| olive_str_from_ptr(loc));
    let msg = olive_str_from_ptr(msg);
    abort_with(&ASSERT_FAILED, &msg, loc.as_deref())
}

/// Raised when `i64` arithmetic overflows: `+`/`-`/`*` on signed or unsigned
/// operands (`kind` 0-5), or the two `i64::MIN / -1` / `i64::MIN % -1`
/// corners (`kind` 6/7), which trap in hardware rather than wrapping so they
/// get their own kinds despite not going through the checked-op codegen path.
#[unsafe(no_mangle)]
pub extern "C" fn olive_overflow_fail(kind: i64, lhs: i64, rhs: i64, loc: i64) -> i64 {
    let loc = (loc != 0).then(|| olive_str_from_ptr(loc));
    let msg = match kind {
        0 => format!("integer overflow: {lhs} + {rhs} does not fit in i64"),
        1 => format!("integer overflow: {lhs} - {rhs} does not fit in i64"),
        2 => format!("integer overflow: {lhs} * {rhs} does not fit in i64"),
        3 => format!(
            "integer overflow: {} + {} does not fit in u64",
            lhs as u64, rhs as u64
        ),
        4 => format!(
            "integer overflow: {} - {} does not fit in u64",
            lhs as u64, rhs as u64
        ),
        5 => format!(
            "integer overflow: {} * {} does not fit in u64",
            lhs as u64, rhs as u64
        ),
        6 => format!("integer overflow: {lhs} / {rhs} does not fit in i64"),
        7 => format!("integer overflow: {lhs} % {rhs} does not fit in i64"),
        _ => unreachable!("olive_overflow_fail: unknown kind {kind}"),
    };
    abort_with(&OVERFLOW, &msg, loc.as_deref())
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

    #[test]
    fn render_fault_includes_code_help_and_note() {
        let dir = std::env::temp_dir().join(format!("olive_fault_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("snippet.liv");
        std::fs::write(&path, "let x = a / b\n").unwrap();
        let loc = format!("{}:1:9", path.to_string_lossy());
        let fault = Fault {
            code: "E0703",
            help: Some("guard the divisor"),
            note: Some("the divisor was zero"),
        };
        let mut buf = Vec::new();
        render_fault(&mut buf, &fault, "divide by zero", Some(&loc), false);
        let rendered = String::from_utf8(buf).unwrap();
        assert!(rendered.contains("[E0703] panic: divide by zero"));
        assert!(rendered.contains("let x = a / b"));
        assert!(rendered.contains('^'));
        assert!(rendered.contains("help: guard the divisor"));
        assert!(rendered.contains("note: the divisor was zero"));
        assert!(rendered.contains("──╯"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_fault_without_location_is_inline() {
        let fault = Fault {
            code: "E0704",
            help: Some("handle the error case"),
            note: None,
        };
        let mut buf = Vec::new();
        render_fault(&mut buf, &fault, "unwrap on Err", None, false);
        let rendered = String::from_utf8(buf).unwrap();
        assert!(rendered.contains("[E0704] panic: unwrap on Err"));
        assert!(rendered.contains("help: handle the error case"));
        assert!(!rendered.contains("╭─["));
        assert!(!rendered.contains("──╯"));
    }
}
