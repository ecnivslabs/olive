use super::{Parser, ast::*, error::ParseResult};
use crate::lexer::TokenKind;

impl Parser {
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
        self.expect(TokenKind::Colon)?;
        let mut fields: Vec<Param> = Vec::new();
        let mut body: Vec<Stmt> = Vec::new();
        if self.peek().kind == TokenKind::Newline {
            self.advance();
            self.expect(TokenKind::Indent)?;
            self.skip_newlines();
            while self.peek().kind != TokenKind::Dedent && self.peek().kind != TokenKind::Eof {
                if self.peek().kind == TokenKind::Identifier && {
                    let next_idx = self.pos + 1;
                    next_idx < self.tokens.len() && self.tokens[next_idx].kind == TokenKind::Colon
                } {
                    let field_start = self.peek().clone();
                    let field_name = self.expect(TokenKind::Identifier)?.value;
                    self.expect(TokenKind::Colon)?;
                    let type_ann = Some(self.parse_type_expr()?);
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
        let (trait_name, type_name) = if self.peek().kind == TokenKind::For {
            self.advance();
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
}
