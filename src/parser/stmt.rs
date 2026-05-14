use super::{
    Parser,
    ast::*,
    error::{ParseError, ParseResult},
};
use crate::lexer::TokenKind;

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
            TokenKind::At | TokenKind::Hash => self.parse_decorated(),
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
                let name = self.expect(TokenKind::Identifier)?.value;
                decorators.push(Decorator {
                    name,
                    is_directive: false,
                });
            } else {
                self.advance(); // '#'
                self.expect(TokenKind::LBracket)?;
                while self.peek().kind != TokenKind::RBracket {
                    let name = self.expect(TokenKind::Identifier)?.value;
                    decorators.push(Decorator {
                        name,
                        is_directive: true,
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

        // decorated async fn
        if self.peek().kind == TokenKind::Async {
            let next_idx = self.pos + 1;
            if next_idx < self.tokens.len() && self.tokens[next_idx].kind == TokenKind::Fn {
                self.advance(); // consume `async`
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
        self.advance(); // consume `async`
        if self.peek().kind == TokenKind::Fn {
            self.parse_fn(true)
        } else if self.peek().kind == TokenKind::Colon {
            // async: block (expression, but used as statement)
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

    pub(crate) fn parse_fn(&mut self, is_async: bool) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Fn)?;
        let name = self.expect(TokenKind::Identifier)?.value;
        let type_params = self.parse_type_params()?;
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
                type_params,
                params,
                return_type,
                body,
                decorators: Vec::new(),
                is_async,
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

    pub(crate) fn parse_struct(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Struct)?;
        let name = self.expect(TokenKind::Identifier)?.value;
        let type_params = self.parse_type_params()?;
        if let Some(first_char) = name.chars().next()
            && !first_char.is_uppercase()
        {
            return Err(self.err_at(&start, "struct names must be capitalized"));
        }
        // fields
        self.expect(TokenKind::Colon)?;
        let mut fields: Vec<Param> = Vec::new();
        let mut body: Vec<Stmt> = Vec::new();
        if self.peek().kind == TokenKind::Newline {
            self.advance();
            self.expect(TokenKind::Indent)?;
            self.skip_newlines();
            while self.peek().kind != TokenKind::Dedent && self.peek().kind != TokenKind::Eof {
                // each line is either `name: Type` or a const/pass
                if self.peek().kind == TokenKind::Identifier && {
                    // check for field
                    let next_idx = self.pos + 1;
                    next_idx < self.tokens.len() && self.tokens[next_idx].kind == TokenKind::Colon
                } {
                    let field_start = self.peek().clone();
                    let field_name = self.expect(TokenKind::Identifier)?.value;
                    self.expect(TokenKind::Colon)?;
                    let type_ann = Some(self.parse_type_expr()?);
                    // optional default
                    let default = if self.peek().kind == TokenKind::Equal {
                        self.advance();
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    self.eat_stmt_end()?;
                    let span = self.span_from(&field_start);
                    fields.push(Param {
                        name: field_name,
                        type_ann,
                        default,
                        kind: ParamKind::Regular,
                        is_mut: false,
                        span,
                    });
                } else {
                    body.push(self.parse_stmt()?);
                }
                self.skip_newlines();
            }
            self.expect(TokenKind::Dedent)?;
        } else {
            // empty struct
            self.eat_stmt_end()?;
        }
        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::Struct {
                name,
                type_params,
                fields,
                body,
                decorators: Vec::new(),
            },
            span,
        ))
    }

    pub(crate) fn parse_impl(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Impl)?;
        let type_params = self.parse_type_params()?;
        let first_name = self.expect(TokenKind::Identifier)?.value;
        // `impl Trait for Type:` vs `impl Type:`
        let (trait_name, type_name) = if self.peek().kind == TokenKind::For {
            self.advance(); // consume `for`
            let ty = self.expect(TokenKind::Identifier)?.value;
            (Some(first_name), ty)
        } else {
            (None, first_name)
        };
        let body = self.parse_block()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::Impl {
                type_params,
                trait_name,
                type_name,
                body,
            },
            span,
        ))
    }

    pub(crate) fn parse_trait(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Trait)?;
        let name = self.expect(TokenKind::Identifier)?.value;
        let type_params = self.parse_type_params()?;
        let raw_body = self.parse_block()?;
        let mut methods = Vec::new();
        for s in raw_body {
            match &s.kind {
                StmtKind::Fn { .. } | StmtKind::Pass => {}
                _ => return Err(self.err_at(&start, "expected fn or pass in trait body")),
            }
            if matches!(s.kind, StmtKind::Fn { .. }) {
                methods.push(s);
            }
        }
        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::Trait {
                name,
                type_params,
                methods,
            },
            span,
        ))
    }

    pub(crate) fn parse_enum(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Enum)?;
        let name = self.expect(TokenKind::Identifier)?.value;
        let type_params = self.parse_type_params()?;
        if let Some(first_char) = name.chars().next()
            && !first_char.is_uppercase()
        {
            return Err(self.err_at(&start, "enum names must be capitalized"));
        }
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
                    while self.peek().kind != TokenKind::RParen
                        && self.peek().kind != TokenKind::Eof
                    {
                        types.push(self.parse_type_expr()?);
                        if self.peek().kind == TokenKind::Comma {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.expect(TokenKind::RParen)?;
                }
                variants.push(EnumVariant {
                    name: v_name,
                    types,
                });
                self.skip_newlines();
            }
            self.expect(TokenKind::Dedent)?;
        } else {
            return Err(self.err_at(
                &self.tokens[self.pos],
                "expected newline and indented block for enum",
            ));
        }

        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::Enum {
                name,
                type_params,
                variants,
                decorators: Vec::new(),
            },
            span,
        ))
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
        if self.peek().kind == TokenKind::String {
            let path = self.advance().value.clone();
            self.expect(TokenKind::As)?;
            let alias = self.expect(TokenKind::Identifier)?.value;
            self.eat_stmt_end()?;
            let span = self.span_from(&start);
            return Ok(Stmt::new(StmtKind::NativeImport { path, alias }, span));
        }
        let mut module = vec![self.expect(TokenKind::Identifier)?.value];
        while self.peek().kind == TokenKind::Dot {
            self.advance();
            module.push(self.expect(TokenKind::Identifier)?.value);
        }
        let alias = if self.peek().kind == TokenKind::As {
            self.advance();
            Some(self.expect(TokenKind::Identifier)?.value)
        } else {
            None
        };
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Import { module, alias }, span))
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
        let mut names = Vec::new();
        loop {
            let name = self.expect(TokenKind::Identifier)?.value;
            let alias = if self.peek().kind == TokenKind::As {
                self.advance();
                Some(self.expect(TokenKind::Identifier)?.value)
            } else {
                None
            };
            names.push((name, alias));
            if self.peek().kind == TokenKind::Comma {
                self.advance();
            } else {
                break;
            }
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
            | TokenKind::ShlEqual
            | TokenKind::ShrEqual
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
                    TokenKind::PercentEqual => AugOp::Mod,
                    TokenKind::DoubleStarEqual => AugOp::Pow,
                    TokenKind::ShlEqual => AugOp::Shl,
                    TokenKind::ShrEqual => AugOp::Shr,
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

    fn parse_type_params(&mut self) -> ParseResult<Vec<String>> {
        let mut params = Vec::new();
        if self.peek().kind == TokenKind::LBracket {
            self.advance();
            while self.peek().kind != TokenKind::RBracket && self.peek().kind != TokenKind::Eof {
                params.push(self.expect(TokenKind::Identifier)?.value);
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
