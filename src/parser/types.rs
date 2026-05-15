use super::{Parser, ast::*, error::ParseResult};
use crate::lexer::TokenKind;

impl Parser {
    pub(crate) fn parse_type_expr(&mut self) -> ParseResult<TypeExpr> {
        let mut left = self.parse_single_type_expr()?;
        while self.peek().kind == TokenKind::Pipe {
            self.advance();
            let right = self.parse_single_type_expr()?;
            let span = left.span.merge(right.span);
            left = TypeExpr::new(TypeExprKind::Union(Box::new(left), Box::new(right)), span);
        }
        Ok(left)
    }

    pub(crate) fn parse_single_type_expr(&mut self) -> ParseResult<TypeExpr> {
        let start = self.peek().clone();
        match self.peek().kind {
            TokenKind::Identifier => {
                let name = self.advance().value;
                if self.peek().kind == TokenKind::LBracket {
                    self.advance();
                    let mut args = Vec::new();
                    while self.peek().kind != TokenKind::RBracket {
                        args.push(self.parse_type_expr()?);
                        if self.peek().kind == TokenKind::Comma {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.expect(TokenKind::RBracket)?;
                    let span = self.span_from(&start);
                    Ok(TypeExpr::new(TypeExprKind::Generic(name, args), span))
                } else {
                    let span = self.span_from(&start);
                    Ok(TypeExpr::new(TypeExprKind::Name(name), span))
                }
            }
            TokenKind::LParen => {
                self.advance();
                let mut types = Vec::new();
                while self.peek().kind != TokenKind::RParen {
                    types.push(self.parse_type_expr()?);
                    if self.peek().kind == TokenKind::Comma {
                        self.advance();
                    } else {
                        break;
                    }
                }
                self.expect(TokenKind::RParen)?;
                let span = self.span_from(&start);
                Ok(TypeExpr::new(TypeExprKind::Tuple(types), span))
            }
            TokenKind::LBracket => {
                self.advance();
                let inner = self.parse_type_expr()?;
                if self.peek().kind == TokenKind::Semicolon {
                    self.advance();
                    let size_tok = self.expect(TokenKind::Integer)?;
                    let n = size_tok.value.parse::<usize>().unwrap_or(0);
                    self.expect(TokenKind::RBracket)?;
                    let span = self.span_from(&start);
                    Ok(TypeExpr::new(TypeExprKind::FixedArray(Box::new(inner), n), span))
                } else {
                    self.expect(TokenKind::RBracket)?;
                    let span = self.span_from(&start);
                    Ok(TypeExpr::new(TypeExprKind::List(Box::new(inner)), span))
                }
            }
            TokenKind::LBrace => {
                self.advance();
                let key = self.parse_type_expr()?;
                self.expect(TokenKind::Colon)?;
                let value = self.parse_type_expr()?;
                self.expect(TokenKind::RBrace)?;
                let span = self.span_from(&start);
                Ok(TypeExpr::new(
                    TypeExprKind::Dict(Box::new(key), Box::new(value)),
                    span,
                ))
            }
            TokenKind::Ampersand => {
                self.advance();
                if self.peek().kind == TokenKind::Mut {
                    self.advance();
                    let inner = self.parse_single_type_expr()?;
                    let span = self.span_from(&start);
                    Ok(TypeExpr::new(TypeExprKind::MutRef(Box::new(inner)), span))
                } else {
                    let inner = self.parse_single_type_expr()?;
                    let span = self.span_from(&start);
                    Ok(TypeExpr::new(TypeExprKind::Ref(Box::new(inner)), span))
                }
            }
            TokenKind::Star => {
                self.advance();
                let inner = self.parse_single_type_expr()?;
                let span = self.span_from(&start);
                Ok(TypeExpr::new(TypeExprKind::Ptr(Box::new(inner)), span))
            }
            TokenKind::Fn => {
                self.advance();
                self.expect(TokenKind::LParen)?;
                let mut params = Vec::new();
                while self.peek().kind != TokenKind::RParen {
                    params.push(self.parse_type_expr()?);
                    if self.peek().kind == TokenKind::Comma {
                        self.advance();
                    } else {
                        break;
                    }
                }
                self.expect(TokenKind::RParen)?;
                self.expect(TokenKind::Arrow)?;
                let ret = self.parse_type_expr()?;
                let span = self.span_from(&start);
                Ok(TypeExpr::new(
                    TypeExprKind::Fn {
                        params,
                        ret: Box::new(ret),
                    },
                    span,
                ))
            }
            _ => {
                let tok = self.peek().clone();
                Err(self.err_at(&tok, "expected type expression"))
            }
        }
    }
}
