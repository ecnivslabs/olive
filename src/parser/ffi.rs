use super::{Parser, ast::*, error::ParseResult};
use crate::lexer::TokenKind;

impl Parser {
    pub(crate) fn parse_ffi_struct_def(&mut self, is_union: bool) -> ParseResult<FfiStructDef> {
        self.expect(TokenKind::Struct)?;
        let name = self.expect(TokenKind::Identifier)?.value;
        let destructor =
            if self.peek().kind == TokenKind::Identifier && self.peek().value == "free_with" {
                self.advance();
                self.expect(TokenKind::LParen)?;
                let dtor = self.expect(TokenKind::Identifier)?.value;
                self.expect(TokenKind::RParen)?;
                Some(dtor)
            } else {
                None
            };
        self.expect(TokenKind::Colon)?;
        self.expect(TokenKind::Newline)?;
        self.expect(TokenKind::Indent)?;
        self.skip_newlines();
        let mut fields = Vec::new();
        while self.peek().kind != TokenKind::Dedent && self.peek().kind != TokenKind::Eof {
            let field_name = self.expect(TokenKind::Identifier)?.value;
            self.expect(TokenKind::Colon)?;
            let ty = self.parse_type_expr()?;
            let bits = if self.peek().kind == TokenKind::At {
                self.advance();
                let tok = self.expect(TokenKind::Integer)?;
                let w: u8 = tok.value.parse().map_err(|_| {
                    self.err_at(
                        &tok,
                        format!("invalid bitfield width `{}`: expected 0-255", tok.value),
                    )
                })?;
                Some(w)
            } else {
                None
            };
            fields.push(FfiStructField {
                name: field_name,
                ty,
                bits,
            });
            self.eat_stmt_end()?;
            self.skip_newlines();
        }
        self.expect(TokenKind::Dedent)?;
        Ok(FfiStructDef {
            name,
            fields,
            is_union,
            destructor,
        })
    }

    pub(crate) fn parse_ffi_var_def(&mut self) -> ParseResult<FfiVarDef> {
        self.advance();
        let name = self.expect(TokenKind::Identifier)?.value;
        self.expect(TokenKind::Colon)?;
        let ty = self.parse_type_expr()?;
        self.eat_stmt_end()?;
        Ok(FfiVarDef { name, ty })
    }

    pub(crate) fn parse_ffi_const_def(&mut self) -> ParseResult<FfiConstDef> {
        self.expect(TokenKind::Const)?;
        let name = self.expect(TokenKind::Identifier)?.value;
        if self.peek().kind == TokenKind::Colon {
            self.advance();
            self.parse_type_expr()?;
        }
        self.expect(TokenKind::Equal)?;
        let negative = if self.peek().kind == TokenKind::Minus {
            self.advance();
            true
        } else {
            false
        };
        let int_tok = self.expect(TokenKind::Integer)?;
        let raw: i64 = int_tok.value.parse().map_err(|_| {
            self.err_at(
                &int_tok,
                format!("invalid integer constant `{}`", int_tok.value),
            )
        })?;
        let value = if negative { -raw } else { raw };
        self.eat_stmt_end()?;
        Ok(FfiConstDef { name, value })
    }

    pub(crate) fn parse_ffi_fn_sig(&mut self) -> ParseResult<FfiFnSig> {
        let mut decorators = Vec::new();
        while self.peek().kind == TokenKind::At {
            self.advance();
            let name = self.expect(TokenKind::Identifier)?.value;
            decorators.push(Decorator {
                name,
                is_directive: false,
            });
            self.skip_newlines();
        }
        self.expect(TokenKind::Fn)?;
        let name_tok = self.expect(TokenKind::Identifier)?;
        let span = crate::span::Span {
            file_id: name_tok.file_id,
            line: name_tok.line,
            col: name_tok.col,
            start: name_tok.span.0,
            end: name_tok.span.1,
        };
        let name = name_tok.value;
        self.expect(TokenKind::LParen)?;
        let mut params = Vec::new();
        let mut is_vararg = false;
        while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
            if self.peek().kind == TokenKind::Star || self.peek().kind == TokenKind::DotDot {
                if self.peek().kind == TokenKind::DotDot {
                    // C variadic `...` lexes as `..` followed by a final `.`.
                    self.advance();
                    if self.peek().kind == TokenKind::Dot {
                        self.advance();
                    }
                } else {
                    self.advance();
                    self.expect(TokenKind::Identifier)?;
                }
                is_vararg = true;
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                }
                break;
            }
            self.expect(TokenKind::Identifier)?;
            self.expect(TokenKind::Colon)?;
            let ty = self.parse_type_expr()?;
            params.push(FfiParam { ty });
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
        let call_conv = decorators.iter().find_map(|d| match d.name.as_str() {
            "stdcall" | "fastcall" | "cdecl" => Some(d.name.clone()),
            _ => None,
        });
        Ok(FfiFnSig {
            name,
            params,
            ret,
            is_vararg,
            decorators,
            call_conv,
            span,
        })
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
    fn parse_ffi_fn_sig_basic() {
        let mut p = make_parser("fn foo(x: i32, y: f64) -> i32\n");
        let sig = p.parse_ffi_fn_sig().expect("parse failed");
        assert_eq!(sig.name, "foo");
        assert_eq!(sig.params.len(), 2);
        assert!(sig.ret.is_some());
        assert!(!sig.is_vararg);
    }

    #[test]
    fn parse_ffi_fn_sig_no_return() {
        let mut p = make_parser("fn foo(x: i32)\n");
        let sig = p.parse_ffi_fn_sig().expect("parse failed");
        assert!(sig.ret.is_none());
    }

    #[test]
    fn parse_ffi_fn_sig_vararg() {
        let mut p = make_parser("fn foo(x: i32, ...)\n");
        let sig = p.parse_ffi_fn_sig().expect("parse failed");
        assert!(sig.is_vararg);
    }

    #[test]
    fn parse_ffi_fn_sig_with_decorators() {
        let mut p = make_parser("@cdecl\nfn foo()\n");
        let sig = p.parse_ffi_fn_sig().expect("parse failed");
        assert_eq!(sig.decorators.len(), 1);
        assert_eq!(sig.decorators[0].name, "cdecl");
        assert_eq!(sig.call_conv, Some("cdecl".into()));
    }

    #[test]
    fn parse_ffi_struct_def_basic() {
        let mut p = make_parser("struct Point:\n    x: i32\n    y: i32\n");
        let def = p.parse_ffi_struct_def(false).expect("parse failed");
        assert_eq!(def.name, "Point");
        assert!(!def.is_union);
        assert_eq!(def.fields.len(), 2);
        assert!(def.destructor.is_none());
    }

    #[test]
    fn parse_ffi_struct_def_union() {
        let mut p = make_parser("struct Data:\n    a: i32\n    b: f64\n");
        let def = p.parse_ffi_struct_def(true).expect("parse failed");
        assert!(def.is_union);
    }

    #[test]
    fn parse_ffi_struct_def_with_destructor() {
        let mut p = make_parser("struct File free_with(free_file):\n    fd: i32\n");
        let def = p.parse_ffi_struct_def(false).expect("parse failed");
        assert_eq!(def.destructor, Some("free_file".into()));
    }

    #[test]
    fn parse_ffi_struct_def_with_bitfield() {
        let mut p = make_parser("struct Flags:\n    flag: i32 @1\n");
        let def = p.parse_ffi_struct_def(false).expect("parse failed");
        assert_eq!(def.fields[0].bits, Some(1));
    }

    #[test]
    fn parse_ffi_var_def() {
        let mut p = make_parser("var errno: i32\n");
        let def = p.parse_ffi_var_def().expect("parse failed");
        assert_eq!(def.name, "errno");
    }

    #[test]
    fn parse_ffi_const_def_positive() {
        let mut p = make_parser("const MAX = 100\n");
        let def = p.parse_ffi_const_def().expect("parse failed");
        assert_eq!(def.name, "MAX");
        assert_eq!(def.value, 100);
    }

    #[test]
    fn parse_ffi_const_def_negative() {
        let mut p = make_parser("const MIN = -50\n");
        let def = p.parse_ffi_const_def().expect("parse failed");
        assert_eq!(def.value, -50);
    }

    #[test]
    fn parse_ffi_bitfield_width_overflow_errors() {
        let mut p = make_parser("struct Flags:\n    flag: i32 @999\n");
        let err = p
            .parse_ffi_struct_def(false)
            .expect_err("expected width error");
        assert!(err.message.contains("invalid bitfield width"));
    }

    #[test]
    fn parse_ffi_const_overflow_errors() {
        let mut p = make_parser("const BIG = 99999999999999999999999\n");
        let err = p.parse_ffi_const_def().expect_err("expected const error");
        assert!(err.message.contains("invalid integer constant"));
    }
}
