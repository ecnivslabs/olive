use super::{
    Parser,
    ast::*,
    error::{ParseError, ParseResult},
};
use crate::lexer::TokenKind;

impl Parser {
    pub(crate) fn parse_stmt(&mut self) -> ParseResult<Stmt> {
        match self.peek().kind {
            TokenKind::Fn => self.parse_fn(),
            TokenKind::Class => self.parse_class(),
            TokenKind::Enum => self.parse_enum(),
            TokenKind::If => self.parse_if(),
            TokenKind::While => self.parse_while(),
            TokenKind::For => self.parse_for(),
            TokenKind::Try => self.parse_try(),
            TokenKind::Return => self.parse_return(),
            TokenKind::Raise => self.parse_raise(),
            TokenKind::Assert => self.parse_assert(),
            TokenKind::Import => self.parse_import(),
            TokenKind::From => self.parse_from_import(),
            TokenKind::Let => self.parse_let(),
            TokenKind::Const => self.parse_const(),
            TokenKind::At => self.parse_decorated(),
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
        while self.peek().kind == TokenKind::At {
            self.advance();
            let name = self.expect(TokenKind::Identifier)?.value;
            decorators.push(name);
            self.skip_newlines();
        }

        let mut stmt = self.parse_stmt()?;
        if let StmtKind::Fn {
            decorators: ref mut d,
            ..
        } = stmt.kind
        {
            *d = decorators;
        } else {
            return Err(self.err_at(
                &self.tokens[self.pos],
                "decorators can only be applied to functions",
            ));
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

    pub(crate) fn parse_fn(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Fn)?;
        let name = self.expect(TokenKind::Identifier)?.value;
        self.expect(TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(TokenKind::RParen)?;
        let return_type = if self.peek().kind == TokenKind::Arrow {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::Fn {
                name,
                params,
                return_type,
                body,
                decorators: Vec::new(),
            },
            span,
        ))
    }

    pub(crate) fn parse_params(&mut self) -> ParseResult<Vec<Param>> {
        let mut params = Vec::new();
        while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
            let param_start = self.peek().clone();
            let kind = match self.peek().kind {
                TokenKind::DoubleStar => {
                    self.advance();
                    ParamKind::KwArg
                }
                TokenKind::Star => {
                    self.advance();
                    ParamKind::VarArg
                }
                _ => ParamKind::Regular,
            };
            let mut is_mut = false;
            if self.peek().kind == TokenKind::Mut {
                self.advance();
                is_mut = true;
            }
            let name = self.expect(TokenKind::Identifier)?.value;
            let type_ann = if self.peek().kind == TokenKind::Colon {
                self.advance();
                Some(self.parse_type_expr()?)
            } else {
                None
            };
            let default = if kind == ParamKind::Regular && self.peek().kind == TokenKind::Equal {
                self.advance();
                Some(self.parse_expr()?)
            } else {
                None
            };
            let span = self.span_from(&param_start);
            params.push(Param {
                name,
                type_ann,
                default,
                kind,
                is_mut,
                span,
            });
            if self.peek().kind == TokenKind::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(params)
    }

    pub(crate) fn parse_class(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Class)?;
        let name = self.expect(TokenKind::Identifier)?.value;
        let bases = if self.peek().kind == TokenKind::LParen {
            self.advance();
            let mut bases = Vec::new();
            while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
                bases.push(self.parse_expr()?);
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(TokenKind::RParen)?;
            bases
        } else {
            Vec::new()
        };
        let body = self.parse_block()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Class { name, bases, body }, span))
    }

    pub(crate) fn parse_enum(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Enum)?;
        let name = self.expect(TokenKind::Identifier)?.value;
        self.expect(TokenKind::Colon)?;
        
        let mut variants = Vec::new();
        
        if self.peek().kind == TokenKind::Newline {
            self.advance();
            self.expect(TokenKind::Indent)?;
            self.skip_newlines();
            while self.peek().kind != TokenKind::Dedent && self.peek().kind != TokenKind::Eof {
                let v_name = self.expect(TokenKind::Identifier)?.value;
                let mut types = Vec::new();
                if self.peek().kind == TokenKind::LParen {
                    self.advance();
                    while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
                        types.push(self.parse_type_expr()?);
                        if self.peek().kind == TokenKind::Comma {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.expect(TokenKind::RParen)?;
                }
                variants.push(EnumVariant { name: v_name, types });
                self.skip_newlines();
            }
            self.expect(TokenKind::Dedent)?;
        } else {
            return Err(self.err_at(&self.tokens[self.pos], "expected newline and indented block for enum"));
        }

        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Enum { name, variants }, span))
    }

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
        let outer_start = self.peek().clone();
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
            let span = self.span_from(&outer_start);
            return Ok(ForTarget::Tuple(names, span));
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
            let span = self.span_from(&outer_start);
            Ok(ForTarget::Tuple(names, span))
        } else {
            Ok(ForTarget::Name(name, name_span))
        }
    }

    pub(crate) fn parse_try(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Try)?;
        let body = self.parse_block()?;

        let mut handlers = Vec::new();
        let mut else_body: Option<Vec<Stmt>> = None;
        let mut finally_body: Option<Vec<Stmt>> = None;

        self.skip_newlines();

        while self.peek().kind == TokenKind::Except {
            let handler_start = self.peek().clone();
            self.advance();
            let exc_type = if self.peek().kind != TokenKind::Colon {
                Some(self.parse_expr()?)
            } else {
                None
            };
            let name = if self.peek().kind == TokenKind::As {
                self.advance();
                Some(self.expect(TokenKind::Identifier)?.value)
            } else {
                None
            };
            let handler_body = self.parse_block()?;
            let handler_span = self.span_from(&handler_start);
            handlers.push(ExceptHandler {
                exc_type,
                name,
                body: handler_body,
                span: handler_span,
            });
            self.skip_newlines();
        }

        if self.peek().kind == TokenKind::Else {
            self.advance();
            else_body = Some(self.parse_block()?);
            self.skip_newlines();
        }

        if self.peek().kind == TokenKind::Finally {
            self.advance();
            finally_body = Some(self.parse_block()?);
        }

        if handlers.is_empty() && finally_body.is_none() {
            let tok = self.peek().clone();
            return Err(ParseError {
                message: "try without except or finally".into(),
                line: tok.line,
                col: tok.col,
                start: tok.span.0,
                end: tok.span.1,
            });
        }

        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::Try {
                body,
                handlers,
                else_body,
                finally_body,
            },
            span,
        ))
    }

    pub(crate) fn parse_return(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Return)?;
        let value = match self.peek().kind {
            TokenKind::Newline | TokenKind::Semicolon | TokenKind::Eof | TokenKind::Dedent => None,
            _ => Some(self.parse_expr()?),
        };
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Return(value), span))
    }

    pub(crate) fn parse_raise(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Raise)?;
        let value = match self.peek().kind {
            TokenKind::Newline | TokenKind::Semicolon | TokenKind::Eof | TokenKind::Dedent => None,
            _ => Some(self.parse_expr()?),
        };
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Raise(value), span))
    }

    pub(crate) fn parse_assert(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Assert)?;
        let test = self.parse_expr()?;
        let msg = if self.peek().kind == TokenKind::Comma {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Assert { test, msg }, span))
    }

    pub(crate) fn parse_import(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Import)?;
        let mut path = vec![self.expect(TokenKind::Identifier)?.value];
        while self.peek().kind == TokenKind::Dot {
            self.advance();
            path.push(self.expect(TokenKind::Identifier)?.value);
        }
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Import(path), span))
    }

    pub(crate) fn parse_from_import(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::From)?;
        let mut module = vec![self.expect(TokenKind::Identifier)?.value];
        while self.peek().kind == TokenKind::Dot {
            self.advance();
            module.push(self.expect(TokenKind::Identifier)?.value);
        }
        self.expect(TokenKind::Import)?;
        let mut names = vec![self.expect(TokenKind::Identifier)?.value];
        while self.peek().kind == TokenKind::Comma {
            self.advance();
            names.push(self.expect(TokenKind::Identifier)?.value);
        }
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::FromImport { module, names }, span))
    }

    pub(crate) fn parse_let(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Let)?;
        let mut is_mut = false;
        if self.peek().kind == TokenKind::Mut {
            self.advance();
            is_mut = true;
        }
        let name = self.expect(TokenKind::Identifier)?.value;
        let type_ann = if self.peek().kind == TokenKind::Colon {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::Equal)?;
        let value = self.parse_expr()?;
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::Let {
                name,
                type_ann,
                value,
                is_mut,
            },
            span,
        ))
    }

    pub(crate) fn parse_const(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Const)?;
        let name = self.expect(TokenKind::Identifier)?.value;
        let type_ann = if self.peek().kind == TokenKind::Colon {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::Equal)?;
        let value = self.parse_expr()?;
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::Const {
                name,
                type_ann,
                value,
            },
            span,
        ))
    }

    pub(crate) fn parse_expr_or_assign(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        let lhs = self.parse_expr_list()?;
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
            | TokenKind::DoubleSlashEqual
            | TokenKind::PercentEqual
            | TokenKind::DoubleStarEqual) => {
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
                    TokenKind::DoubleSlashEqual => AugOp::FloorDiv,
                    TokenKind::PercentEqual => AugOp::Mod,
                    TokenKind::DoubleStarEqual => AugOp::Pow,
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
}
