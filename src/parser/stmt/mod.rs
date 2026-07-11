use super::{
    Parser,
    ast::*,
    error::{ParseError, ParseResult},
};
use crate::lexer::{Token, TokenKind};

mod control;
mod functions;

#[cfg(test)]
mod tests;

impl Parser {
    pub(crate) fn parse_stmt(&mut self) -> ParseResult<Stmt> {
        match self.peek().kind {
            TokenKind::Fn => self.parse_fn(false),
            TokenKind::Async => self.parse_async_stmt(),
            TokenKind::Struct => self.parse_struct(),
            TokenKind::Impl => self.parse_impl(),
            TokenKind::Trait => self.parse_trait(),
            TokenKind::Enum => self.parse_enum(),
            TokenKind::If => self.parse_if(),
            TokenKind::While => self.parse_while(),
            TokenKind::For => self.parse_for(),
            TokenKind::Return => self.parse_return(),
            TokenKind::Assert => self.parse_assert(),
            TokenKind::Import => self.parse_import(),
            TokenKind::From => self.parse_from_import(),
            TokenKind::Let => self.parse_let(),
            TokenKind::Const => self.parse_const(),
            TokenKind::Identifier
                if self.peek().value == "type"
                    && self.peek_at(1).kind == TokenKind::Identifier
                    && self.peek_at(2).kind == TokenKind::Equal =>
            {
                self.parse_type_alias()
            }
            TokenKind::At | TokenKind::Hash => self.parse_decorated(),
            TokenKind::Unsafe => self.parse_unsafe_stmt(),
            TokenKind::Defer => self.parse_defer(),
            TokenKind::With => self.parse_with(),
            TokenKind::Pass => {
                let s = self.peek().clone();
                self.advance();
                self.eat_stmt_end()?;
                Ok(Stmt::new(StmtKind::Pass, self.span_from(&s)))
            }
            TokenKind::Break => {
                let s = self.peek().clone();
                self.advance();
                self.eat_stmt_end()?;
                Ok(Stmt::new(StmtKind::Break, self.span_from(&s)))
            }
            TokenKind::Continue => {
                let s = self.peek().clone();
                self.advance();
                self.eat_stmt_end()?;
                Ok(Stmt::new(StmtKind::Continue, self.span_from(&s)))
            }
            _ => self.parse_expr_or_assign(),
        }
    }

    pub(crate) fn parse_decorated(&mut self) -> ParseResult<Stmt> {
        let mut decorators = Vec::new();
        while self.peek().kind == TokenKind::At || self.peek().kind == TokenKind::Hash {
            if self.peek().kind == TokenKind::At {
                self.advance();
                let name_tok = self.expect(TokenKind::Identifier)?;
                decorators.push(Decorator {
                    name: name_tok.value.clone(),
                    is_directive: false,
                    span: Self::tok_span(&name_tok),
                });
            } else {
                self.advance();
                self.expect(TokenKind::LBracket)?;
                while self.peek().kind != TokenKind::RBracket {
                    let name_tok = self.expect(TokenKind::Identifier)?;
                    decorators.push(Decorator {
                        name: name_tok.value.clone(),
                        is_directive: true,
                        span: Self::tok_span(&name_tok),
                    });
                    if self.peek().kind == TokenKind::Comma {
                        self.advance();
                    } else if self.peek().kind == TokenKind::RBracket {
                        break;
                    } else {
                        return Err(self.err_at(
                            self.peek(),
                            format!(
                                "expected ',' or ']' in directive, found {:?}",
                                self.peek().kind
                            ),
                        ));
                    }
                }
                self.expect(TokenKind::RBracket)?;
            }
            self.skip_newlines();
        }

        if self.peek().kind == TokenKind::Async {
            let next_idx = self.pos + 1;
            if next_idx < self.tokens.len() && self.tokens[next_idx].kind == TokenKind::Fn {
                self.advance();
                let mut stmt = self.parse_fn(true)?;
                if let StmtKind::Fn { decorators: d, .. } = &mut stmt.kind {
                    *d = decorators;
                }
                return Ok(stmt);
            }
        }

        let mut stmt = self.parse_stmt()?;
        match &mut stmt.kind {
            StmtKind::Fn { decorators: d, .. }
            | StmtKind::Struct { decorators: d, .. }
            | StmtKind::Enum { decorators: d, .. } => {
                *d = decorators;
            }
            StmtKind::NativeImport { block_safe, .. } => {
                for d in &decorators {
                    if d.name == "safe" {
                        *block_safe = true;
                    } else {
                        return Err(self.err_at(
                            &self.tokens[self.pos],
                            format!(
                                "unknown decorator `@{}` on import; only `@safe` is allowed",
                                d.name
                            ),
                        ));
                    }
                }
            }
            _ => {
                if !decorators.is_empty() {
                    return Err(self.err_at(
                        &self.tokens[self.pos],
                        "decorators can only be applied to functions, structs, and enums",
                    ));
                }
            }
        }
        Ok(stmt)
    }

    pub(crate) fn parse_block(&mut self) -> ParseResult<Vec<Stmt>> {
        self.expect(TokenKind::Colon)?;
        if self.peek().kind == TokenKind::Newline {
            self.advance();
            self.expect(TokenKind::Indent)?;
            let mut stmts = Vec::new();
            self.skip_newlines();
            while self.peek().kind != TokenKind::Dedent && self.peek().kind != TokenKind::Eof {
                stmts.push(self.parse_stmt()?);
                self.skip_newlines();
            }
            self.expect(TokenKind::Dedent)?;
            Ok(stmts)
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    pub(crate) fn parse_async_stmt(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.advance();
        if self.peek().kind == TokenKind::Fn {
            self.parse_fn(true)
        } else if self.peek().kind == TokenKind::Colon {
            let body = self.parse_block()?;
            let span = self.span_from(&start);
            Ok(Stmt::new(
                StmtKind::ExprStmt(Expr::new(ExprKind::AsyncBlock(body), span)),
                span,
            ))
        } else {
            Err(self.err_at(
                &self.tokens[self.pos],
                "expected 'fn' or ':' after 'async' at statement level",
            ))
        }
    }

    pub(crate) fn parse_unsafe_stmt(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.advance();
        if self.peek().kind == TokenKind::Colon {
            let body = self.parse_block()?;
            let span = self.span_from(&start);
            Ok(Stmt::new(StmtKind::UnsafeBlock(body), span))
        } else {
            Err(self.err_at(&self.tokens[self.pos], "expected ':' after 'unsafe'"))
        }
    }

    pub(crate) fn parse_defer(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.advance();
        let expr = self.parse_expr()?;
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Defer(expr), span))
    }

    /// A bare `*name` element of a multi-target list (`a, *rest = xs`)
    /// parses through the ordinary expression grammar as `Deref(name)`,
    /// indistinguishable at the token level from a real pointer
    /// dereference (`*ptr = 5`). The two are disambiguated by arity: a
    /// lone target keeps its `Deref` meaning unchanged (`*ptr = 5` still
    /// writes through the pointer); only inside a genuine multi-target
    /// tuple does a bare `*name` become a starred gather target. Errors if
    /// more than one element would become starred.
    fn rewrite_starred_targets(lhs: &mut Expr, err_tok: &Token) -> ParseResult<()> {
        let ExprKind::Tuple(elems) = &mut lhs.kind else {
            return Ok(());
        };
        let mut seen = false;
        for elem in elems.iter_mut() {
            if let ExprKind::Deref(inner) = &elem.kind
                && matches!(inner.kind, ExprKind::Identifier(_))
            {
                if seen {
                    return Err(ParseError {
                        message: "at most one `*name` target is allowed".into(),
                        line: err_tok.line,
                        col: err_tok.col,
                        start: elem.span.start,
                        end: elem.span.end,
                    });
                }
                seen = true;
                let ExprKind::Deref(inner) = std::mem::replace(&mut elem.kind, ExprKind::Null)
                else {
                    unreachable!()
                };
                elem.kind = ExprKind::Starred(inner);
            }
        }
        Ok(())
    }

    pub(crate) fn parse_expr_or_assign(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        let mut lhs = self.parse_expr_list()?;
        Self::rewrite_starred_targets(&mut lhs, &start)?;
        let (op_line, op_col) = (self.peek().line, self.peek().col);
        match self.peek().kind.clone() {
            TokenKind::Equal => {
                if !Self::is_valid_assign_target(&lhs) {
                    return Err(ParseError {
                        message: "invalid assignment target".into(),
                        line: op_line,
                        col: op_col,
                        start: lhs.span.start,
                        end: lhs.span.end,
                    });
                }
                self.advance();
                let value = self.parse_expr_list()?;
                self.eat_stmt_end()?;
                let span = self.span_from(&start);
                Ok(Stmt::new(StmtKind::Assign { target: lhs, value }, span))
            }
            kind @ (TokenKind::PlusEqual
            | TokenKind::MinusEqual
            | TokenKind::StarEqual
            | TokenKind::SlashEqual
            | TokenKind::ShlEqual
            | TokenKind::ShrEqual
            | TokenKind::PercentEqual
            | TokenKind::DoubleStarEqual
            | TokenKind::PipeEqual
            | TokenKind::AmpersandEqual
            | TokenKind::CaretEqual) => {
                if !Self::is_valid_assign_target(&lhs) {
                    return Err(ParseError {
                        message: "invalid augmented assignment target".into(),
                        line: op_line,
                        col: op_col,
                        start: lhs.span.start,
                        end: lhs.span.end,
                    });
                }
                self.advance();
                let value = self.parse_expr()?;
                self.eat_stmt_end()?;
                let op = match kind {
                    TokenKind::PlusEqual => AugOp::Add,
                    TokenKind::MinusEqual => AugOp::Sub,
                    TokenKind::StarEqual => AugOp::Mul,
                    TokenKind::SlashEqual => AugOp::Div,
                    TokenKind::PercentEqual => AugOp::Mod,
                    TokenKind::DoubleStarEqual => AugOp::Pow,
                    TokenKind::ShlEqual => AugOp::Shl,
                    TokenKind::ShrEqual => AugOp::Shr,
                    TokenKind::PipeEqual => AugOp::BitOr,
                    TokenKind::AmpersandEqual => AugOp::BitAnd,
                    TokenKind::CaretEqual => AugOp::BitXor,
                    _ => unreachable!(),
                };
                let span = self.span_from(&start);
                Ok(Stmt::new(
                    StmtKind::AugAssign {
                        target: lhs,
                        op,
                        value,
                    },
                    span,
                ))
            }
            _ => {
                self.eat_stmt_end()?;
                let span = lhs.span;
                Ok(Stmt::new(StmtKind::ExprStmt(lhs), span))
            }
        }
    }

    pub(super) fn parse_type_params(&mut self) -> ParseResult<Vec<String>> {
        let mut params = Vec::new();
        if self.peek().kind == TokenKind::LBracket {
            self.advance();
            while self.peek().kind != TokenKind::RBracket && self.peek().kind != TokenKind::Eof {
                params.push(self.expect(TokenKind::Identifier)?.value);
                // An optional trait bound, e.g. `[T: Comparable]`. Generics are
                // resolved structurally, so the bound documents the requirement
                // the body already imposes; parse and accept it.
                if self.peek().kind == TokenKind::Colon {
                    self.advance();
                    self.parse_type_expr()?;
                }
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(TokenKind::RBracket)?;
        }
        Ok(params)
    }
}
