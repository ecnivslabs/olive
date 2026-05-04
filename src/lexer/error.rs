#[derive(Debug, Clone)]
pub struct LexError {
    pub message: String,
    pub line: usize,
    pub col: usize,
    pub start: usize,
    pub end: usize,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)
    }
}

impl std::error::Error for LexError {}

pub type LexResult<T> = Result<T, LexError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_format() {
        let err = LexError {
            message: "test error".into(),
            line: 5,
            col: 10,
            start: 0,
            end: 0,
        };
        assert_eq!(err.to_string(), "5:10: test error");
    }

    #[test]
    fn implements_error_trait() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<LexError>();
    }

    #[test]
    fn clone_and_debug() {
        let err = LexError {
            message: "debug".into(),
            line: 1,
            col: 2,
            start: 3,
            end: 4,
        };
        let cloned = err.clone();
        assert_eq!(err.to_string(), cloned.to_string());
        let debug = format!("{:?}", err);
        assert!(debug.contains("debug"));
    }
}
