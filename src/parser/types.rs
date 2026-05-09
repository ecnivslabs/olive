use super::{Parser, ast::*, error::ParseResult};
use crate::lexer::TokenKind;

impl Parser {
    pub(crate) fn parse_type_expr(&mut self) -> ParseResult<TypeExpr> {
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
                self.expect(TokenKind::RBracket)?;
                let span = self.span_from(&start);
                Ok(TypeExpr::new(TypeExprKind::List(Box::new(inner)), span))
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
            _ => {
                let tok = self.peek().clone();
                Err(self.err_at(&tok, "expected type expression"))
            }
        }
    }
}
