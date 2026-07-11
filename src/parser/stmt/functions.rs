use super::super::{Parser, ast::*, error::ParseResult};
use crate::lexer::TokenKind;
use crate::span::Span;

impl Parser {
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
        if self.peek().kind == TokenKind::Identifier
            && self.peek().value == "py"
            && self.peek_at(1).kind == TokenKind::String
        {
            self.advance(); // consume `py`
            let module = self.advance().value.clone();
            self.expect(TokenKind::As)?;
            let alias = self.expect(TokenKind::Identifier)?.value;
            if self.peek().kind == TokenKind::Colon && self.peek_at(1).kind == TokenKind::Newline {
                self.advance(); // colon
                self.advance(); // newline
                self.expect(TokenKind::Indent)?;
                self.skip_newlines();
                let mut typed_types = Vec::new();
                let mut typed_fns = Vec::new();
                while self.peek().kind != TokenKind::Dedent && self.peek().kind != TokenKind::Eof {
                    if self.peek().kind == TokenKind::Identifier && self.peek().value == "type" {
                        self.advance();
                        let type_name = self.expect(TokenKind::Identifier)?.value;
                        typed_types.push(type_name);
                        self.eat_stmt_end()?;
                    } else if self.peek().kind == TokenKind::Fn {
                        typed_fns.push(self.parse_py_fn_sig()?);
                    } else {
                        let tok = self.peek().clone();
                        return Err(
                            self.err_at(&tok, "expected 'type' or 'fn' in Python import block")
                        );
                    }
                    self.skip_newlines();
                }
                self.expect(TokenKind::Dedent)?;
                let span = self.span_from(&start);
                return Ok(Stmt::new(
                    StmtKind::PyImport {
                        module,
                        alias,
                        typed_types,
                        typed_fns,
                    },
                    span,
                ));
            }
            self.eat_stmt_end()?;
            let span = self.span_from(&start);
            return Ok(Stmt::new(
                StmtKind::PyImport {
                    module,
                    alias,
                    typed_types: Vec::new(),
                    typed_fns: Vec::new(),
                },
                span,
            ));
        }
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
        let mut is_star = false;
        if self.peek().kind == TokenKind::Star {
            self.advance();
            is_star = true;
        } else {
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
        }
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::FromImport {
                module,
                names,
                is_star,
            },
            span,
        ))
    }

    fn parse_py_fn_sig(&mut self) -> ParseResult<PyFnSig> {
        self.expect(TokenKind::Fn)?;
        let name = self.expect(TokenKind::Identifier)?.value;
        self.expect(TokenKind::LParen)?;
        let mut params = Vec::new();
        while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
            // param names are optional: `x: float` or just `float`
            let ty = if self.peek().kind == TokenKind::Identifier
                && self.peek_at(1).kind == TokenKind::Colon
            {
                self.advance(); // name
                self.advance(); // colon
                self.parse_type_expr()?
            } else {
                self.parse_type_expr()?
            };
            params.push(ty);
            if self.peek().kind == TokenKind::Comma {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::RParen)?;
        let ret = if self.peek().kind == TokenKind::Arrow {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.eat_stmt_end()?;
        Ok(PyFnSig { name, params, ret })
    }

    pub(crate) fn parse_type_alias(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.advance(); // `type`
        let name_tok = self.expect(TokenKind::Identifier)?;
        let name_span = Self::tok_span(&name_tok);
        self.expect(TokenKind::Equal)?;
        let target = self.parse_type_expr()?;
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::TypeAlias {
                name: name_tok.value,
                name_span,
                target,
            },
            span,
        ))
    }

    pub(crate) fn parse_let(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Let)?;
        let mut is_mut = false;
        if self.peek().kind == TokenKind::Mut {
            self.advance();
            is_mut = true;
        }

        // `let (a, b) = t` parses identically to `let a, b = t`; the parens
        // are pure grouping with no effect on the resulting AST (E4.5).
        let parenthesized = self.peek().kind == TokenKind::LParen;
        if parenthesized {
            self.advance();
        }

        let mut starred: Option<usize> = None;
        let mut names: Vec<String> = Vec::new();
        let mut name_spans: Vec<Span> = Vec::new();
        loop {
            if self.peek().kind == TokenKind::Star {
                let star_tok = self.peek().clone();
                self.advance();
                if starred.is_some() {
                    return Err(self.err_at(&star_tok, "at most one `*name` target is allowed"));
                }
                starred = Some(names.len());
                let tok = self.expect(TokenKind::Identifier)?;
                name_spans.push(Self::tok_span(&star_tok).merge(Self::tok_span(&tok)));
                names.push(tok.value);
            } else {
                let tok = self.expect(TokenKind::Identifier)?;
                name_spans.push(Self::tok_span(&tok));
                names.push(tok.value);
            }
            if self.peek().kind != TokenKind::Comma {
                break;
            }
            self.advance();
        }

        if parenthesized {
            self.expect(TokenKind::RParen)?;
        }

        let type_ann = if self.peek().kind == TokenKind::Colon {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::Equal)?;
        let value = if names.len() > 1 {
            self.parse_expr_list()?
        } else {
            self.parse_expr()?
        };
        self.eat_stmt_end()?;
        let span = self.span_from(&start);

        if names.len() == 1 {
            Ok(Stmt::new(
                StmtKind::Let {
                    name: names.into_iter().next().unwrap(),
                    name_span: name_spans[0],
                    type_ann,
                    value,
                    is_mut,
                },
                span,
            ))
        } else {
            Ok(Stmt::new(
                StmtKind::MultiLet {
                    names,
                    name_spans,
                    type_ann,
                    value,
                    is_mut,
                    starred,
                },
                span,
            ))
        }
    }

    pub(crate) fn parse_const(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Const)?;

        let first = self.expect(TokenKind::Identifier)?;
        let mut name_spans = vec![Self::tok_span(&first)];
        let mut names = vec![first.value];
        while self.peek().kind == TokenKind::Comma {
            self.advance();
            let tok = self.expect(TokenKind::Identifier)?;
            name_spans.push(Self::tok_span(&tok));
            names.push(tok.value);
        }

        let type_ann = if self.peek().kind == TokenKind::Colon {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::Equal)?;
        let value = if names.len() > 1 {
            self.parse_expr_list()?
        } else {
            self.parse_expr()?
        };
        self.eat_stmt_end()?;
        let span = self.span_from(&start);

        if names.len() == 1 {
            Ok(Stmt::new(
                StmtKind::Const {
                    name: names.into_iter().next().unwrap(),
                    name_span: name_spans[0],
                    type_ann,
                    value,
                },
                span,
            ))
        } else {
            Ok(Stmt::new(
                StmtKind::MultiConst {
                    names,
                    name_spans,
                    type_ann,
                    value,
                },
                span,
            ))
        }
    }
}
