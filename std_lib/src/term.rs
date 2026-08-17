use crate::{olive_str_from_ptr, olive_str_internal};
use base64::{Engine, engine::general_purpose::STANDARD};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags, poll, read,
};
use crossterm::{cursor, queue, terminal};
use std::fs::File;
use std::io::{BufWriter, IsTerminal, Write, stdout};
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
#[cfg(windows)]
use std::os::windows::io::FromRawHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[cfg(unix)]
fn raw_stdout_file() -> File {
    unsafe { File::from_raw_fd(1) }
}

#[cfg(windows)]
fn raw_stdout_file() -> File {
    extern "system" {
        fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
    }
    unsafe { File::from_raw_handle(GetStdHandle((-11i32) as u32)) }
}

/// All `term.*` output funnels through this single handle instead of a
/// fresh `stdout()` per call. Buffering is scoped to a `begin_sync`/
/// `end_sync` bracket (`SYNC_ACTIVE`): outside one, every op still flushes
/// immediately, so a bare `term.write` keeps the old "flushes every call"
/// contract plain scripts rely on. Inside one -- which is how the frame
/// renderer's `_apply` always uses it, one bracket per redrawn frame --
/// writes queue and only `end_sync` flushes, cutting a redrawn frame from
/// 4+ syscalls per changed line down to one. Raw fd 1 / stdout handle, not
/// `std::io::Stdout`, since `Stdout` internally line-buffers and flushes on
/// every `\n`.
fn term_out() -> &'static Mutex<BufWriter<File>> {
    static OUT: OnceLock<Mutex<BufWriter<File>>> = OnceLock::new();
    OUT.get_or_init(|| Mutex::new(BufWriter::with_capacity(64 * 1024, raw_stdout_file())))
}

static SYNC_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Flushes immediately unless a sync bracket is open, in which case the
/// flush is deferred to `end_sync`.
fn flush_unless_syncing(out: &mut BufWriter<File>) -> bool {
    if SYNC_ACTIVE.load(Ordering::Relaxed) {
        return true;
    }
    out.flush().is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_enable_raw() -> i64 {
    terminal::enable_raw_mode().is_ok() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_disable_raw() -> i64 {
    terminal::disable_raw_mode().is_ok() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_enter_alt_screen() -> i64 {
    let mut out = term_out().lock().unwrap();
    let ok = queue!(*out, terminal::EnterAlternateScreen).is_ok();
    (ok && flush_unless_syncing(&mut out)) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_leave_alt_screen() -> i64 {
    let mut out = term_out().lock().unwrap();
    let ok = queue!(*out, terminal::LeaveAlternateScreen).is_ok();
    (ok && flush_unless_syncing(&mut out)) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_clear() -> i64 {
    let mut out = term_out().lock().unwrap();
    let ok = queue!(*out, terminal::Clear(terminal::ClearType::All)).is_ok();
    (ok && flush_unless_syncing(&mut out)) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_cursor_hide() -> i64 {
    let mut out = term_out().lock().unwrap();
    let ok = queue!(*out, cursor::Hide).is_ok();
    (ok && flush_unless_syncing(&mut out)) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_cursor_show() -> i64 {
    let mut out = term_out().lock().unwrap();
    let ok = queue!(*out, cursor::Show).is_ok();
    (ok && flush_unless_syncing(&mut out)) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_cursor_move(x: i64, y: i64) -> i64 {
    if x < 0 || y < 0 || x > u16::MAX as i64 || y > u16::MAX as i64 {
        return 0;
    }
    let mut out = term_out().lock().unwrap();
    let ok = queue!(*out, cursor::MoveTo(x as u16, y as u16)).is_ok();
    (ok && flush_unless_syncing(&mut out)) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_flush() -> i64 {
    term_out().lock().unwrap().flush().is_ok() as i64
}

/// Writes a string to stdout with no trailing newline, for redrawing a line
/// in place under raw mode (`print` always appends one). Flushes
/// immediately unless called inside a `begin_sync`/`end_sync` bracket.
#[unsafe(no_mangle)]
pub extern "C" fn olive_term_write(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = olive_str_from_ptr(s);
    let mut out = term_out().lock().unwrap();
    let ok = out.write_all(text.as_bytes()).is_ok();
    (ok && flush_unless_syncing(&mut out)) as i64
}

/// Opens a synchronized-output bracket (DECSET 2026): every `term.*` write
/// until the matching `end_sync` is buffered and flushed as one syscall,
/// instead of one flush per write.
#[unsafe(no_mangle)]
pub extern "C" fn olive_term_begin_sync() -> i64 {
    let mut out = term_out().lock().unwrap();
    let ok = out.write_all(b"\x1b[?2026h").is_ok();
    let flushed = out.flush().is_ok();
    SYNC_ACTIVE.store(true, Ordering::Relaxed);
    (ok && flushed) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_end_sync() -> i64 {
    let mut out = term_out().lock().unwrap();
    let ok = out.write_all(b"\x1b[?2026l").is_ok();
    SYNC_ACTIVE.store(false, Ordering::Relaxed);
    (ok && out.flush().is_ok()) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_cols() -> i64 {
    terminal::size().map(|(c, _)| c as i64).unwrap_or(80)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_rows() -> i64 {
    terminal::size().map(|(_, r)| r as i64).unwrap_or(24)
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_is_tty() -> i64 {
    stdout().is_terminal() as i64
}

/// Requests disambiguated key events (e.g. shift+enter distinct from enter)
/// from terminals that support the Kitty keyboard protocol. A no-op, safely
/// ignorable, on terminals that don't -- `read_key` still works there, it just
/// can't tell shift+enter apart from enter.
#[unsafe(no_mangle)]
pub extern "C" fn olive_term_enable_key_enhancement() -> i64 {
    if !matches!(terminal::supports_keyboard_enhancement(), Ok(true)) {
        return 0;
    }
    let mut out = term_out().lock().unwrap();
    let ok = queue!(
        *out,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok();
    (ok && out.flush().is_ok()) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_disable_key_enhancement() -> i64 {
    let mut out = term_out().lock().unwrap();
    let ok = queue!(*out, PopKeyboardEnhancementFlags).is_ok();
    (ok && out.flush().is_ok()) as i64
}

/// Requests SGR mouse tracking (wheel + button presses) from the terminal.
/// Without this, wheel scroll is invisible to the app entirely -- some
/// terminals fall back to sending arrow-key codes for it, which is
/// indistinguishable from a real key press.
#[unsafe(no_mangle)]
pub extern "C" fn olive_term_enable_mouse() -> i64 {
    let mut out = term_out().lock().unwrap();
    let ok = queue!(*out, EnableMouseCapture).is_ok();
    (ok && out.flush().is_ok()) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_disable_mouse() -> i64 {
    let mut out = term_out().lock().unwrap();
    let ok = queue!(*out, DisableMouseCapture).is_ok();
    (ok && out.flush().is_ok()) as i64
}

/// Writes `text` to the system clipboard via OSC 52 -- works locally, over
/// SSH, and inside tmux/screen with clipboard passthrough enabled, with no
/// platform-specific clipboard binary required.
#[unsafe(no_mangle)]
pub extern "C" fn olive_term_clipboard_write(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = olive_str_from_ptr(s);
    let encoded = STANDARD.encode(text.as_bytes());
    let seq = format!("\x1b]52;c;{encoded}\x07");
    let mut out = term_out().lock().unwrap();
    let wrote = out.write_all(seq.as_bytes()).is_ok();
    (wrote && out.flush().is_ok()) as i64
}

/// Builds the fixed-order "ctrl+alt+shift+" prefix for whichever of those
/// three modifiers are held, empty string if none are. Kept separate from
/// `encode_key` so a modifier combo is spelled identically no matter which
/// named key it lands on.
fn modifier_prefix(modifiers: KeyModifiers) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl");
    }
    if modifiers.contains(KeyModifiers::ALT) {
        parts.push("alt");
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("shift");
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("{}+", parts.join("+"))
    }
}

fn encode_named(base: &str, modifiers: KeyModifiers) -> String {
    format!("{}{base}", modifier_prefix(modifiers))
}

/// Normalizes a crossterm key event into the token string the olive `term`
/// module hands to callers: a named key ("enter", "backspace", "tab",
/// "escape", "up", "down", "left", "right", "home", "end", "delete",
/// "pageup", "pagedown", "insert", "backtab", "f1".."f12"), any of those
/// prefixed with a fixed-order "ctrl+alt+shift+" combo when held (e.g.
/// "shift+up", "ctrl+alt+left"), "ctrl+<c>" for a held-control character,
/// or the literal character typed otherwise. Pure and unit-testable
/// independent of any real terminal.
fn encode_key(code: KeyCode, modifiers: KeyModifiers) -> Option<String> {
    Some(match code {
        KeyCode::Enter if modifiers.contains(KeyModifiers::SHIFT) => "shift+enter".to_string(),
        KeyCode::Enter => encode_named("enter", modifiers),
        KeyCode::Backspace => encode_named("backspace", modifiers),
        KeyCode::Tab => encode_named("tab", modifiers),
        KeyCode::BackTab => encode_named("backtab", modifiers),
        KeyCode::Esc => encode_named("escape", modifiers),
        KeyCode::Up => encode_named("up", modifiers),
        KeyCode::Down => encode_named("down", modifiers),
        KeyCode::Left => encode_named("left", modifiers),
        KeyCode::Right => encode_named("right", modifiers),
        KeyCode::Home => encode_named("home", modifiers),
        KeyCode::End => encode_named("end", modifiers),
        KeyCode::Delete => encode_named("delete", modifiers),
        KeyCode::PageUp => encode_named("pageup", modifiers),
        KeyCode::PageDown => encode_named("pagedown", modifiers),
        KeyCode::Insert => encode_named("insert", modifiers),
        KeyCode::F(n) if (1..=12).contains(&n) => encode_named(&format!("f{n}"), modifiers),
        KeyCode::Char(c) if modifiers.contains(KeyModifiers::CONTROL) => format!("ctrl+{c}"),
        KeyCode::Char(c) => c.to_string(),
        _ => return None,
    })
}

/// Normalizes a crossterm mouse event into a token: "wheelup"/"wheeldown"
/// for scroll, "mousedown:<col>:<row>"/"mousedrag:<col>:<row>"/
/// "mouseup:<col>:<row>" for left-button press/move-while-held/release --
/// together these let callers tell a plain click (down+up, no drag) apart
/// from a text-selection drag. Other buttons are swallowed.
fn encode_mouse(event: MouseEvent) -> Option<String> {
    match event.kind {
        MouseEventKind::ScrollUp => Some("wheelup".to_string()),
        MouseEventKind::ScrollDown => Some("wheeldown".to_string()),
        MouseEventKind::Down(MouseButton::Left) => {
            Some(format!("mousedown:{}:{}", event.column, event.row))
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            Some(format!("mousedrag:{}:{}", event.column, event.row))
        }
        MouseEventKind::Up(MouseButton::Left) => {
            Some(format!("mouseup:{}:{}", event.column, event.row))
        }
        _ => None,
    }
}

/// Blocks for the next key press or terminal resize and returns a token from
/// `encode_key`, "resize" on geometry change, or "eof" if the input stream
/// is closed or errored -- callers must treat "eof" as a hard stop, since
/// retrying `read()` after that will busy-loop returning the same result.
/// Key-release events are swallowed so callers only see presses.
#[unsafe(no_mangle)]
pub extern "C" fn olive_term_read_key() -> i64 {
    loop {
        let event = match read() {
            Ok(event) => event,
            Err(_) => return olive_str_internal("eof"),
        };
        let key = match event {
            Event::Resize(_, _) => return olive_str_internal("resize"),
            Event::Key(key) => key,
            Event::Mouse(m) => match encode_mouse(m) {
                Some(token) => return olive_str_internal(&token),
                None => continue,
            },
            _ => continue,
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match encode_key(key.code, key.modifiers) {
            Some(token) => return olive_str_internal(&token),
            None => continue,
        }
    }
}

/// Same contract as `olive_term_read_key`, but returns "idle" once `ms`
/// elapses with no event instead of blocking forever. Lets callers redraw
/// on a tick (e.g. to clear a timed-out confirmation hint) without a
/// second thread.
#[unsafe(no_mangle)]
pub extern "C" fn olive_term_read_key_timeout(ms: i64) -> i64 {
    let budget = Duration::from_millis(ms.max(0) as u64);
    loop {
        let ready = match poll(budget) {
            Ok(ready) => ready,
            Err(_) => return olive_str_internal("eof"),
        };
        if !ready {
            return olive_str_internal("idle");
        }
        let event = match read() {
            Ok(event) => event,
            Err(_) => return olive_str_internal("eof"),
        };
        let key = match event {
            Event::Resize(_, _) => return olive_str_internal("resize"),
            Event::Key(key) => key,
            Event::Mouse(m) => match encode_mouse(m) {
                Some(token) => return olive_str_internal(&token),
                None => continue,
            },
            _ => continue,
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match encode_key(key.code, key.modifiers) {
            Some(token) => return olive_str_internal(&token),
            None => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_key_plain_enter() {
        assert_eq!(
            encode_key(KeyCode::Enter, KeyModifiers::NONE),
            Some("enter".to_string())
        );
    }

    #[test]
    fn encode_key_shift_enter() {
        assert_eq!(
            encode_key(KeyCode::Enter, KeyModifiers::SHIFT),
            Some("shift+enter".to_string())
        );
    }

    #[test]
    fn encode_key_ctrl_c() {
        assert_eq!(
            encode_key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some("ctrl+c".to_string())
        );
    }

    #[test]
    fn encode_key_plain_char() {
        assert_eq!(
            encode_key(KeyCode::Char('x'), KeyModifiers::NONE),
            Some("x".to_string())
        );
    }

    #[test]
    fn encode_key_shift_char_is_uppercase_via_crossterm() {
        // crossterm reports shifted letters as the uppercase char itself,
        // not a separate modifier -- confirm we pass it through untouched.
        assert_eq!(
            encode_key(KeyCode::Char('X'), KeyModifiers::SHIFT),
            Some("X".to_string())
        );
    }

    #[test]
    fn encode_key_named_keys() {
        assert_eq!(
            encode_key(KeyCode::Backspace, KeyModifiers::NONE),
            Some("backspace".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Tab, KeyModifiers::NONE),
            Some("tab".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Esc, KeyModifiers::NONE),
            Some("escape".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Up, KeyModifiers::NONE),
            Some("up".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Down, KeyModifiers::NONE),
            Some("down".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Left, KeyModifiers::NONE),
            Some("left".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Right, KeyModifiers::NONE),
            Some("right".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Home, KeyModifiers::NONE),
            Some("home".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::End, KeyModifiers::NONE),
            Some("end".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Delete, KeyModifiers::NONE),
            Some("delete".to_string())
        );
    }

    #[test]
    fn encode_key_unhandled_code_is_none() {
        assert_eq!(encode_key(KeyCode::Null, KeyModifiers::NONE), None);
        assert_eq!(encode_key(KeyCode::Menu, KeyModifiers::NONE), None);
        assert_eq!(encode_key(KeyCode::F(13), KeyModifiers::NONE), None);
    }

    #[test]
    fn encode_key_new_named_keys() {
        assert_eq!(
            encode_key(KeyCode::PageUp, KeyModifiers::NONE),
            Some("pageup".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::PageDown, KeyModifiers::NONE),
            Some("pagedown".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Insert, KeyModifiers::NONE),
            Some("insert".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::BackTab, KeyModifiers::NONE),
            Some("backtab".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::F(1), KeyModifiers::NONE),
            Some("f1".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::F(12), KeyModifiers::NONE),
            Some("f12".to_string())
        );
    }

    #[test]
    fn encode_key_modified_named_keys() {
        assert_eq!(
            encode_key(KeyCode::Up, KeyModifiers::SHIFT),
            Some("shift+up".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Down, KeyModifiers::CONTROL),
            Some("ctrl+down".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Left, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            Some("ctrl+shift+left".to_string())
        );
        assert_eq!(
            encode_key(
                KeyCode::Right,
                KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT
            ),
            Some("ctrl+alt+shift+right".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::F(5), KeyModifiers::ALT),
            Some("alt+f5".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Enter, KeyModifiers::CONTROL),
            Some("ctrl+enter".to_string())
        );
    }

    #[test]
    fn encode_key_bare_named_keys_unchanged() {
        assert_eq!(
            encode_key(KeyCode::Enter, KeyModifiers::NONE),
            Some("enter".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Tab, KeyModifiers::NONE),
            Some("tab".to_string())
        );
        assert_eq!(
            encode_key(KeyCode::Backspace, KeyModifiers::NONE),
            Some("backspace".to_string())
        );
    }

    #[test]
    fn encode_key_ctrl_takes_priority_over_shift_on_char() {
        assert_eq!(
            encode_key(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            ),
            Some("ctrl+c".to_string())
        );
    }

    fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn encode_mouse_wheel_up() {
        assert_eq!(
            encode_mouse(mouse_event(MouseEventKind::ScrollUp, 0, 0)),
            Some("wheelup".to_string())
        );
    }

    #[test]
    fn encode_mouse_wheel_down() {
        assert_eq!(
            encode_mouse(mouse_event(MouseEventKind::ScrollDown, 0, 0)),
            Some("wheeldown".to_string())
        );
    }

    #[test]
    fn encode_mouse_left_down() {
        assert_eq!(
            encode_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 12, 34)),
            Some("mousedown:12:34".to_string())
        );
    }

    #[test]
    fn encode_mouse_left_drag() {
        assert_eq!(
            encode_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 5, 9)),
            Some("mousedrag:5:9".to_string())
        );
    }

    #[test]
    fn encode_mouse_left_up() {
        assert_eq!(
            encode_mouse(mouse_event(MouseEventKind::Up(MouseButton::Left), 5, 9)),
            Some("mouseup:5:9".to_string())
        );
    }

    #[test]
    fn encode_mouse_right_click_is_none() {
        assert_eq!(
            encode_mouse(mouse_event(MouseEventKind::Down(MouseButton::Right), 0, 0)),
            None
        );
    }
}
