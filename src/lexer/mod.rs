mod engine;
mod error;
mod tests;
mod token;

pub use engine::Lexer;
pub use token::{Comment, CommentKind, Token, TokenKind};

/// Normalizes CRLF and lone CR line endings to LF. Every reader of raw
/// source text (files, stdin, editor overlays) must call this before the
/// string is stored or handed to `Lexer::new`, so byte offsets used for
/// diagnostics stay aligned between the two. `\r` is otherwise unhandled by
/// the indent tracker and produces spurious tokens on Windows checkouts.
pub fn normalize_newlines(source: &str) -> String {
    if !source.as_bytes().contains(&b'\r') {
        return source.to_string();
    }
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            out.push('\n');
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}
