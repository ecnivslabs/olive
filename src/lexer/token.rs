#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Identifier,
    Integer,
    Float,
    String,
    FString,

    Fn,
    Let,
    Const,
    If,
    Else,
    Elif,
    While,
    For,
    In,
    Return,
    True,
    False,
    Not,
    And,
    Or,
    Pass,
    Break,
    Continue,
    Import,
    From,
    Struct,
    Impl,
    Trait,
    Try,
    As,
    Assert,
    Mut,
    Enum,
    Match,
    Async,
    Await,
    Case,
    Unsafe,
    Defer,
    Null,
    With,
    Lambda,

    Plus,
    Minus,
    Star,
    DoubleStar,
    Slash,
    Percent,
    Equal,
    DoubleEqual,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    PercentEqual,
    DoubleStarEqual,
    Shl,
    Shr,
    ShlEqual,
    ShrEqual,
    Ampersand,
    Pipe,
    Caret,
    Tilde,
    AmpersandEqual,
    PipeEqual,
    CaretEqual,

    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Colon,
    Comma,
    Dot,
    DotDot,
    DotDotEq,
    Arrow,
    Semicolon,
    At,
    Underscore,
    DoubleColon,
    Question,
    QuestionQuestion,
    Hash,

    Newline,
    Indent,
    Dedent,

    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub value: String,
    pub line: usize,
    pub col: usize,
    pub span: (usize, usize),
    pub file_id: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    Line,
    Block,
}

/// A comment recovered by the lexer. Comments never enter the token stream; they
/// are collected on the side so the formatter can re-attach them. `span` holds the
/// `(start, end)` char offsets of the comment, including its `//` or `/* */` markers.
#[derive(Debug, Clone)]
pub struct Comment {
    pub kind: CommentKind,
    pub text: String,
    pub span: (usize, usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_kind_equality() {
        assert_eq!(TokenKind::Fn, TokenKind::Fn);
        assert_ne!(TokenKind::Fn, TokenKind::Let);
    }

    #[test]
    fn token_kinds_are_distinct() {
        assert_ne!(TokenKind::Fn, TokenKind::Let);
        assert_ne!(TokenKind::If, TokenKind::Elif);
        assert_ne!(TokenKind::Plus, TokenKind::Minus);
        assert_ne!(TokenKind::LParen, TokenKind::RParen);
        assert_ne!(TokenKind::Newline, TokenKind::Eof);
        assert_ne!(TokenKind::Integer, TokenKind::Float);
    }

    #[test]
    fn token_construction() {
        let tok = Token {
            kind: TokenKind::Integer,
            value: "42".into(),
            line: 1,
            col: 5,
            span: (4, 6),
            file_id: 0,
        };
        assert_eq!(tok.kind, TokenKind::Integer);
        assert_eq!(tok.value, "42");
        assert_eq!(tok.line, 1);
        assert_eq!(tok.col, 5);
    }

    #[test]
    fn token_debug_and_clone() {
        let tok = Token {
            kind: TokenKind::Fn,
            value: "fn".into(),
            line: 1,
            col: 1,
            span: (0, 2),
            file_id: 0,
        };
        let cloned = tok.clone();
        assert_eq!(tok.kind, cloned.kind);
        let debug = format!("{:?}", tok);
        assert!(debug.contains("Fn"));
    }
}
