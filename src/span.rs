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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_default() {
        let s = Span::default();
        assert_eq!(s.file_id, 0);
        assert_eq!(s.start, 0);
        assert_eq!(s.end, 0);
    }

    #[test]
    fn span_merge() {
        let a = Span {
            file_id: 0,
            line: 1,
            col: 1,
            start: 5,
            end: 10,
        };
        let b = Span {
            file_id: 0,
            line: 1,
            col: 1,
            start: 10,
            end: 20,
        };
        let m = a.merge(b);
        assert_eq!(m.start, 5);
        assert_eq!(m.end, 20);
    }

    #[test]
    fn span_equality() {
        let a = Span {
            file_id: 0,
            line: 1,
            col: 2,
            start: 3,
            end: 4,
        };
        let b = Span {
            file_id: 0,
            line: 1,
            col: 2,
            start: 3,
            end: 4,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn span_inequality() {
        let a = Span {
            file_id: 0,
            line: 1,
            col: 2,
            start: 3,
            end: 4,
        };
        let b = Span {
            file_id: 1,
            line: 1,
            col: 2,
            start: 3,
            end: 4,
        };
        assert_ne!(a, b);
    }
}
