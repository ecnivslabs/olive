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
        let first_ty = self.parse_type_expr()?;
        let (trait_name, type_name) = if self.peek().kind == TokenKind::For {
            self.advance();
            let ty = self.parse_type_expr()?;
            (Some(first_ty), ty)
        } else {
            (None, first_ty)
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

        self.expect(TokenKind::Colon)?;

        let mut variants = Vec::new();
        let mut body = Vec::new();

        if self.peek().kind == TokenKind::Newline {
            self.advance();
            self.expect(TokenKind::Indent)?;
            self.skip_newlines();
            while self.peek().kind != TokenKind::Dedent && self.peek().kind != TokenKind::Eof {
                if self.peek().kind == TokenKind::Fn {
                    body.push(self.parse_stmt()?);
                } else {
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

                    let value = if self.peek().kind == TokenKind::Equal {
                        self.advance();
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };

                    variants.push(EnumVariant {
                        name: v_name,
                        types,
                        value,
                    });

                    if self.peek().kind == TokenKind::Comma {
                        self.advance();
                    } else if self.peek().kind != TokenKind::Newline
                        && self.peek().kind != TokenKind::Dedent
                        && self.peek().kind != TokenKind::Eof
                    {
                        return Err(self.err_at(
                            &self.tokens[self.pos],
                            "expected newline or comma after enum variant",
                        ));
                    }
                }
                self.skip_newlines();
            }
            self.expect(TokenKind::Dedent)?;
        } else {
            self.eat_stmt_end()?;
        }

        let span = self.span_from(&start);
        Ok(Stmt::new(
            StmtKind::Enum {
                name,
                type_params,
                variants,
                body,
                decorators: Vec::new(),
            },
            span,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_parser(src: &str) -> Parser {
        let tokens = crate::lexer::Lexer::new(src, 0)
            .tokenise()
            .expect("lex error");
        Parser::new(tokens)
    }

    #[test]
    fn parse_fn_no_args_no_return() {
        let mut p = make_parser("fn foo(): pass\n");
        let stmt = p.parse_fn(false).expect("parse failed");
        match &stmt.kind {
            StmtKind::Fn {
                name,
                params,
                return_type,
                body,
                is_async,
                ..
            } => {
                assert_eq!(name, "foo");
                assert!(params.is_empty());
                assert!(return_type.is_none());
                assert_eq!(body.len(), 1);
                assert!(!is_async);
            }
            _ => panic!("expected Fn"),
        }
    }

    #[test]
    fn parse_fn_with_return_type() {
        let mut p = make_parser("fn foo() -> i64: pass\n");
        let stmt = p.parse_fn(false).expect("parse failed");
        match &stmt.kind {
            StmtKind::Fn { return_type, .. } => {
                assert!(
                    matches!(return_type, Some(TypeExpr { kind: TypeExprKind::Name(t), .. }) if t == "i64")
                );
            }
            _ => panic!("expected Fn"),
        }
    }

    #[test]
    fn parse_fn_async() {
        let mut p = make_parser("fn foo(): pass\n");
        let stmt = p.parse_fn(true).expect("parse failed");
        match &stmt.kind {
            StmtKind::Fn { name, is_async, .. } => {
                assert_eq!(name, "foo");
                assert!(*is_async);
            }
            _ => panic!("expected Fn"),
        }
    }

    #[test]
    fn parse_params_with_vararg_and_kwarg() {
        let mut p = make_parser("fn f(a: i64, *args, **kwargs): pass\n");
        let stmt = p.parse_fn(false).expect("parse failed");
        match &stmt.kind {
            StmtKind::Fn { params, .. } => {
                assert_eq!(params.len(), 3);
                assert_eq!(params[0].kind, ParamKind::Regular);
                assert_eq!(params[1].kind, ParamKind::VarArg);
                assert_eq!(params[2].kind, ParamKind::KwArg);
            }
            _ => panic!("expected Fn"),
        }
    }

    #[test]
    fn parse_params_with_defaults() {
        let mut p = make_parser("fn f(x: i64 = 42): pass\n");
        let stmt = p.parse_fn(false).expect("parse failed");
        match &stmt.kind {
            StmtKind::Fn { params, .. } => {
                assert_eq!(params.len(), 1);
                assert!(params[0].default.is_some());
                assert_eq!(params[0].name, "x");
            }
            _ => panic!("expected Fn"),
        }
    }

    #[test]
    fn parse_fn_body() {
        let mut p = make_parser("fn foo():\n    pass\n    pass\n");
        let stmt = p.parse_fn(false).expect("parse failed");
        match &stmt.kind {
            StmtKind::Fn { body, .. } => {
                assert_eq!(body.len(), 2);
            }
            _ => panic!("expected Fn"),
        }
    }

    #[test]
    fn parse_struct_with_fields() {
        let mut p = make_parser("struct Point:\n    x: i64\n    y: i64\n");
        let stmt = p.parse_struct().expect("parse failed");
        match &stmt.kind {
            StmtKind::Struct {
                name, fields, body, ..
            } => {
                assert_eq!(name, "Point");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "x");
                assert!(body.is_empty());
            }
            _ => panic!("expected Struct"),
        }
    }

    #[test]
    fn parse_struct_with_default() {
        let mut p = make_parser("struct Point:\n    x: i64 = 0\n    y: i64 = 0\n");
        let stmt = p.parse_struct().expect("parse failed");
        match &stmt.kind {
            StmtKind::Struct { fields, .. } => {
                assert!(fields[0].default.is_some());
                assert!(fields[1].default.is_some());
            }
            _ => panic!("expected Struct"),
        }
    }

    #[test]
    fn parse_struct_with_methods() {
        let mut p = make_parser("struct Counter:\n    val: i64\n    fn inc(self): pass\n");
        let stmt = p.parse_struct().expect("parse failed");
        match &stmt.kind {
            StmtKind::Struct { fields, body, .. } => {
                assert_eq!(fields.len(), 1);
                assert_eq!(body.len(), 1);
                assert!(matches!(body[0].kind, StmtKind::Fn { .. }));
            }
            _ => panic!("expected Struct"),
        }
    }

    #[test]
    fn parse_impl_block() {
        let mut p = make_parser("impl MyStruct:\n    fn f(): pass\n");
        let stmt = p.parse_impl().expect("parse failed");
        match &stmt.kind {
            StmtKind::Impl {
                type_name,
                body,
                trait_name,
                ..
            } => {
                assert!(matches!(&type_name.kind, TypeExprKind::Name(n) if n == "MyStruct"));
                assert!(trait_name.is_none());
                assert_eq!(body.len(), 1);
            }
            _ => panic!("expected Impl"),
        }
    }

    #[test]
    fn parse_impl_with_trait() {
        let mut p = make_parser("impl Display for MyStruct:\n    fn f(): pass\n");
        let stmt = p.parse_impl().expect("parse failed");
        match &stmt.kind {
            StmtKind::Impl {
                type_name,
                trait_name,
                ..
            } => {
                assert!(trait_name.is_some());
                assert!(matches!(&type_name.kind, TypeExprKind::Name(n) if n == "MyStruct"));
            }
            _ => panic!("expected Impl"),
        }
    }

    #[test]
    fn parse_trait_block() {
        let mut p = make_parser("trait Foo:\n    fn f(self): pass\n");
        let stmt = p.parse_trait().expect("parse failed");
        match &stmt.kind {
            StmtKind::Trait { name, methods, .. } => {
                assert_eq!(name, "Foo");
                assert_eq!(methods.len(), 1);
            }
            _ => panic!("expected Trait"),
        }
    }

    #[test]
    fn parse_enum_simple() {
        let mut p = make_parser("enum Color:\n    Red\n    Green\n    Blue\n");
        let stmt = p.parse_enum().expect("parse failed");
        match &stmt.kind {
            StmtKind::Enum { name, variants, .. } => {
                assert_eq!(name, "Color");
                assert_eq!(variants.len(), 3);
                assert_eq!(variants[0].name, "Red");
            }
            _ => panic!("expected Enum"),
        }
    }

    #[test]
    fn parse_enum_with_values() {
        let mut p = make_parser("enum Http:\n    OK = 200\n    NotFound = 404\n");
        let stmt = p.parse_enum().expect("parse failed");
        match &stmt.kind {
            StmtKind::Enum { variants, .. } => {
                assert_eq!(variants.len(), 2);
                assert!(variants[0].value.is_some());
                assert!(variants[1].value.is_some());
            }
            _ => panic!("expected Enum"),
        }
    }

    #[test]
    fn parse_enum_with_tuple_variants() {
        let mut p = make_parser("enum Opt:\n    Some(i64)\n    Nil\n");
        let stmt = p.parse_enum().expect("parse failed");
        match &stmt.kind {
            StmtKind::Enum { variants, .. } => {
                assert_eq!(variants.len(), 2);
                assert_eq!(variants[0].types.len(), 1);
                assert!(variants[1].types.is_empty());
            }
            _ => panic!("expected Enum"),
        }
    }
}
