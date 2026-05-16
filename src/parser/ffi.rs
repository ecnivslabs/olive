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
                let w: u8 = self.expect(TokenKind::Integer)?.value.parse().unwrap_or(0);
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
        let raw: i64 = self.expect(TokenKind::Integer)?.value.parse().unwrap_or(0);
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
        let name = self.expect(TokenKind::Identifier)?.value;
        self.expect(TokenKind::LParen)?;
        let mut params = Vec::new();
        let mut is_vararg = false;
        while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
            if self.peek().kind == TokenKind::Star
                || (self.peek().kind == TokenKind::Dot && self.peek_at(1).kind == TokenKind::Dot)
            {
                if self.peek().kind == TokenKind::Dot {
                    self.advance();
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
        })
    }
}
