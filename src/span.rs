/// Source span tracking exact position in source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub file_id: usize,
    pub line: usize,
    pub col: usize,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn merge(self, other: Span) -> Span {
        Span {
            file_id: self.file_id,
            line: self.line,
            col: self.col,
            start: self.start,
            end: other.end,
        }
    }
}
