use super::super::{Parser, ast::*, error::ParseResult};
use crate::lexer::TokenKind;

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
            self.eat_stmt_end()?;
            let span = self.span_from(&start);
            return Ok(Stmt::new(StmtKind::PyImport { module, alias }, span));
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

    pub(crate) fn parse_let(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Let)?;
        let mut is_mut = false;
        if self.peek().kind == TokenKind::Mut {
            self.advance();
            is_mut = true;
        }

        let mut names = vec![self.expect(TokenKind::Identifier)?.value];
        while self.peek().kind == TokenKind::Comma {
            self.advance();
            names.push(self.expect(TokenKind::Identifier)?.value);
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
                    type_ann,
                    value,
                    is_mut,
                },
                span,
            ))
        }
    }

    pub(crate) fn parse_const(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Const)?;

        let mut names = vec![self.expect(TokenKind::Identifier)?.value];
        while self.peek().kind == TokenKind::Comma {
            self.advance();
            names.push(self.expect(TokenKind::Identifier)?.value);
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
                    type_ann,
                    value,
                },
                span,
            ))
        } else {
            Ok(Stmt::new(
                StmtKind::MultiConst {
                    names,
                    type_ann,
                    value,
                },
                span,
            ))
        }
    }
}
