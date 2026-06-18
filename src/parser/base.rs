use super::{
    Parser, Program,
    error::{ParseError, ParseResult},
};
use crate::lexer::{Token, TokenKind};
use crate::span::Span;

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub(crate) fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    pub(crate) fn peek_at(&self, offset: usize) -> &Token {
        let i = self.pos + offset;
        if i < self.tokens.len() {
            &self.tokens[i]
        } else {
            self.tokens.last().unwrap()
        }
    }

    pub(crate) fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if tok.kind != TokenKind::Eof {
            self.pos += 1;
        }
        tok
    }

    pub(crate) fn expect(&mut self, kind: TokenKind) -> ParseResult<Token> {
        let tok = self.peek().clone();
        if tok.kind == kind {
            if tok.kind != TokenKind::Eof {
                self.pos += 1;
            }
            Ok(tok)
        } else {
            Err(ParseError {
                message: format!("expected {:?}, got {:?} {:?}", kind, tok.kind, tok.value),
                line: tok.line,
                col: tok.col,
                start: tok.span.0,
                end: tok.span.1,
            })
        }
    }

    pub(crate) fn err_at(&self, tok: &Token, msg: impl Into<String>) -> ParseError {
        ParseError {
            message: msg.into(),
            line: tok.line,
            col: tok.col,
            start: tok.span.0,
            end: tok.span.1,
        }
    }

    pub(crate) fn skip_newlines(&mut self) {
        while self.peek().kind == TokenKind::Newline {
            self.pos += 1;
        }
    }

    pub(crate) fn eat_stmt_end(&mut self) -> ParseResult<()> {
        match self.peek().kind {
            TokenKind::Newline | TokenKind::Semicolon => {
                self.pos += 1;
                Ok(())
            }
            TokenKind::Eof | TokenKind::Dedent => Ok(()),
            _ => {
                let tok = self.peek().clone();
                Err(ParseError {
                    message: format!("expected newline, got {:?} {:?}", tok.kind, tok.value),
                    line: tok.line,
                    col: tok.col,
                    start: tok.span.0,
                    end: tok.span.1,
                })
            }
        }
    }

    pub(crate) fn span_from(&self, start: &Token) -> Span {
        let mut idx = self.pos;
        while idx > 0
            && matches!(
                self.tokens[idx - 1].kind,
                TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent
            )
        {
            idx -= 1;
        }
        let end = if idx > 0 {
            self.tokens[idx - 1].span.1.max(start.span.1)
        } else {
            start.span.1
        };
        Span {
            file_id: start.file_id,
            line: start.line,
            col: start.col,
            start: start.span.0,
            end,
        }
    }

    pub fn parse_program(&mut self) -> ParseResult<Program> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while self.peek().kind != TokenKind::Eof {
            stmts.push(self.parse_stmt()?);
            self.skip_newlines();
        }
        Ok(Program { stmts })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_parser(src: &str) -> Parser {
        let tokens = crate::lexer::Lexer::new(src, 0)
            .tokenise()
            .expect("lex error");
        Parser::new(tokens)
    }

    #[test]
    fn peek_returns_first_token() {
        let p = make_parser("42\n");
        assert_eq!(p.peek().kind, TokenKind::Integer);
        assert_eq!(p.peek().value, "42");
    }

    #[test]
    fn peek_at_offset() {
        let p = make_parser("fn f()\n");
        assert_eq!(p.peek_at(0).kind, TokenKind::Fn);
        assert_eq!(p.peek_at(1).kind, TokenKind::Identifier);
    }

    #[test]
    fn peek_at_beyond_length_returns_last() {
        let p = make_parser("42\n");
        assert_eq!(p.peek_at(100).kind, TokenKind::Eof);
    }

    #[test]
    fn advance_consumes_and_returns_token() {
        let mut p = make_parser("42 99\n");
        assert_eq!(p.advance().value, "42");
        assert_eq!(p.peek().kind, TokenKind::Integer);
    }

    #[test]
    fn advance_stops_at_eof() {
        let mut p = make_parser("");
        let t1 = p.advance();
        assert_eq!(t1.kind, TokenKind::Eof);
        assert_eq!(p.pos, 0);
    }

    #[test]
    fn expect_ok_on_matching_kind() {
        let mut p = make_parser("42\n");
        let tok = p.expect(TokenKind::Integer).expect("expected integer");
        assert_eq!(tok.value, "42");
    }

    #[test]
    fn expect_err_on_mismatch() {
        let mut p = make_parser("fn\n");
        let err = p.expect(TokenKind::If).unwrap_err();
        assert!(err.message.contains("expected"));
    }

    #[test]
    fn err_at_creates_error_with_token_info() {
        let p = make_parser("x\n");
        let tok = p.peek().clone();
        let err = p.err_at(&tok, "custom error");
        assert_eq!(err.message, "custom error");
        assert_eq!(err.line, tok.line);
        assert_eq!(err.col, tok.col);
    }

    #[test]
    fn skip_newlines_advances_past_newlines() {
        let mut p = make_parser("\n\n\n42\n");
        p.skip_newlines();
        assert_eq!(p.peek().kind, TokenKind::Integer);
    }

    #[test]
    fn eat_stmt_end_accepts_newline() {
        let mut p = make_parser("\n");
        assert!(p.eat_stmt_end().is_ok());
    }

    #[test]
    fn eat_stmt_end_accepts_eof() {
        let mut p = make_parser("");
        assert!(p.eat_stmt_end().is_ok());
    }

    #[test]
    fn eat_stmt_end_rejects_invalid() {
        let mut p = make_parser("fn");
        let err = p.eat_stmt_end().unwrap_err();
        assert!(err.message.contains("expected newline"));
    }

    #[test]
    fn span_from_uses_start_and_current_position() {
        let mut p = make_parser("42\n");
        let start = p.peek().clone();
        p.advance();
        let span = p.span_from(&start);
        assert_eq!(span.start, start.span.0);
        assert_eq!(span.file_id, start.file_id);
    }

    #[test]
    fn parse_program_empty() {
        let mut p = make_parser("");
        let prog = p.parse_program().expect("parse failed");
        assert!(prog.stmts.is_empty());
    }

    #[test]
    fn parse_program_single_stmt() {
        let mut p = make_parser("pass\n");
        let prog = p.parse_program().expect("parse failed");
        assert_eq!(prog.stmts.len(), 1);
    }
}
