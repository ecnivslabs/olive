use crate::{olive_str_from_ptr, olive_str_internal};
use crossterm::event::{
    Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags, read,
};
use crossterm::{cursor, execute, terminal};
use std::io::{IsTerminal, Write, stdout};

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
    execute!(stdout(), terminal::EnterAlternateScreen).is_ok() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_leave_alt_screen() -> i64 {
    execute!(stdout(), terminal::LeaveAlternateScreen).is_ok() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_clear() -> i64 {
    execute!(stdout(), terminal::Clear(terminal::ClearType::All)).is_ok() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_cursor_hide() -> i64 {
    execute!(stdout(), cursor::Hide).is_ok() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_cursor_show() -> i64 {
    execute!(stdout(), cursor::Show).is_ok() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_cursor_move(x: i64, y: i64) -> i64 {
    if x < 0 || y < 0 || x > u16::MAX as i64 || y > u16::MAX as i64 {
        return 0;
    }
    execute!(stdout(), cursor::MoveTo(x as u16, y as u16)).is_ok() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_flush() -> i64 {
    stdout().flush().is_ok() as i64
}

/// Writes a string to stdout with no trailing newline, for redrawing a line
/// in place under raw mode (`print` always appends one).
#[unsafe(no_mangle)]
pub extern "C" fn olive_term_write(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = olive_str_from_ptr(s);
    let mut out = stdout();
    let wrote = out.write_all(text.as_bytes()).is_ok();
    let flushed = out.flush().is_ok();
    (wrote && flushed) as i64
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
    execute!(
        stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_term_disable_key_enhancement() -> i64 {
    execute!(stdout(), PopKeyboardEnhancementFlags).is_ok() as i64
}

/// Normalizes a crossterm key event into the token string the olive `term`
/// module hands to callers: a named key ("enter", "backspace", "tab",
/// "escape", "up", "down", "left", "right", "home", "end", "delete"),
/// "ctrl+<c>" for control combos, or the literal character typed. Pure and
/// unit-testable independent of any real terminal.
fn encode_key(code: KeyCode, modifiers: KeyModifiers) -> Option<String> {
    Some(match code {
        KeyCode::Enter if modifiers.contains(KeyModifiers::SHIFT) => "shift+enter".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::Esc => "escape".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Char(c) if modifiers.contains(KeyModifiers::CONTROL) => format!("ctrl+{c}"),
        KeyCode::Char(c) => c.to_string(),
        _ => return None,
    })
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
        assert_eq!(encode_key(KeyCode::F(1), KeyModifiers::NONE), None);
        assert_eq!(encode_key(KeyCode::PageUp, KeyModifiers::NONE), None);
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
}
