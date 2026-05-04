use super::super::{Parser, ast::*, error::ParseResult};
use crate::lexer::TokenKind;

impl Parser {
    pub(crate) fn parse_if(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::If)?;
        let condition = self.parse_expr()?;
        let then_body = self.parse_block()?;

        let mut elif_clauses = Vec::new();
        let mut else_body = None;

        loop {
            self.skip_newlines();
            let kind = self.peek().kind.clone();
            if kind == TokenKind::Elif {
                self.advance();
                let cond = self.parse_expr()?;
                let body = self.parse_block()?;
                elif_clauses.push((cond, body));
            } else if kind == TokenKind::Else {
                self.advance();
                self.skip_newlines();
                if self.peek().kind == TokenKind::If {
                    self.advance();
                    let cond = self.parse_expr()?;
                    let body = self.parse_block()?;
                    elif_clauses.push((cond, body));
                } else {
                    else_body = Some(self.parse_block()?);
                    break;
                }
            } else {
                break;
            }
        }

        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::If {
                condition,
                then_body,
                elif_clauses,
                else_body,
            },
            span,
        ))
    }

    pub(crate) fn parse_while(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::While)?;
        let condition = self.parse_expr()?;
        let body = self.parse_block()?;
        self.skip_newlines();
        let else_body = if self.peek().kind == TokenKind::Else {
            self.advance();
            Some(self.parse_block()?)
        } else {
            None
        };
        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::While {
                condition,
                body,
                else_body,
            },
            span,
        ))
    }

    pub(crate) fn parse_for(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::For)?;
        let target = self.parse_for_target()?;
        self.expect(TokenKind::In)?;
        let iter = self.parse_expr()?;
        let body = self.parse_block()?;
        self.skip_newlines();
        let else_body = if self.peek().kind == TokenKind::Else {
            self.advance();
            Some(self.parse_block()?)
        } else {
            None
        };
        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::For {
                target,
                iter,
                body,
                else_body,
            },
            span,
        ))
    }

    pub(crate) fn parse_for_target(&mut self) -> ParseResult<ForTarget> {
        use crate::span::Span;
        if self.peek().kind == TokenKind::LParen {
            self.advance();
            let mut names = Vec::new();
            let tok = self.expect(TokenKind::Identifier)?;
            let sp = Span {
                file_id: tok.file_id,
                line: tok.line,
                col: tok.col,
                start: tok.span.0,
                end: tok.span.1,
            };
            names.push((tok.value, sp));
            while self.peek().kind == TokenKind::Comma {
                self.advance();
                if self.peek().kind == TokenKind::RParen {
                    break;
                }
                let tok = self.expect(TokenKind::Identifier)?;
                let sp = Span {
                    file_id: tok.file_id,
                    line: tok.line,
                    col: tok.col,
                    start: tok.span.0,
                    end: tok.span.1,
                };
                names.push((tok.value, sp));
            }
            self.expect(TokenKind::RParen)?;
            return Ok(ForTarget::Tuple(names));
        }
        let tok = self.expect(TokenKind::Identifier)?;
        let name_span = Span {
            file_id: tok.file_id,
            line: tok.line,
            col: tok.col,
            start: tok.span.0,
            end: tok.span.1,
        };
        let name = tok.value;
        if self.peek().kind == TokenKind::Comma {
            let mut names = vec![(name, name_span)];
            while self.peek().kind == TokenKind::Comma {
                self.advance();
                if self.peek().kind == TokenKind::In {
                    break;
                }
                let tok = self.expect(TokenKind::Identifier)?;
                let sp = Span {
                    file_id: tok.file_id,
                    line: tok.line,
                    col: tok.col,
                    start: tok.span.0,
                    end: tok.span.1,
                };
                names.push((tok.value, sp));
            }
            Ok(ForTarget::Tuple(names))
        } else {
            Ok(ForTarget::Name(name, name_span))
        }
    }

    pub(crate) fn parse_with(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::With)?;

        let mut items = Vec::new();
        loop {
            let mut context_expr = self.parse_expr()?;
            let mut alias = None;

            if let ExprKind::Cast(inner, ty) = &context_expr.kind {
                if let crate::parser::ast::TypeExprKind::Name(name) = &ty.kind {
                    let alias_span = ty.span;
                    alias = Some(Expr::new(ExprKind::Identifier(name.clone()), alias_span));
                    context_expr = *inner.clone();
                }
            } else if self.peek().kind == TokenKind::As {
                self.advance();
                let ident = self.expect(TokenKind::Identifier)?;
                let span = self.span_from(&ident);
                alias = Some(Expr::new(ExprKind::Identifier(ident.value), span));
            }

            items.push(WithItem {
                context_expr,
                alias,
            });

            if self.peek().kind == TokenKind::Comma {
                self.advance();
                self.skip_newlines();
            } else {
                break;
            }
        }

        let body = self.parse_block()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::With { items, body }, span))
    }
}
