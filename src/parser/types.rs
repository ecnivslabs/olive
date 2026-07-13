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
            TokenKind::Null => {
                self.advance();
                let span = self.span_from(&start);
                Ok(TypeExpr::new(TypeExprKind::Name("None".to_string()), span))
            }
            TokenKind::Identifier => {
                let name = self.advance().value;
                let mut parts = vec![name];
                while self.peek().kind == TokenKind::Dot {
                    self.advance();
                    parts.push(self.expect(TokenKind::Identifier)?.value);
                }
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
                    Ok(TypeExpr::new(
                        TypeExprKind::Generic(parts.join("."), args),
                        span,
                    ))
                } else if parts.len() > 1 {
                    let span = self.span_from(&start);
                    Ok(TypeExpr::new(TypeExprKind::Qualified(parts), span))
                } else {
                    let span = self.span_from(&start);
                    Ok(TypeExpr::new(TypeExprKind::Name(parts.remove(0)), span))
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
                    Ok(TypeExpr::new(
                        TypeExprKind::FixedArray(Box::new(inner), n),
                        span,
                    ))
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
    fn parse_type_name() {
        let mut p = make_parser("i64\n");
        let ty = p.parse_type_expr().expect("parse failed");
        assert!(matches!(ty.kind, TypeExprKind::Name(n) if n == "i64"));
    }

    #[test]
    fn parse_type_generic() {
        let mut p = make_parser("Option[i64]\n");
        let ty = p.parse_type_expr().expect("parse failed");
        match &ty.kind {
            TypeExprKind::Generic(name, args) => {
                assert_eq!(name, "Option");
                assert_eq!(args.len(), 1);
            }
            _ => panic!("expected Generic"),
        }
    }

    #[test]
    fn parse_type_tuple() {
        let mut p = make_parser("(i64, f64)\n");
        let ty = p.parse_type_expr().expect("parse failed");
        match &ty.kind {
            TypeExprKind::Tuple(types) => assert_eq!(types.len(), 2),
            _ => panic!("expected Tuple"),
        }
    }

    #[test]
    fn parse_type_list() {
        let mut p = make_parser("[i64]\n");
        let ty = p.parse_type_expr().expect("parse failed");
        assert!(matches!(ty.kind, TypeExprKind::List(_)));
    }

    #[test]
    fn parse_type_fixed_array() {
        let mut p = make_parser("[i64; 8]\n");
        let ty = p.parse_type_expr().expect("parse failed");
        match &ty.kind {
            TypeExprKind::FixedArray(inner, n) => {
                assert_eq!(*n, 8);
                assert!(matches!(&inner.kind, TypeExprKind::Name(t) if t == "i64"));
            }
            _ => panic!("expected FixedArray"),
        }
    }

    #[test]
    fn parse_type_dict() {
        let mut p = make_parser("{i64: f64}\n");
        let ty = p.parse_type_expr().expect("parse failed");
        assert!(matches!(ty.kind, TypeExprKind::Dict(_, _)));
    }

    #[test]
    fn parse_type_ref() {
        let mut p = make_parser("&i64\n");
        let ty = p.parse_type_expr().expect("parse failed");
        assert!(matches!(ty.kind, TypeExprKind::Ref(_)));
    }

    #[test]
    fn parse_type_mut_ref() {
        let mut p = make_parser("&mut i64\n");
        let ty = p.parse_type_expr().expect("parse failed");
        assert!(matches!(ty.kind, TypeExprKind::MutRef(_)));
    }

    #[test]
    fn parse_type_ptr() {
        let mut p = make_parser("*i64\n");
        let ty = p.parse_type_expr().expect("parse failed");
        assert!(matches!(ty.kind, TypeExprKind::Ptr(_)));
    }

    #[test]
    fn parse_type_fn() {
        let mut p = make_parser("fn(i64) -> bool\n");
        let ty = p.parse_type_expr().expect("parse failed");
        match &ty.kind {
            TypeExprKind::Fn { params, ret } => {
                assert_eq!(params.len(), 1);
                assert!(matches!(&ret.kind, TypeExprKind::Name(t) if t == "bool"));
            }
            _ => panic!("expected Fn"),
        }
    }

    #[test]
    fn parse_type_union() {
        let mut p = make_parser("i64 | f64\n");
        let ty = p.parse_type_expr().expect("parse failed");
        assert!(matches!(ty.kind, TypeExprKind::Union(_, _)));
    }

    #[test]
    fn parse_type_generic_with_multiple_args() {
        let mut p = make_parser("Dict[i64, f64]\n");
        let ty = p.parse_type_expr().expect("parse failed");
        match &ty.kind {
            TypeExprKind::Generic(_, args) => assert_eq!(args.len(), 2),
            _ => panic!("expected Generic"),
        }
    }

    #[test]
    fn parse_type_qualified() {
        let mut p = make_parser("glm.vec3\n");
        let ty = p.parse_type_expr().expect("parse failed");
        match &ty.kind {
            TypeExprKind::Qualified(parts) => {
                assert_eq!(parts, &["glm", "vec3"]);
            }
            _ => panic!("expected Qualified"),
        }
    }

    #[test]
    fn parse_type_qualified_three_parts() {
        let mut p = make_parser("a.b.c\n");
        let ty = p.parse_type_expr().expect("parse failed");
        match &ty.kind {
            TypeExprKind::Qualified(parts) => {
                assert_eq!(parts, &["a", "b", "c"]);
            }
            _ => panic!("expected Qualified"),
        }
    }
}
