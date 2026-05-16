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
            TokenKind::Unsafe => self.parse_unsafe_stmt(),
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
                self.advance();
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
            if self.peek().kind == TokenKind::Colon && self.peek_at(1).kind == TokenKind::Newline {
                self.advance();
                self.advance();
                self.expect(TokenKind::Indent)?;
                self.skip_newlines();
                let mut functions = Vec::new();
                let mut structs = Vec::new();
                let mut vars = Vec::new();
                let mut consts = Vec::new();
                while self.peek().kind != TokenKind::Dedent && self.peek().kind != TokenKind::Eof {
                    if self.peek().kind == TokenKind::Struct {
                        structs.push(self.parse_ffi_struct_def(false)?);
                    } else if self.peek().kind == TokenKind::Identifier
                        && self.peek().value == "union"
                        && self.peek_at(1).kind == TokenKind::Struct
                    {
                        self.advance();
                        structs.push(self.parse_ffi_struct_def(true)?);
                    } else if self.peek().kind == TokenKind::Identifier
                        && self.peek().value == "var"
                    {
                        vars.push(self.parse_ffi_var_def()?);
                    } else if self.peek().kind == TokenKind::Const {
                        consts.push(self.parse_ffi_const_def()?);
                    } else {
                        functions.push(self.parse_ffi_fn_sig()?);
                    }
                    self.skip_newlines();
                }
                self.expect(TokenKind::Dedent)?;
                let span = self.span_from(&start);
                return Ok(Stmt::new(
                    StmtKind::NativeImport {
                        path,
                        alias,
                        functions,
                        structs,
                        vars,
                        consts,
                        block_safe: false,
                    },
                    span,
                ));
            }
            self.eat_stmt_end()?;
            let span = self.span_from(&start);
            return Ok(Stmt::new(
                StmtKind::NativeImport {
                    path,
                    alias,
                    functions: Vec::new(),
                    structs: Vec::new(),
                    vars: Vec::new(),
                    consts: Vec::new(),
                    block_safe: false,
                },
                span,
            ));
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

    pub(super) fn parse_type_params(&mut self) -> ParseResult<Vec<String>> {
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
