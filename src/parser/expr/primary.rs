use super::super::{
    Parser,
    ast::*,
    error::{ParseError, ParseResult},
};
use crate::lexer::{Token, TokenKind};
use crate::span::Span;

impl Parser {
    pub(crate) fn parse_match(&mut self, start_tok: Token) -> ParseResult<Expr> {
        let start = Span {
            file_id: start_tok.file_id,
            line: start_tok.line,
            col: start_tok.col,
            start: start_tok.span.0,
            end: start_tok.span.1,
        };

        let expr = self.parse_expr()?;
        self.expect(TokenKind::Colon)?;

        let mut cases = Vec::new();
        if self.peek().kind == TokenKind::Newline {
            self.advance();
            self.expect(TokenKind::Indent)?;
            self.skip_newlines();
            self.skip_newlines();
            while self.peek().kind != TokenKind::Dedent && self.peek().kind != TokenKind::Eof {
                let case_tok = self.peek().clone();
                if self.peek().kind == TokenKind::Case {
                    self.advance();
                }
                let case_span = Span {
                    file_id: case_tok.file_id,
                    line: case_tok.line,
                    col: case_tok.col,
                    start: case_tok.span.0,
                    end: case_tok.span.1,
                };
                let pattern = self.parse_pattern()?;
                let guard = if self.peek().kind == TokenKind::If {
                    self.advance();
                    Some(self.parse_or()?)
                } else {
                    None
                };

                let body = self.parse_block()?;
                cases.push(MatchCase {
                    pattern,
                    guard,
                    body,
                    span: case_span,
                });
                self.skip_newlines();
            }
            let end_span = self.peek().span.1;
            self.expect(TokenKind::Dedent)?;

            let dummy = self.peek().clone();
            self.tokens.insert(
                self.pos,
                crate::lexer::Token {
                    kind: TokenKind::Newline,
                    value: "\n".into(),
                    line: dummy.line,
                    col: dummy.col,
                    span: dummy.span,
                    file_id: dummy.file_id,
                },
            );

            Ok(Expr::new(
                ExprKind::Match {
                    expr: Box::new(expr),
                    cases,
                },
                Span {
                    end: end_span,
                    ..start
                },
            ))
        } else {
            Err(self.err_at(
                &self.tokens[self.pos],
                "expected newline and indented block for match cases",
            ))
        }
    }

    pub(crate) fn parse_pattern(&mut self) -> ParseResult<MatchPattern> {
        match self.peek().kind {
            TokenKind::Underscore => {
                self.advance();
                Ok(MatchPattern::Wildcard)
            }
            TokenKind::Integer
            | TokenKind::Float
            | TokenKind::String
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Null => {
                let expr = self.parse_primary()?;
                Ok(MatchPattern::Literal(expr))
            }
            TokenKind::Identifier => {
                let tok = self.advance();
                let name = tok.value.clone();
                let name_span = Self::tok_span(&tok);
                if self.peek().kind == TokenKind::LParen {
                    self.advance();
                    let mut patterns = Vec::new();
                    while self.peek().kind != TokenKind::RParen
                        && self.peek().kind != TokenKind::Eof
                    {
                        patterns.push(self.parse_pattern()?);
                        if self.peek().kind == TokenKind::Comma {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.expect(TokenKind::RParen)?;
                    Ok(MatchPattern::Variant(name, patterns))
                } else {
                    if name.chars().next().is_some_and(char::is_uppercase) {
                        Ok(MatchPattern::Variant(name, vec![]))
                    } else {
                        Ok(MatchPattern::Identifier(name, name_span))
                    }
                }
            }
            _ => Err(self.err_at(&self.tokens[self.pos], "expected pattern")),
        }
    }

    pub(crate) fn parse_comp_clauses(&mut self) -> ParseResult<Vec<CompClause>> {
        let mut clauses = Vec::new();
        while self.peek().kind == TokenKind::For {
            self.advance();
            let target = self.parse_for_target()?;
            self.expect(TokenKind::In)?;
            let iter = self.parse_range()?;
            let condition = if self.peek().kind == TokenKind::If {
                self.advance();
                Some(self.parse_or()?)
            } else {
                None
            };
            clauses.push(CompClause {
                target,
                iter,
                condition,
            });
        }
        Ok(clauses)
    }

    pub(crate) fn parse_fstring(&mut self, tok: Token) -> ParseResult<Expr> {
        let value = &tok.value;
        let mut exprs = Vec::new();
        let mut last_pos = 0;
        let mut i = 0;
        let chars: Vec<char> = value.chars().collect();
        let span = Span {
            file_id: tok.file_id,
            line: tok.line,
            col: tok.col,
            start: tok.span.0,
            end: tok.span.1,
        };

        while i < chars.len() {
            if chars[i] == '{' {
                if i + 1 < chars.len() && chars[i + 1] == '{' {
                    i += 2;
                    continue;
                }

                if i > last_pos {
                    let s: String = chars[last_pos..i].iter().collect();
                    let s = s.replace("{{", "{").replace("}}", "}");
                    if !s.is_empty() {
                        exprs.push(FStrPart {
                            expr: Expr::new(ExprKind::Str(s), span),
                            spec: None,
                        });
                    }
                }

                i += 1;
                let start_expr = i;
                let mut brace_count = 1;
                while i < chars.len() && brace_count > 0 {
                    if chars[i] == '{' {
                        brace_count += 1;
                    } else if chars[i] == '}' {
                        brace_count -= 1;
                    }
                    i += 1;
                }

                if brace_count > 0 {
                    return Err(self.err_at(&tok, "unclosed '{' in f-string"));
                }

                let field: String = chars[start_expr..i - 1].iter().collect();
                let (mut expr_str, spec) = split_fstring_spec(&field);
                if expr_str.trim().is_empty() {
                    return Err(self.err_at(&tok, "empty expression in f-string"));
                }

                // Python 3.8 debug form `{expr=}`: a trailing `=` (not part of
                // `==`/`!=`/`<=`/`>=`) prints the source text of `expr` verbatim,
                // followed by `=`, followed by its value.
                let trimmed_end = expr_str.trim_end();
                if let Some(before) = trimmed_end.strip_suffix('=')
                    && !before.trim().is_empty()
                    && !matches!(before.chars().last(), Some('=' | '!' | '<' | '>'))
                {
                    exprs.push(FStrPart {
                        expr: Expr::new(ExprKind::Str(trimmed_end.to_string()), span),
                        spec: None,
                    });
                    expr_str = before.to_string();
                }

                let mut lexer = crate::lexer::Lexer::new(&expr_str, tok.file_id);
                let tokens = lexer.tokenise().map_err(|e| ParseError {
                    message: format!("lexer error in f-string: {}", e.message),
                    line: tok.line,
                    col: tok.col,
                    start: tok.span.0 + start_expr,
                    end: tok.span.0 + i,
                })?;

                let mut parser = Parser::new(tokens);
                let expr = parser.parse_expr().map_err(|e| ParseError {
                    message: format!("parser error in f-string: {}", e.message),
                    line: tok.line,
                    col: tok.col,
                    start: tok.span.0 + start_expr,
                    end: tok.span.0 + i,
                })?;
                exprs.push(FStrPart { expr, spec });

                last_pos = i;
            } else if chars[i] == '}' {
                if i + 1 < chars.len() && chars[i + 1] == '}' {
                    i += 2;
                    continue;
                }
                return Err(self.err_at(&tok, "single '}' not allowed in f-string"));
            } else {
                i += 1;
            }
        }

        if last_pos < chars.len() {
            let s: String = chars[last_pos..].iter().collect();
            let s = s.replace("{{", "{").replace("}}", "}");
            if !s.is_empty() {
                exprs.push(FStrPart {
                    expr: Expr::new(ExprKind::Str(s), span),
                    spec: None,
                });
            }
        }

        Ok(Expr::new(ExprKind::FStr(exprs), span))
    }

    pub(crate) fn parse_primary(&mut self) -> ParseResult<Expr> {
        let tok = self.peek().clone();
        let start = Span {
            file_id: tok.file_id,
            line: tok.line,
            col: tok.col,
            start: tok.span.0,
            end: tok.span.1,
        };
        match tok.kind {
            TokenKind::Integer => {
                self.advance();
                let val: Result<i64, _> =
                    if tok.value.starts_with("0x") || tok.value.starts_with("0X") {
                        i64::from_str_radix(&tok.value[2..], 16)
                    } else if tok.value.starts_with("0o") || tok.value.starts_with("0O") {
                        i64::from_str_radix(&tok.value[2..], 8)
                    } else if tok.value.starts_with("0b") || tok.value.starts_with("0B") {
                        i64::from_str_radix(&tok.value[2..], 2)
                    } else {
                        tok.value.parse::<i64>()
                    };
                val.map(|n| Expr::new(ExprKind::Integer(n), start))
                    .map_err(|_| {
                        self.err_at(
                            &tok,
                            format!("integer literal '{}' out of i64 range", tok.value),
                        )
                    })
            }
            TokenKind::Float => {
                self.advance();
                tok.value
                    .parse::<f64>()
                    .map(|f| Expr::new(ExprKind::Float(f), start))
                    .map_err(|_| {
                        self.err_at(&tok, format!("invalid float literal '{}'", tok.value))
                    })
            }
            TokenKind::String => {
                self.advance();
                Ok(Expr::new(ExprKind::Str(tok.value), start))
            }
            TokenKind::FString => {
                self.advance();
                self.parse_fstring(tok)
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr::new(ExprKind::Bool(true), start))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr::new(ExprKind::Bool(false), start))
            }
            TokenKind::Null => {
                self.advance();
                Ok(Expr::new(ExprKind::Null, start))
            }
            TokenKind::Match => {
                self.advance();
                self.parse_match(tok)
            }
            TokenKind::Async => {
                self.advance();
                let body = self.parse_block()?;
                let span = self.span_from(&tok);
                let dummy = self.tokens[self.pos].clone();
                self.tokens.insert(
                    self.pos,
                    Token {
                        kind: TokenKind::Newline,
                        value: "\n".into(),
                        line: dummy.line,
                        col: dummy.col,
                        span: dummy.span,
                        file_id: dummy.file_id,
                    },
                );
                Ok(Expr::new(ExprKind::AsyncBlock(body), span))
            }
            TokenKind::Identifier => {
                self.advance();
                Ok(Expr::new(ExprKind::Identifier(tok.value), start))
            }

            TokenKind::LParen => {
                self.advance();
                if self.peek().kind == TokenKind::RParen {
                    let end = self.peek().span.1;
                    self.advance();
                    return Ok(Expr::new(ExprKind::Tuple(vec![]), Span { end, ..start }));
                }
                let first = self.parse_expr()?;
                if self.peek().kind == TokenKind::Comma {
                    let mut elems = vec![first];
                    while self.peek().kind == TokenKind::Comma {
                        self.advance();
                        if self.peek().kind == TokenKind::RParen {
                            break;
                        }
                        elems.push(self.parse_expr()?);
                    }
                    let end = self.peek().span.1;
                    self.expect(TokenKind::RParen)?;
                    Ok(Expr::new(ExprKind::Tuple(elems), Span { end, ..start }))
                } else {
                    self.expect(TokenKind::RParen)?;
                    Ok(first)
                }
            }

            TokenKind::LBracket => {
                self.advance();
                if self.peek().kind == TokenKind::RBracket {
                    let end = self.peek().span.1;
                    self.advance();
                    return Ok(Expr::new(ExprKind::List(vec![]), Span { end, ..start }));
                }
                let first = self.parse_expr()?;
                if self.peek().kind == TokenKind::For {
                    let clauses = self.parse_comp_clauses()?;
                    let end = self.peek().span.1;
                    self.expect(TokenKind::RBracket)?;
                    Ok(Expr::new(
                        ExprKind::ListComp {
                            elt: Box::new(first),
                            clauses,
                        },
                        Span { end, ..start },
                    ))
                } else {
                    let mut elems = vec![first];
                    while self.peek().kind == TokenKind::Comma {
                        self.advance();
                        if self.peek().kind == TokenKind::RBracket {
                            break;
                        }
                        elems.push(self.parse_expr()?);
                    }
                    let end = self.peek().span.1;
                    self.expect(TokenKind::RBracket)?;
                    Ok(Expr::new(ExprKind::List(elems), Span { end, ..start }))
                }
            }

            TokenKind::LBrace => {
                self.advance();
                if self.peek().kind == TokenKind::RBrace {
                    let end = self.peek().span.1;
                    self.advance();
                    return Ok(Expr::new(ExprKind::Dict(vec![]), Span { end, ..start }));
                }
                let first = self.parse_expr()?;
                match self.peek().kind {
                    TokenKind::Colon => {
                        self.advance();
                        let first_val = self.parse_expr()?;
                        if self.peek().kind == TokenKind::For {
                            let clauses = self.parse_comp_clauses()?;
                            let end = self.peek().span.1;
                            self.expect(TokenKind::RBrace)?;
                            Ok(Expr::new(
                                ExprKind::DictComp {
                                    key: Box::new(first),
                                    value: Box::new(first_val),
                                    clauses,
                                },
                                Span { end, ..start },
                            ))
                        } else {
                            let mut pairs = vec![(first, first_val)];
                            while self.peek().kind == TokenKind::Comma {
                                self.advance();
                                if self.peek().kind == TokenKind::RBrace {
                                    break;
                                }
                                let k = self.parse_expr()?;
                                self.expect(TokenKind::Colon)?;
                                let v = self.parse_expr()?;
                                pairs.push((k, v));
                            }
                            let end = self.peek().span.1;
                            self.expect(TokenKind::RBrace)?;
                            Ok(Expr::new(ExprKind::Dict(pairs), Span { end, ..start }))
                        }
                    }
                    TokenKind::For => {
                        let clauses = self.parse_comp_clauses()?;
                        let end = self.peek().span.1;
                        self.expect(TokenKind::RBrace)?;
                        Ok(Expr::new(
                            ExprKind::SetComp {
                                elt: Box::new(first),
                                clauses,
                            },
                            Span { end, ..start },
                        ))
                    }
                    _ => {
                        let mut elems = vec![first];
                        while self.peek().kind == TokenKind::Comma {
                            self.advance();
                            if self.peek().kind == TokenKind::RBrace {
                                break;
                            }
                            elems.push(self.parse_expr()?);
                        }
                        let end = self.peek().span.1;
                        self.expect(TokenKind::RBrace)?;
                        Ok(Expr::new(ExprKind::Set(elems), Span { end, ..start }))
                    }
                }
            }

            TokenKind::Lambda => {
                self.advance();
                self.parse_lambda(tok)
            }

            _ => Err(self.err_at(
                &tok,
                format!("unexpected token {:?} {:?}", tok.kind, tok.value),
            )),
        }
    }

    pub(crate) fn parse_lambda(&mut self, start_tok: Token) -> ParseResult<Expr> {
        let start = Span {
            file_id: start_tok.file_id,
            line: start_tok.line,
            col: start_tok.col,
            start: start_tok.span.0,
            end: start_tok.span.1,
        };
        // `lambda x, y: expr` (bare names, unannotated) reads naturally up to
        // the body-separating `:`. A type annotation would itself use `:`
        // and collide with that separator, so an annotated param list must
        // be parenthesized: `lambda (x: int, y: int): expr`.
        let params = if self.peek().kind == TokenKind::Colon {
            Vec::new()
        } else if self.peek().kind == TokenKind::LParen {
            self.advance();
            let params = self.parse_params()?;
            self.expect(TokenKind::RParen)?;
            params
        } else {
            self.parse_bare_lambda_params()?
        };
        self.expect(TokenKind::Colon)?;
        let body = self.parse_expr()?;
        let end = body.span.end;
        Ok(Expr::new(
            ExprKind::Lambda {
                params,
                body: Box::new(body),
            },
            Span { end, ..start },
        ))
    }

    fn parse_bare_lambda_params(&mut self) -> ParseResult<Vec<Param>> {
        let mut params = Vec::new();
        loop {
            let tok = self.expect(TokenKind::Identifier)?;
            let span = self.span_from(&tok);
            params.push(Param {
                name: tok.value,
                type_ann: None,
                default: None,
                kind: ParamKind::Regular,
                is_mut: false,
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
}

/// Splits an f-string field into its expression and optional format spec at the
/// first top-level colon, leaving colons inside slices, indexes, or nested
/// braces untouched.
fn split_fstring_spec(field: &str) -> (String, Option<String>) {
    let chars: Vec<char> = field.chars().collect();
    let mut depth = 0i32;
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ':' if depth == 0 => {
                return (
                    chars[..i].iter().collect(),
                    Some(chars[i + 1..].iter().collect()),
                );
            }
            _ => {}
        }
    }
    (field.to_string(), None)
}
