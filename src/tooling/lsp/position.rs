//! Char-offset `Span` <-> UTF-16 `Position`; diverges on chars outside the BMP.

use crate::span::Span;
use serde_json::{Value, json};

pub struct LineIndex {
    /// Char offset where each line starts, index 0 is line 0's start (always 0).
    line_starts: Vec<usize>,
    chars: Vec<char>,
}

/// 0-based; `character` is a UTF-16 code-unit offset within the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

impl LspPosition {
    pub fn to_json(self) -> Value {
        json!({"line": self.line, "character": self.character})
    }
}

impl LspRange {
    pub fn to_json(self) -> Value {
        json!({"start": self.start.to_json(), "end": self.end.to_json()})
    }
}

impl LineIndex {
    pub fn new(source: &str) -> Self {
        let chars: Vec<char> = source.chars().collect();
        let mut line_starts = vec![0];
        for (i, c) in chars.iter().enumerate() {
            if *c == '\n' {
                line_starts.push(i + 1);
            }
        }
        Self { line_starts, chars }
    }

    /// Char offset -> LSP position; out-of-range clamps rather than panics.
    pub fn to_lsp(&self, char_offset: usize) -> LspPosition {
        let offset = char_offset.min(self.chars.len());
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = self.line_starts[line];
        let character: u32 = self.chars[line_start..offset]
            .iter()
            .map(|c| c.len_utf16() as u32)
            .sum();
        LspPosition {
            line: line as u32,
            character,
        }
    }

    /// Inverse of `to_lsp`.
    pub fn to_char_offset(&self, pos: LspPosition) -> usize {
        let line = (pos.line as usize).min(self.line_starts.len().saturating_sub(1));
        let start = self.line_starts[line];
        let end = self
            .line_starts
            .get(line + 1)
            .map(|&s| s.saturating_sub(1)) // exclude the newline itself
            .unwrap_or(self.chars.len());

        let mut units = 0u32;
        let mut offset = start;
        while offset < end && units < pos.character {
            units += self.chars[offset].len_utf16() as u32;
            offset += 1;
        }
        offset
    }

    pub fn span_to_range(&self, span: Span) -> LspRange {
        LspRange {
            start: self.to_lsp(span.start),
            end: self.to_lsp(span.end),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_single_line() {
        let idx = LineIndex::new("let x = 1");
        assert_eq!(
            idx.to_lsp(4),
            LspPosition {
                line: 0,
                character: 4
            }
        );
    }

    #[test]
    fn ascii_multi_line() {
        let idx = LineIndex::new("let x = 1\nprint(x)\n");
        // offset of `p` in `print`, second line.
        let offset = "let x = 1\n".chars().count();
        assert_eq!(
            idx.to_lsp(offset),
            LspPosition {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn multibyte_emoji_diverges_char_vs_utf16() {
        // 😀 is one char but two UTF-16 units; next char must land at column 2.
        let src = "😀x";
        let idx = LineIndex::new(src);
        let x_char_offset = 1; // second `char` in the Vec<char>, i.e. 'x'
        assert_eq!(
            idx.to_lsp(x_char_offset),
            LspPosition {
                line: 0,
                character: 2
            }
        );
    }

    #[test]
    fn roundtrip_char_offset_through_lsp_position() {
        let src = "😀 let value = 42\nnext line here";
        let idx = LineIndex::new(src);
        for offset in 0..src.chars().count() {
            let pos = idx.to_lsp(offset);
            assert_eq!(
                idx.to_char_offset(pos),
                offset,
                "roundtrip failed at {offset}"
            );
        }
    }

    #[test]
    fn out_of_range_offset_clamps_instead_of_panicking() {
        let idx = LineIndex::new("abc");
        let pos = idx.to_lsp(999);
        assert_eq!(idx.to_char_offset(pos), 3);
    }

    #[test]
    fn span_to_range_converts_both_ends() {
        let idx = LineIndex::new("let x = 1\n");
        let span = Span {
            file_id: 0,
            line: 1,
            col: 5,
            start: 4,
            end: 5,
        };
        let range = idx.span_to_range(span);
        assert_eq!(range.start.character, 4);
        assert_eq!(range.end.character, 5);
    }
}
