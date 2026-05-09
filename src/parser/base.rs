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
        let end = if self.pos > 0 {
            self.tokens[self.pos - 1].span.1
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
