use super::{
    Parser,
    ast::*,
    error::{ParseError, ParseResult},
};
use crate::lexer::{Token, TokenKind};
use crate::span::Span;

impl Parser {
    pub(crate) fn is_valid_assign_target(expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Identifier(_) | ExprKind::Attr { .. } | ExprKind::Index { .. } => true,
            ExprKind::Tuple(elems) => elems.iter().all(Self::is_valid_assign_target),
            _ => false,
        }
    }

    pub(crate) fn parse_expr_list(&mut self) -> ParseResult<Expr> {
        let first = self.parse_expr()?;
        if self.peek().kind != TokenKind::Comma {
            return Ok(first);
        }
        let start_span = first.span;
        let mut elems = vec![first];
        while self.peek().kind == TokenKind::Comma {
            self.advance();
            if matches!(
                self.peek().kind,
                TokenKind::Equal
                    | TokenKind::PlusEqual
                    | TokenKind::MinusEqual
                    | TokenKind::StarEqual
                    | TokenKind::SlashEqual
                    | TokenKind::PercentEqual
                    | TokenKind::DoubleStarEqual
                    | TokenKind::Newline
                    | TokenKind::Semicolon
                    | TokenKind::Eof
                    | TokenKind::Dedent
            ) {
                break;
            }
            elems.push(self.parse_expr()?);
        }
        let end_span = elems.last().map(|e| e.span).unwrap_or(start_span);
        Ok(Expr::new(
            ExprKind::Tuple(elems),
            start_span.merge(end_span),
        ))
    }

    pub(crate) fn parse_expr(&mut self) -> ParseResult<Expr> {
        self.parse_or()
    }

    pub(crate) fn parse_or(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_and()?;
        while self.peek().kind == TokenKind::Or {
            self.advance();
            let right = self.parse_and()?;
            let span = left.span.merge(right.span);
            left = Expr::new(
                ExprKind::BinOp {
                    left: Box::new(left),
                    op: BinOp::Or,
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    pub(crate) fn parse_and(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_not()?;
        while self.peek().kind == TokenKind::And {
            self.advance();
            let right = self.parse_not()?;
            let span = left.span.merge(right.span);
            left = Expr::new(
                ExprKind::BinOp {
                    left: Box::new(left),
                    op: BinOp::And,
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    pub(crate) fn parse_not(&mut self) -> ParseResult<Expr> {
        if self.peek().kind == TokenKind::Not {
            let start = self.peek().clone();
            self.advance();
            let operand = self.parse_not()?;
            let span = self.span_from(&start);
            Ok(Expr::new(
                ExprKind::UnaryOp {
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                },
                span,
            ))
        } else {
            self.parse_comparison()
        }
    }

    pub(crate) fn parse_comparison(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_add()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::DoubleEqual => {
                    self.advance();
                    BinOp::Eq
                }
                TokenKind::NotEqual => {
                    self.advance();
                    BinOp::NotEq
                }
                TokenKind::Not => {
                    // Check for 'not in' or binary 'not' (alias for !=)
                    self.advance();
                    if self.peek().kind == TokenKind::In {
                        self.advance();
                        BinOp::NotIn
                    } else {
                        BinOp::NotEq
                    }
                }
                TokenKind::Less => {
                    self.advance();
                    BinOp::Lt
                }
                TokenKind::LessEqual => {
                    self.advance();
                    BinOp::LtEq
                }
                TokenKind::Greater => {
                    self.advance();
                    BinOp::Gt
                }
                TokenKind::GreaterEqual => {
                    self.advance();
                    BinOp::GtEq
                }
                TokenKind::In => {
                    self.advance();
                    BinOp::In
                }
                _ => break,
            };
            let right = self.parse_add()?;
            let span = left.span.merge(right.span);
            left = Expr::new(
                ExprKind::BinOp {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    pub(crate) fn parse_add(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul()?;
            let span = left.span.merge(right.span);
            left = Expr::new(
                ExprKind::BinOp {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    pub(crate) fn parse_mul(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary()?;
            let span = left.span.merge(right.span);
            left = Expr::new(
                ExprKind::BinOp {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    pub(crate) fn parse_unary(&mut self) -> ParseResult<Expr> {
        match self.peek().kind {
            TokenKind::Try => {
                let start = self.peek().clone();
                self.advance();
                let operand = self.parse_unary()?;
                let span = self.span_from(&start);
                Ok(Expr::new(ExprKind::Try(Box::new(operand)), span))
            }
            TokenKind::Ampersand => {
                let start = self.peek().clone();
                self.advance();
                if self.peek().kind == TokenKind::Mut {
                    self.advance();
                    let operand = self.parse_unary()?;
                    let span = self.span_from(&start);
                    Ok(Expr::new(ExprKind::MutBorrow(Box::new(operand)), span))
                } else {
                    let operand = self.parse_unary()?;
                    let span = self.span_from(&start);
                    Ok(Expr::new(ExprKind::Borrow(Box::new(operand)), span))
                }
            }
            TokenKind::Minus => {
                let start = self.peek().clone();
                self.advance();
                let operand = self.parse_unary()?;
                let span = self.span_from(&start);
                Ok(Expr::new(
                    ExprKind::UnaryOp {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }
            TokenKind::Plus => {
                let start = self.peek().clone();
                self.advance();
                let operand = self.parse_unary()?;
                let span = self.span_from(&start);
                Ok(Expr::new(
                    ExprKind::UnaryOp {
                        op: UnaryOp::Pos,
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }
            _ => self.parse_power(),
        }
    }

    pub(crate) fn parse_power(&mut self) -> ParseResult<Expr> {
        let base = self.parse_postfix()?;
        if self.peek().kind == TokenKind::DoubleStar {
            self.advance();
            let exp = self.parse_unary()?;
            let span = base.span.merge(exp.span);
            Ok(Expr::new(
                ExprKind::BinOp {
                    left: Box::new(base),
                    op: BinOp::Pow,
                    right: Box::new(exp),
                },
                span,
            ))
        } else {
            Ok(base)
        }
    }

    pub(crate) fn parse_postfix(&mut self) -> ParseResult<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek().kind {
                TokenKind::Dot | TokenKind::DoubleColon => {
                    let op = self.advance();
                    let attr = self.expect(TokenKind::Identifier)?.value;
                    let span = self.span_from(&Token {
                        kind: TokenKind::Identifier,
                        value: String::new(),
                        line: expr.span.line,
                        col: expr.span.col,
                        span: (expr.span.start, expr.span.end),
                        file_id: expr.span.file_id,
                    });
                    if op.kind == TokenKind::DoubleColon {
                        if let ExprKind::Identifier(ref name) = expr.kind {
                            expr = Expr::new(
                                ExprKind::Identifier(format!("{}::{}", name, attr)),
                                span,
                            );
                        } else {
                            return Err(self.err_at(&op, "expected identifier before '::'"));
                        }
                    } else {
                        expr = Expr::new(
                            ExprKind::Attr {
                                obj: Box::new(expr),
                                attr,
                            },
                            span,
                        );
                    }
                }
                TokenKind::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(TokenKind::RBracket)?;
                    let span = self.span_from(&Token {
                        kind: TokenKind::Identifier,
                        value: String::new(),
                        line: expr.span.line,
                        col: expr.span.col,
                        span: (expr.span.start, expr.span.end),
                        file_id: expr.span.file_id,
                    });
                    expr = Expr::new(
                        ExprKind::Index {
                            obj: Box::new(expr),
                            index: Box::new(index),
                        },
                        span,
                    );
                }
                TokenKind::LParen => {
                    self.advance();
                    let args = self.parse_call_args()?;
                    self.expect(TokenKind::RParen)?;
                    let span = self.span_from(&Token {
                        kind: TokenKind::Identifier,
                        value: String::new(),
                        line: expr.span.line,
                        col: expr.span.col,
                        span: (expr.span.start, expr.span.end),
                        file_id: expr.span.file_id,
                    });
                    expr = Expr::new(
                        ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                        },
                        span,
                    );
                }
                TokenKind::Question => {
                    let op = self.advance();
                    let span = expr.span.merge(self.span_from(&op));
                    expr = Expr::new(ExprKind::Try(Box::new(expr)), span);
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    pub(crate) fn parse_call_args(&mut self) -> ParseResult<Vec<CallArg>> {
        let mut args = Vec::new();
        while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
            let arg = if self.peek().kind == TokenKind::DoubleStar {
                self.advance();
                CallArg::KwSplat(self.parse_expr()?)
            } else if self.peek().kind == TokenKind::Star {
                self.advance();
                CallArg::Splat(self.parse_expr()?)
            } else if self.peek().kind == TokenKind::Identifier
                && self.peek_at(1).kind == TokenKind::Equal
            {
                let name = self.advance().value;
                self.advance(); // =
                CallArg::Keyword(name, self.parse_expr()?)
            } else {
                CallArg::Positional(self.parse_expr()?)
            };
            args.push(arg);
            if self.peek().kind == TokenKind::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(args)
    }

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
            while self.peek().kind != TokenKind::Dedent && self.peek().kind != TokenKind::Eof {
                // Parse pattern
                let pattern = self.parse_pattern()?;
                
                let body = self.parse_block()?;
                cases.push(MatchCase { pattern, body });
                self.skip_newlines();
            }
            let end_span = self.peek().span.1;
            self.expect(TokenKind::Dedent)?;
            
            // Inject a synthetic Newline token to terminate the enclosing statement
            // because the Dedent effectively ends the logical line.
            let dummy = self.peek().clone();
            self.tokens.insert(self.pos, crate::lexer::Token {
                kind: TokenKind::Newline,
                value: "\n".into(),
                line: dummy.line,
                col: dummy.col,
                span: dummy.span,
                file_id: dummy.file_id,
            });
            
            Ok(Expr::new(ExprKind::Match {
                expr: Box::new(expr),
                cases,
            }, Span { end: end_span, ..start }))
        } else {
            return Err(self.err_at(&self.tokens[self.pos], "expected newline and indented block for match cases"));
        }
    }

    pub(crate) fn parse_pattern(&mut self) -> ParseResult<MatchPattern> {
        match self.peek().kind {
            TokenKind::Underscore => {
                self.advance();
                Ok(MatchPattern::Wildcard)
            }
            TokenKind::Integer | TokenKind::Float | TokenKind::String | TokenKind::True | TokenKind::False => {
                let expr = self.parse_primary()?;
                Ok(MatchPattern::Literal(expr))
            }
            TokenKind::Identifier => {
                let tok = self.advance();
                let name = tok.value.clone();
                if self.peek().kind == TokenKind::LParen {
                    self.advance();
                    let mut patterns = Vec::new();
                    while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
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
                    if name.chars().next().unwrap().is_uppercase() {
                        Ok(MatchPattern::Variant(name, vec![]))
                    } else {
                        Ok(MatchPattern::Identifier(name))
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
            let iter = self.parse_or()?;
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
                // Check if it's double {{
                if i + 1 < chars.len() && chars[i + 1] == '{' {
                    i += 2;
                    continue;
                }

                // Add literal part before {
                if i > last_pos {
                    let s: String = chars[last_pos..i].iter().collect();
                    let s = s.replace("{{", "{").replace("}}", "}");
                    if !s.is_empty() {
                        exprs.push(Expr::new(ExprKind::Str(s), span));
                    }
                }

                // Find matching }
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

                let expr_str: String = chars[start_expr..i - 1].iter().collect();
                if expr_str.trim().is_empty() {
                    return Err(self.err_at(&tok, "empty expression in f-string"));
                }

                // Lex and parse expr_str
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
                exprs.push(expr);

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
                exprs.push(Expr::new(ExprKind::Str(s), span));
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
            TokenKind::Match => {
                self.advance();
                self.parse_match(tok)
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

            _ => Err(self.err_at(
                &tok,
                format!("unexpected token {:?} {:?}", tok.kind, tok.value),
            )),
        }
    }
}
