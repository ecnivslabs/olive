#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub col: usize,
    pub start: usize,
    pub end: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)
    }
}

impl std::error::Error for ParseError {}

pub type ParseResult<T> = Result<T, ParseError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_formats_line_col_message() {
        let err = ParseError {
            message: "unexpected token".into(),
            line: 5,
            col: 10,
            start: 20,
            end: 25,
        };
        assert_eq!(err.to_string(), "5:10: unexpected token");
    }

    #[test]
    fn debug_output() {
        let err = ParseError {
            message: "test error".into(),
            line: 1,
            col: 2,
            start: 3,
            end: 4,
        };
        let s = format!("{:?}", err);
        assert!(s.contains("test error"));
        assert!(s.contains("line"));
    }

    #[test]
    fn clone_preserves_fields() {
        let err = ParseError {
            message: "original".into(),
            line: 10,
            col: 20,
            start: 30,
            end: 40,
        };
        let cloned = err.clone();
        assert_eq!(cloned.message, "original");
        assert_eq!(cloned.line, 10);
        assert_eq!(cloned.col, 20);
        assert_eq!(cloned.start, 30);
        assert_eq!(cloned.end, 40);
    }

    #[test]
    fn implements_error_trait() {
        fn takes_error<E: std::error::Error>() {}
        takes_error::<ParseError>();
    }

    #[test]
    fn display_with_different_values() {
        let err = ParseError {
            message: "syntax error".into(),
            line: 3,
            col: 7,
            start: 15,
            end: 20,
        };
        assert_eq!(format!("{}", err), "3:7: syntax error");
    }
}
