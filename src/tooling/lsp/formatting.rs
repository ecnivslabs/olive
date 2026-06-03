//! `textDocument/formatting`: one edit replacing the whole document.

use crate::fmt::{self, DEFAULT_WIDTH};

/// Formatted text, or `None` if the source doesn't parse.
pub fn format_document(source: &str) -> Option<String> {
    fmt::format_source(source, DEFAULT_WIDTH).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_valid_source() {
        let out = format_document("let   x=1\n").expect("formats");
        assert!(out.contains("let x = 1"));
    }

    #[test]
    fn unparseable_source_returns_none() {
        assert!(format_document("let x = \n").is_none());
    }
}
