use super::super::{Parser, ast::*, error::ParseResult};
use crate::lexer::{Token, TokenKind};

impl Parser {
    pub(crate) fn parse_postfix(&mut self) -> ParseResult<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek().kind {
                TokenKind::Dot | TokenKind::DoubleColon
                    if self.peek().kind == TokenKind::Dot
                        || self.peek_at(1).kind == TokenKind::Identifier =>
                {
                    let op = self.advance();
                    if op.kind == TokenKind::Dot && self.peek().kind == TokenKind::Await {
                        let end_tok = self.advance();
                        let span = expr.span.merge(self.span_from(&end_tok));
                        expr = Expr::new(ExprKind::Await(Box::new(expr)), span);
                        continue;
                    }
                    let attr = self.expect(TokenKind::Identifier)?.value;
                    let span = self.span_from(&Token {
                        kind: TokenKind::Identifier,
                        value: String::new(),
                        line: expr.span.line,
                        col: expr.span.col,
                        span: (expr.span.start, expr.span.end),
                        file_id: expr.span.file_id,
                    });
                    if op.kind == TokenKind::DoubleColon {
                        if let ExprKind::Identifier(ref name) = expr.kind {
                            expr = Expr::new(
                                ExprKind::Identifier(format!("{}::{}", name, attr)),
                                span,
                            );
                        } else {
                            return Err(self.err_at(&op, "expected identifier before '::'"));
                        }
                    } else {
                        expr = Expr::new(
                            ExprKind::Attr {
                                obj: Box::new(expr),
                                attr,
                            },
                            span,
                        );
                    }
                }
                TokenKind::QuestionDot => {
                    self.advance();
                    let attr = self.expect(TokenKind::Identifier)?.value;
                    let span = self.span_from(&Token {
                        kind: TokenKind::Identifier,
                        value: String::new(),
                        line: expr.span.line,
                        col: expr.span.col,
                        span: (expr.span.start, expr.span.end),
                        file_id: expr.span.file_id,
                    });
                    expr = Expr::new(
                        ExprKind::OptAttr {
                            obj: Box::new(expr),
                            attr,
                        },
                        span,
                    );
                }
                TokenKind::LBracket => {
                    self.advance();
                    let span_tok = Token {
                        kind: TokenKind::Identifier,
                        value: String::new(),
                        line: expr.span.line,
                        col: expr.span.col,
                        span: (expr.span.start, expr.span.end),
                        file_id: expr.span.file_id,
                    };
                    let index = if self.peek().kind == TokenKind::Colon {
                        self.advance();
                        self.parse_slice_remainder(None, &span_tok)?
                    } else if self.peek().kind == TokenKind::DoubleColon {
                        self.advance();
                        self.parse_slice_step(None, None, &span_tok)?
                    } else {
                        let first = self.parse_expr()?;
                        if self.peek().kind == TokenKind::Colon {
                            self.advance();
                            self.parse_slice_remainder(Some(Box::new(first)), &span_tok)?
                        } else if self.peek().kind == TokenKind::DoubleColon {
                            self.advance();
                            self.parse_slice_step(Some(Box::new(first)), None, &span_tok)?
                        } else {
                            first
                        }
                    };
                    self.expect(TokenKind::RBracket)?;
                    let span = self.span_from(&span_tok);
                    expr = Expr::new(
                        ExprKind::Index {
                            obj: Box::new(expr),
                            index: Box::new(index),
                        },
                        span,
                    );
                }
                TokenKind::LParen => {
                    self.advance();
                    let args = self.parse_call_args()?;
                    self.expect(TokenKind::RParen)?;
                    let span = self.span_from(&Token {
                        kind: TokenKind::Identifier,
                        value: String::new(),
                        line: expr.span.line,
                        col: expr.span.col,
                        span: (expr.span.start, expr.span.end),
                        file_id: expr.span.file_id,
                    });
                    expr = Expr::new(
                        ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                        },
                        span,
                    );
                }
                TokenKind::Question => {
                    let op = self.advance();
                    let span = expr.span.merge(self.span_from(&op));
                    expr = Expr::new(ExprKind::Try(Box::new(expr)), span);
                }
                TokenKind::As => {
                    self.advance();
                    let ty = self.parse_type_expr()?;
                    let span = expr.span.merge(ty.span);
                    expr = Expr::new(ExprKind::Cast(Box::new(expr), ty), span);
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    pub(crate) fn parse_call_args(&mut self) -> ParseResult<Vec<CallArg>> {
        let mut args = Vec::new();
        while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
            let arg = if self.peek().kind == TokenKind::DoubleStar {
                self.advance();
                CallArg::KwSplat(self.parse_expr()?)
            } else if self.peek().kind == TokenKind::Star {
                self.advance();
                CallArg::Splat(self.parse_expr()?)
            } else if self.peek().kind == TokenKind::Identifier
                && self.peek_at(1).kind == TokenKind::Equal
            {
                let name = self.advance().value;
                self.advance();
                CallArg::Keyword(name, self.parse_expr()?)
            } else {
                CallArg::Positional(self.parse_expr()?)
            };
            args.push(arg);
            if self.peek().kind == TokenKind::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(args)
    }

    fn parse_slice_remainder(
        &mut self,
        start: Option<Box<Expr>>,
        span_tok: &Token,
    ) -> ParseResult<Expr> {
        let stop =
            if self.peek().kind == TokenKind::RBracket || self.peek().kind == TokenKind::Colon {
                None
            } else {
                Some(Box::new(self.parse_expr()?))
            };
        if self.peek().kind == TokenKind::Colon {
            self.advance();
            return self.parse_slice_step(start, stop, span_tok);
        }
        let slice_span = self.span_from(span_tok);
        Ok(Expr::new(
            ExprKind::Slice {
                start,
                stop,
                step: None,
            },
            slice_span,
        ))
    }

    /// Parses an optional step after the second slice colon has been consumed.
    fn parse_slice_step(
        &mut self,
        start: Option<Box<Expr>>,
        stop: Option<Box<Expr>>,
        span_tok: &Token,
    ) -> ParseResult<Expr> {
        let step = if self.peek().kind == TokenKind::RBracket {
            None
        } else {
            Some(Box::new(self.parse_expr()?))
        };
        let slice_span = self.span_from(span_tok);
        Ok(Expr::new(ExprKind::Slice { start, stop, step }, slice_span))
    }
}
