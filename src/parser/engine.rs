use crate::lexer::{Token, TokenKind};
use crate::span::Span;
use super::ast::*;
use super::error::{ParseError, ParseResult};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_at(&self, offset: usize) -> &Token {
        let i = self.pos + offset;
        if i < self.tokens.len() { &self.tokens[i] } else { self.tokens.last().unwrap() }
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if tok.kind != TokenKind::Eof {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, kind: TokenKind) -> ParseResult<Token> {
        let tok = self.peek().clone();
        if tok.kind == kind {
            if tok.kind != TokenKind::Eof {
                self.pos += 1;
            }
            Ok(tok)
        } else {
            Err(ParseError {
                message: format!("expected {:?}, got {:?} {:?}", kind, tok.kind, tok.value),
                line: tok.line,
                col: tok.col,
                start: tok.span.0,
                end: tok.span.1,
            })
        }
    }

    fn err_at(&self, tok: &Token, msg: impl Into<String>) -> ParseError {
        ParseError {
            message: msg.into(),
            line: tok.line,
            col: tok.col,
            start: tok.span.0,
            end: tok.span.1,
        }
    }

    fn skip_newlines(&mut self) {
        while self.peek().kind == TokenKind::Newline {
            self.pos += 1;
        }
    }

    fn eat_stmt_end(&mut self) -> ParseResult<()> {
        match self.peek().kind {
            TokenKind::Newline | TokenKind::Semicolon => { self.pos += 1; Ok(()) }
            TokenKind::Eof | TokenKind::Dedent => Ok(()),
            _ => {
                let tok = self.peek().clone();
                Err(ParseError {
                    message: format!("expected newline, got {:?} {:?}", tok.kind, tok.value),
                    line: tok.line,
                    col: tok.col,
                    start: tok.span.0,
                    end: tok.span.1,
                })
            }
        }
    }

    fn span_from(&self, start: &Token) -> Span {
        let end = if self.pos > 0 {
            self.tokens[self.pos - 1].span.1
        } else {
            start.span.1
        };
        Span { file_id: start.file_id, line: start.line, col: start.col, start: start.span.0, end }
    }

    // --- Public entry point ---

    pub fn parse_program(&mut self) -> ParseResult<Program> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while self.peek().kind != TokenKind::Eof {
            stmts.push(self.parse_stmt()?);
            self.skip_newlines();
        }
        Ok(Program { stmts })
    }

    // --- Statements ---

    fn parse_stmt(&mut self) -> ParseResult<Stmt> {
        match self.peek().kind {
            TokenKind::Fn       => self.parse_fn(),
            TokenKind::Class    => self.parse_class(),
            TokenKind::If       => self.parse_if(),
            TokenKind::While    => self.parse_while(),
            TokenKind::For      => self.parse_for(),
            TokenKind::Try      => self.parse_try(),
            TokenKind::Return   => self.parse_return(),
            TokenKind::Raise    => self.parse_raise(),
            TokenKind::Assert   => self.parse_assert(),
            TokenKind::Import   => self.parse_import(),
            TokenKind::From     => self.parse_from_import(),
            TokenKind::Let      => self.parse_let(),
            TokenKind::Pass     => {
                let s = self.peek().clone();
                self.advance(); self.eat_stmt_end()?;
                Ok(Stmt::new(StmtKind::Pass, self.span_from(&s)))
            }
            TokenKind::Break    => {
                let s = self.peek().clone();
                self.advance(); self.eat_stmt_end()?;
                Ok(Stmt::new(StmtKind::Break, self.span_from(&s)))
            }
            TokenKind::Continue => {
                let s = self.peek().clone();
                self.advance(); self.eat_stmt_end()?;
                Ok(Stmt::new(StmtKind::Continue, self.span_from(&s)))
            }
            _ => self.parse_expr_or_assign(),
        }
    }

    fn parse_block(&mut self) -> ParseResult<Vec<Stmt>> {
        self.expect(TokenKind::Colon)?;
        if self.peek().kind == TokenKind::Newline {
            self.advance();
            self.expect(TokenKind::Indent)?;
            let mut stmts = Vec::new();
            self.skip_newlines();
            while self.peek().kind != TokenKind::Dedent && self.peek().kind != TokenKind::Eof {
                stmts.push(self.parse_stmt()?);
                self.skip_newlines();
            }
            self.expect(TokenKind::Dedent)?;
            Ok(stmts)
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    fn parse_fn(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Fn)?;
        let name = self.expect(TokenKind::Identifier)?.value;
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
        Ok(Stmt::new(StmtKind::Fn { name, params, return_type, body }, span))
    }

    fn parse_params(&mut self) -> ParseResult<Vec<Param>> {
        let mut params = Vec::new();
        while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
            let param_start = self.peek().clone();
            let kind = match self.peek().kind {
                TokenKind::DoubleStar => { self.advance(); ParamKind::KwArg }
                TokenKind::Star       => { self.advance(); ParamKind::VarArg }
                _                     => ParamKind::Regular,
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
            params.push(Param { name, type_ann, default, kind, is_mut, span });
            if self.peek().kind == TokenKind::Comma { self.advance(); } else { break; }
        }
        Ok(params)
    }

    fn parse_class(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Class)?;
        let name = self.expect(TokenKind::Identifier)?.value;
        let bases = if self.peek().kind == TokenKind::LParen {
            self.advance();
            let mut bases = Vec::new();
            while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
                bases.push(self.parse_expr()?);
                if self.peek().kind == TokenKind::Comma { self.advance(); } else { break; }
            }
            self.expect(TokenKind::RParen)?;
            bases
        } else {
            Vec::new()
        };
        let body = self.parse_block()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Class { name, bases, body }, span))
    }

    fn parse_if(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::If)?;
        let condition = self.parse_expr()?;
        let then_body = self.parse_block()?;

        let mut elif_clauses = Vec::new();
        let mut else_body = None;

        loop {
            self.skip_newlines();
            let kind = self.peek().kind.clone();
            if kind == TokenKind::Elif {
                self.advance();
                let cond = self.parse_expr()?;
                let body = self.parse_block()?;
                elif_clauses.push((cond, body));
            } else if kind == TokenKind::Else {
                self.advance();
                self.skip_newlines();
                if self.peek().kind == TokenKind::If {
                    self.advance();
                    let cond = self.parse_expr()?;
                    let body = self.parse_block()?;
                    elif_clauses.push((cond, body));
                } else {
                    else_body = Some(self.parse_block()?);
                    break;
                }
            } else {
                break;
            }
        }

        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::If { condition, then_body, elif_clauses, else_body }, span))
    }

    fn parse_while(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::While)?;
        let condition = self.parse_expr()?;
        let body = self.parse_block()?;
        self.skip_newlines();
        let else_body = if self.peek().kind == TokenKind::Else {
            self.advance();
            Some(self.parse_block()?)
        } else {
            None
        };
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::While { condition, body, else_body }, span))
    }

    fn parse_for(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::For)?;
        let target = self.parse_for_target()?;
        self.expect(TokenKind::In)?;
        let iter = self.parse_expr()?;
        let body = self.parse_block()?;
        self.skip_newlines();
        let else_body = if self.peek().kind == TokenKind::Else {
            self.advance();
            Some(self.parse_block()?)
        } else {
            None
        };
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::For { target, iter, body, else_body }, span))
    }

    fn parse_for_target(&mut self) -> ParseResult<ForTarget> {
        let outer_start = self.peek().clone();
        if self.peek().kind == TokenKind::LParen {
            self.advance();
            let mut names = Vec::new();
            let tok = self.expect(TokenKind::Identifier)?;
            let sp = Span { file_id: tok.file_id, line: tok.line, col: tok.col, start: tok.span.0, end: tok.span.1 };
            names.push((tok.value, sp));
            while self.peek().kind == TokenKind::Comma {
                self.advance();
                if self.peek().kind == TokenKind::RParen { break; }
                let tok = self.expect(TokenKind::Identifier)?;
                let sp = Span { file_id: tok.file_id, line: tok.line, col: tok.col, start: tok.span.0, end: tok.span.1 };
                names.push((tok.value, sp));
            }
            self.expect(TokenKind::RParen)?;
            let span = self.span_from(&outer_start);
            return Ok(ForTarget::Tuple(names, span));
        }
        let tok = self.expect(TokenKind::Identifier)?;
        let name_span = Span { file_id: tok.file_id, line: tok.line, col: tok.col, start: tok.span.0, end: tok.span.1 };
        let name = tok.value;
        if self.peek().kind == TokenKind::Comma {
            let mut names = vec![(name, name_span)];
            while self.peek().kind == TokenKind::Comma {
                self.advance();
                if self.peek().kind == TokenKind::In { break; }
                let tok = self.expect(TokenKind::Identifier)?;
                let sp = Span { file_id: tok.file_id, line: tok.line, col: tok.col, start: tok.span.0, end: tok.span.1 };
                names.push((tok.value, sp));
            }
            let span = self.span_from(&outer_start);
            Ok(ForTarget::Tuple(names, span))
        } else {
            Ok(ForTarget::Name(name, name_span))
        }
    }

    fn parse_try(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Try)?;
        let body = self.parse_block()?;

        let mut handlers = Vec::new();
        let mut else_body: Option<Vec<Stmt>> = None;
        let mut finally_body: Option<Vec<Stmt>> = None;

        self.skip_newlines();

        while self.peek().kind == TokenKind::Except {
            let handler_start = self.peek().clone();
            self.advance();
            let exc_type = if self.peek().kind != TokenKind::Colon {
                Some(self.parse_expr()?)
            } else {
                None
            };
            let name = if self.peek().kind == TokenKind::As {
                self.advance();
                Some(self.expect(TokenKind::Identifier)?.value)
            } else {
                None
            };
            let handler_body = self.parse_block()?;
            let handler_span = self.span_from(&handler_start);
            handlers.push(ExceptHandler { exc_type, name, body: handler_body, span: handler_span });
            self.skip_newlines();
        }

        if self.peek().kind == TokenKind::Else {
            self.advance();
            else_body = Some(self.parse_block()?);
            self.skip_newlines();
        }

        if self.peek().kind == TokenKind::Finally {
            self.advance();
            finally_body = Some(self.parse_block()?);
        }

        if handlers.is_empty() && finally_body.is_none() {
            let tok = self.peek().clone();
            return Err(ParseError {
                message: "try without except or finally".into(),
                line: tok.line,
                col: tok.col,
                start: tok.span.0,
                end: tok.span.1,
            });
        }

        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Try { body, handlers, else_body, finally_body }, span))
    }

    fn parse_return(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Return)?;
        let value = match self.peek().kind {
            TokenKind::Newline | TokenKind::Semicolon
            | TokenKind::Eof   | TokenKind::Dedent => None,
            _ => Some(self.parse_expr()?),
        };
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Return(value), span))
    }

    fn parse_raise(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Raise)?;
        let value = match self.peek().kind {
            TokenKind::Newline | TokenKind::Semicolon
            | TokenKind::Eof   | TokenKind::Dedent => None,
            _ => Some(self.parse_expr()?),
        };
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Raise(value), span))
    }

    fn parse_assert(&mut self) -> ParseResult<Stmt> {
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

    fn parse_import(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Import)?;
        let mut path = vec![self.expect(TokenKind::Identifier)?.value];
        while self.peek().kind == TokenKind::Dot {
            self.advance();
            path.push(self.expect(TokenKind::Identifier)?.value);
        }
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Import(path), span))
    }

    fn parse_from_import(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::From)?;
        let mut module = vec![self.expect(TokenKind::Identifier)?.value];
        while self.peek().kind == TokenKind::Dot {
            self.advance();
            module.push(self.expect(TokenKind::Identifier)?.value);
        }
        self.expect(TokenKind::Import)?;
        let mut names = vec![self.expect(TokenKind::Identifier)?.value];
        while self.peek().kind == TokenKind::Comma {
            self.advance();
            names.push(self.expect(TokenKind::Identifier)?.value);
        }
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::FromImport { module, names }, span))
    }

    fn parse_let(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        self.expect(TokenKind::Let)?;
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
        self.expect(TokenKind::Equal)?;
        let value = self.parse_expr()?;
        self.eat_stmt_end()?;
        let span = self.span_from(&start);
        Ok(Stmt::new(StmtKind::Let { name, type_ann, value, is_mut }, span))
    }

    fn is_valid_assign_target(expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Identifier(_)
            | ExprKind::Attr { .. }
            | ExprKind::Index { .. } => true,
            ExprKind::Tuple(elems) => elems.iter().all(Self::is_valid_assign_target),
            _ => false,
        }
    }

    fn parse_expr_list(&mut self) -> ParseResult<Expr> {
        let first = self.parse_expr()?;
        if self.peek().kind != TokenKind::Comma {
            return Ok(first);
        }
        let start_span = first.span;
        let mut elems = vec![first];
        while self.peek().kind == TokenKind::Comma {
            self.advance();
            if matches!(self.peek().kind,
                TokenKind::Equal
                | TokenKind::PlusEqual | TokenKind::MinusEqual
                | TokenKind::StarEqual | TokenKind::SlashEqual
                | TokenKind::DoubleSlashEqual | TokenKind::PercentEqual
                | TokenKind::DoubleStarEqual
                | TokenKind::Newline | TokenKind::Semicolon
                | TokenKind::Eof    | TokenKind::Dedent
            ) { break; }
            elems.push(self.parse_expr()?);
        }
        let end_span = elems.last().map(|e| e.span).unwrap_or(start_span);
        Ok(Expr::new(ExprKind::Tuple(elems), start_span.merge(end_span)))
    }

    fn parse_expr_or_assign(&mut self) -> ParseResult<Stmt> {
        let start = self.peek().clone();
        let lhs = self.parse_expr_list()?;
        let (op_line, op_col) = (self.peek().line, self.peek().col);
        match self.peek().kind.clone() {
            TokenKind::Equal => {
                if !Self::is_valid_assign_target(&lhs) {
                    return Err(ParseError {
                        message: "invalid assignment target".into(),
                        line: op_line, col: op_col,
                        start: lhs.span.start,
                        end: lhs.span.end,
                    });
                }
                self.advance();
                let value = self.parse_expr_list()?;
                self.eat_stmt_end()?;
                let span = self.span_from(&start);
                Ok(Stmt::new(StmtKind::Assign { target: lhs, value }, span))
            }
            kind @ (TokenKind::PlusEqual | TokenKind::MinusEqual | TokenKind::StarEqual
                  | TokenKind::SlashEqual | TokenKind::DoubleSlashEqual
                  | TokenKind::PercentEqual | TokenKind::DoubleStarEqual) => {
                if !Self::is_valid_assign_target(&lhs) {
                    return Err(ParseError {
                        message: "invalid augmented assignment target".into(),
                        line: op_line, col: op_col,
                        start: lhs.span.start,
                        end: lhs.span.end,
                    });
                }
                self.advance();
                let value = self.parse_expr()?;
                self.eat_stmt_end()?;
                let op = match kind {
                    TokenKind::PlusEqual         => AugOp::Add,
                    TokenKind::MinusEqual        => AugOp::Sub,
                    TokenKind::StarEqual         => AugOp::Mul,
                    TokenKind::SlashEqual        => AugOp::Div,
                    TokenKind::DoubleSlashEqual  => AugOp::FloorDiv,
                    TokenKind::PercentEqual      => AugOp::Mod,
                    TokenKind::DoubleStarEqual   => AugOp::Pow,
                    _ => unreachable!(),
                };
                let span = self.span_from(&start);
                Ok(Stmt::new(StmtKind::AugAssign { target: lhs, op, value }, span))
            }
            _ => {
                self.eat_stmt_end()?;
                let span = lhs.span;
                Ok(Stmt::new(StmtKind::ExprStmt(lhs), span))
            }
        }
    }

    // --- Type expressions ---

    fn parse_type_expr(&mut self) -> ParseResult<TypeExpr> {
        if self.peek().kind == TokenKind::LParen {
            self.advance();
            let mut types = Vec::new();
            while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
                types.push(self.parse_type_expr()?);
                if self.peek().kind == TokenKind::Comma { self.advance(); } else { break; }
            }
            self.expect(TokenKind::RParen)?;
            return Ok(TypeExpr::Tuple(types));
        }

        if self.peek().kind == TokenKind::Fn {
            self.advance();
            self.expect(TokenKind::LParen)?;
            let mut params = Vec::new();
            while self.peek().kind != TokenKind::RParen && self.peek().kind != TokenKind::Eof {
                params.push(self.parse_type_expr()?);
                if self.peek().kind == TokenKind::Comma { self.advance(); } else { break; }
            }
            self.expect(TokenKind::RParen)?;
            let ret = if self.peek().kind == TokenKind::Arrow {
                self.advance();
                Box::new(self.parse_type_expr()?)
            } else {
                Box::new(TypeExpr::Named("None".to_string()))
            };
            return Ok(TypeExpr::Fn { params, ret });
        }

        if self.peek().kind == TokenKind::Ampersand {
            self.advance();
            if self.peek().kind == TokenKind::Mut {
                self.advance();
                return Ok(TypeExpr::MutRef(Box::new(self.parse_type_expr()?)));
            } else {
                return Ok(TypeExpr::Ref(Box::new(self.parse_type_expr()?)));
            }
        }

        let tok = self.peek().clone();
        let name = self.expect(TokenKind::Identifier).map_err(|_| ParseError {
            message: format!("expected type, got {:?} {:?}", tok.kind, tok.value),
            line: tok.line,
            col: tok.col,
            start: tok.span.0,
            end: tok.span.1,
        })?.value;


        if self.peek().kind == TokenKind::LBracket {
            self.advance();
            let mut args = Vec::new();
            while self.peek().kind != TokenKind::RBracket && self.peek().kind != TokenKind::Eof {
                args.push(self.parse_type_expr()?);
                if self.peek().kind == TokenKind::Comma { self.advance(); } else { break; }
            }
            self.expect(TokenKind::RBracket)?;
            Ok(TypeExpr::Generic { name, args })
        } else {
            Ok(TypeExpr::Named(name))
        }
    }

    // --- Expressions ---
    //
    // Precedence (lowest → highest):
    //   walrus (:=) > or > and > not > comparison > add > mul > unary > power(**) > postfix > primary

    fn parse_expr(&mut self) -> ParseResult<Expr> {
        if self.peek().kind == TokenKind::Identifier
            && self.peek_at(1).kind == TokenKind::ColonEqual
        {
            let tok = self.peek().clone();
            let name = self.advance().value;
            self.advance(); // :=
            let value = self.parse_or()?;
            let span = tok.span;
            let end = value.span.end;
            return Ok(Expr::new(ExprKind::Walrus { name, value: Box::new(value) },
                Span { file_id: tok.file_id, line: tok.line, col: tok.col, start: span.0, end }));
        }
        self.parse_or()
    }

    fn parse_or(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_and()?;
        while self.peek().kind == TokenKind::Or {
            self.advance();
            let right = self.parse_and()?;
            let span = left.span.merge(right.span);
            left = Expr::new(ExprKind::BinOp { left: Box::new(left), op: BinOp::Or, right: Box::new(right) }, span);
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_not()?;
        while self.peek().kind == TokenKind::And {
            self.advance();
            let right = self.parse_not()?;
            let span = left.span.merge(right.span);
            left = Expr::new(ExprKind::BinOp { left: Box::new(left), op: BinOp::And, right: Box::new(right) }, span);
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> ParseResult<Expr> {
        if self.peek().kind == TokenKind::Not {
            let start = self.peek().clone();
            self.advance();
            let operand = self.parse_not()?;
            let span = self.span_from(&start);
            Ok(Expr::new(ExprKind::UnaryOp { op: UnaryOp::Not, operand: Box::new(operand) }, span))
        } else {
            self.parse_comparison()
        }
    }

    fn parse_comparison(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_add()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::DoubleEqual => { self.advance(); BinOp::Eq }
                TokenKind::Is => {
                    self.advance();
                    if self.peek().kind == TokenKind::Not {
                        self.advance();
                        BinOp::IsNot
                    } else {
                        BinOp::Is
                    }
                }
                TokenKind::NotEqual => { self.advance(); BinOp::NotEq }
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
                TokenKind::Less         => { self.advance(); BinOp::Lt }
                TokenKind::LessEqual    => { self.advance(); BinOp::LtEq }
                TokenKind::Greater      => { self.advance(); BinOp::Gt }
                TokenKind::GreaterEqual => { self.advance(); BinOp::GtEq }
                TokenKind::In           => { self.advance(); BinOp::In }
                _ => break,
            };
            let right = self.parse_add()?;
            let span = left.span.merge(right.span);
            left = Expr::new(ExprKind::BinOp { left: Box::new(left), op, right: Box::new(right) }, span);
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Plus  => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul()?;
            let span = left.span.merge(right.span);
            left = Expr::new(ExprKind::BinOp { left: Box::new(left), op, right: Box::new(right) }, span);
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Star        => BinOp::Mul,
                TokenKind::Slash       => BinOp::Div,
                TokenKind::DoubleSlash => BinOp::FloorDiv,
                TokenKind::Percent     => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary()?;
            let span = left.span.merge(right.span);
            left = Expr::new(ExprKind::BinOp { left: Box::new(left), op, right: Box::new(right) }, span);
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> ParseResult<Expr> {
        match self.peek().kind {
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
                Ok(Expr::new(ExprKind::UnaryOp { op: UnaryOp::Neg, operand: Box::new(operand) }, span))
            }
            TokenKind::Plus => {
                let start = self.peek().clone();
                self.advance();
                let operand = self.parse_unary()?;
                let span = self.span_from(&start);
                Ok(Expr::new(ExprKind::UnaryOp { op: UnaryOp::Pos, operand: Box::new(operand) }, span))
            }
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> ParseResult<Expr> {
        let base = self.parse_postfix()?;
        if self.peek().kind == TokenKind::DoubleStar {
            self.advance();
            let exp = self.parse_unary()?;
            let span = base.span.merge(exp.span);
            Ok(Expr::new(ExprKind::BinOp { left: Box::new(base), op: BinOp::Pow, right: Box::new(exp) }, span))
        } else {
            Ok(base)
        }
    }

    fn parse_postfix(&mut self) -> ParseResult<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek().kind {
                TokenKind::Dot => {
                    self.advance();
                    let attr = self.expect(TokenKind::Identifier)?.value;
                    let span = self.span_from(&Token {
                        kind: TokenKind::Identifier,
                        value: String::new(),
                        line: expr.span.line,
                        col: expr.span.col,
                        span: (expr.span.start, expr.span.end),
                        file_id: expr.span.file_id,
                    });
                    expr = Expr::new(ExprKind::Attr { obj: Box::new(expr), attr }, span);
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
                    expr = Expr::new(ExprKind::Index { obj: Box::new(expr), index: Box::new(index) }, span);
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
                    expr = Expr::new(ExprKind::Call { callee: Box::new(expr), args }, span);
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_call_args(&mut self) -> ParseResult<Vec<CallArg>> {
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
            if self.peek().kind == TokenKind::Comma { self.advance(); } else { break; }
        }
        Ok(args)
    }

    fn parse_comp_clauses(&mut self) -> ParseResult<Vec<CompClause>> {
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
            clauses.push(CompClause { target, iter, condition });
        }
        Ok(clauses)
    }

    fn parse_primary(&mut self) -> ParseResult<Expr> {
        let tok = self.peek().clone();
        let start = Span { file_id: tok.file_id, line: tok.line, col: tok.col, start: tok.span.0, end: tok.span.1 };
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
                val.map(|n| Expr::new(ExprKind::Integer(n), start)).map_err(|_| {
                    self.err_at(&tok, format!("integer literal '{}' out of i64 range", tok.value))
                })
            }
            TokenKind::Float => {
                self.advance();
                tok.value.parse::<f64>()
                    .map(|f| Expr::new(ExprKind::Float(f), start))
                    .map_err(|_| self.err_at(&tok, format!("invalid float literal '{}'", tok.value)))
            }
            TokenKind::String     => { self.advance(); Ok(Expr::new(ExprKind::Str(tok.value), start)) }
            TokenKind::True       => { self.advance(); Ok(Expr::new(ExprKind::Bool(true), start)) }
            TokenKind::False      => { self.advance(); Ok(Expr::new(ExprKind::Bool(false), start)) }
            TokenKind::Null       => { self.advance(); Ok(Expr::new(ExprKind::Null, start)) }
            TokenKind::Identifier => { self.advance(); Ok(Expr::new(ExprKind::Identifier(tok.value), start)) }

            TokenKind::LParen => {
                self.advance();
                if self.peek().kind == TokenKind::RParen {
                    let end = self.peek().span.1;
                    self.advance();
                    return Ok(Expr::new(ExprKind::Tuple(vec![]),
                        Span { end, ..start }));
                }
                let first = self.parse_expr()?;
                if self.peek().kind == TokenKind::Comma {
                    let mut elems = vec![first];
                    while self.peek().kind == TokenKind::Comma {
                        self.advance();
                        if self.peek().kind == TokenKind::RParen { break; }
                        elems.push(self.parse_expr()?);
                    }
                    let end = self.peek().span.1;
                    self.expect(TokenKind::RParen)?;
                    Ok(Expr::new(ExprKind::Tuple(elems), Span { end, ..start }))
                } else {
                    self.expect(TokenKind::RParen)?;
                    Ok(first) // grouping — unwrap, keep inner span
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
                    Ok(Expr::new(ExprKind::ListComp { elt: Box::new(first), clauses },
                        Span { end, ..start }))
                } else {
                    let mut elems = vec![first];
                    while self.peek().kind == TokenKind::Comma {
                        self.advance();
                        if self.peek().kind == TokenKind::RBracket { break; }
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
                            Ok(Expr::new(ExprKind::DictComp {
                                key: Box::new(first),
                                value: Box::new(first_val),
                                clauses,
                            }, Span { end, ..start }))
                        } else {
                            let mut pairs = vec![(first, first_val)];
                            while self.peek().kind == TokenKind::Comma {
                                self.advance();
                                if self.peek().kind == TokenKind::RBrace { break; }
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
                        Ok(Expr::new(ExprKind::SetComp { elt: Box::new(first), clauses },
                            Span { end, ..start }))
                    }
                    _ => {
                        let mut elems = vec![first];
                        while self.peek().kind == TokenKind::Comma {
                            self.advance();
                            if self.peek().kind == TokenKind::RBrace { break; }
                            elems.push(self.parse_expr()?);
                        }
                        let end = self.peek().span.1;
                        self.expect(TokenKind::RBrace)?;
                        Ok(Expr::new(ExprKind::Set(elems), Span { end, ..start }))
                    }
                }
            }

            _ => Err(ParseError {
                message: format!("unexpected token {:?} {:?}", tok.kind, tok.value),
                line: tok.line,
                col: tok.col,
                start: tok.span.0,
                end: tok.span.1,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn parse(src: &str) -> Program {
        let tokens = Lexer::new(src, 0).tokenise().expect("lex error");
        Parser::new(tokens).parse_program().expect("parse error")
    }

    fn parse_err(src: &str) -> String {
        let tokens = Lexer::new(src, 0).tokenise().expect("lex error");
        Parser::new(tokens).parse_program().unwrap_err().message
    }

    fn first(p: &Program) -> &StmtKind {
        &p.stmts.first().expect("empty program").kind
    }

    fn expr_stmt(p: &Program) -> &ExprKind {
        match first(p) {
            StmtKind::ExprStmt(e) => &e.kind,
            _ => panic!("expected ExprStmt"),
        }
    }

    // ── literals ──────────────────────────────────────────────────────────────

    #[test]
    fn integer_literal() {
        assert!(matches!(expr_stmt(&parse("42\n")), ExprKind::Integer(42)));
    }

    #[test]
    fn hex_oct_bin_literals() {
        assert!(matches!(expr_stmt(&parse("0xFF\n")),   ExprKind::Integer(255)));
        assert!(matches!(expr_stmt(&parse("0o77\n")),   ExprKind::Integer(63)));
        assert!(matches!(expr_stmt(&parse("0b1010\n")), ExprKind::Integer(10)));
    }

    #[test]
    fn float_literal() {
        assert!(matches!(expr_stmt(&parse("3.14\n")), ExprKind::Float(_)));
    }

    #[test]
    fn string_literal() {
        assert!(matches!(expr_stmt(&parse("\"hello\"\n")), ExprKind::Str(s) if s == "hello"));
    }

    #[test]
    fn bool_null_literals() {
        assert!(matches!(expr_stmt(&parse("True\n")),  ExprKind::Bool(true)));
        assert!(matches!(expr_stmt(&parse("False\n")), ExprKind::Bool(false)));
        assert!(matches!(expr_stmt(&parse("None\n")),  ExprKind::Null));
    }

    // ── arithmetic precedence ─────────────────────────────────────────────────

    #[test]
    fn additive_left_assoc() {
        match expr_stmt(&parse("1 + 2 - 3\n")) {
            ExprKind::BinOp { op: BinOp::Sub, left, .. } =>
                assert!(matches!(left.kind, ExprKind::BinOp { op: BinOp::Add, .. })),
            _ => panic!(),
        }
    }

    #[test]
    fn mul_over_add() {
        match expr_stmt(&parse("1 + 2 * 3\n")) {
            ExprKind::BinOp { op: BinOp::Add, right, .. } =>
                assert!(matches!(right.kind, ExprKind::BinOp { op: BinOp::Mul, .. })),
            _ => panic!(),
        }
    }

    #[test]
    fn power_right_assoc() {
        match expr_stmt(&parse("2**3**2\n")) {
            ExprKind::BinOp { op: BinOp::Pow, right, .. } =>
                assert!(matches!(right.kind, ExprKind::BinOp { op: BinOp::Pow, .. })),
            _ => panic!(),
        }
    }

    #[test]
    fn unary_neg_over_power() {
        match expr_stmt(&parse("-2**2\n")) {
            ExprKind::UnaryOp { op: UnaryOp::Neg, operand } =>
                assert!(matches!(operand.kind, ExprKind::BinOp { op: BinOp::Pow, .. })),
            _ => panic!(),
        }
    }

    #[test]
    fn unary_pos() {
        assert!(matches!(expr_stmt(&parse("+x\n")),
            ExprKind::UnaryOp { op: UnaryOp::Pos, .. }));
    }

    #[test]
    fn floor_div_mod() {
        match expr_stmt(&parse("a // b % c\n")) {
            ExprKind::BinOp { op: BinOp::Mod, left, .. } =>
                assert!(matches!(left.kind, ExprKind::BinOp { op: BinOp::FloorDiv, .. })),
            _ => panic!(),
        }
    }

    // ── comparison / logical ──────────────────────────────────────────────────

    #[test]
    fn comparison_ops() {
        for src in ["a == b\n","a != b\n","a < b\n","a <= b\n","a > b\n","a >= b\n"] {
            parse(src);
        }
    }

    #[test]
    fn in_not_in_operators() {
        assert!(matches!(expr_stmt(&parse("x in [1,2]\n")),
            ExprKind::BinOp { op: BinOp::In, .. }));
        assert!(matches!(expr_stmt(&parse("x not in [1,2]\n")),
            ExprKind::BinOp { op: BinOp::NotIn, .. }));
    }

    #[test]
    fn logical_not_in_combination() {
        match expr_stmt(&parse("not x in y\n")) {
            ExprKind::UnaryOp { op: UnaryOp::Not, operand } =>
                assert!(matches!(operand.kind, ExprKind::BinOp { op: BinOp::In, .. })),
            _ => panic!(),
        }
    }

    #[test]
    fn logical_and_or_precedence() {
        match expr_stmt(&parse("a and b or c\n")) {
            ExprKind::BinOp { op: BinOp::Or, left, .. } =>
                assert!(matches!(left.kind, ExprKind::BinOp { op: BinOp::And, .. })),
            _ => panic!(),
        }
    }

    // ── postfix ───────────────────────────────────────────────────────────────

    #[test]
    fn call_no_args() {
        assert!(matches!(expr_stmt(&parse("f()\n")),
            ExprKind::Call { args, .. } if args.is_empty()));
    }

    #[test]
    fn call_splat_kwsplat() {
        let p = parse("f(*a, **b)\n");
        match expr_stmt(&p) {
            ExprKind::Call { args, .. } => {
                assert!(matches!(args[0], CallArg::Splat(_)));
                assert!(matches!(args[1], CallArg::KwSplat(_)));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn call_keyword_arg() {
        match expr_stmt(&parse("f(x=1)\n")) {
            ExprKind::Call { args, .. } =>
                assert!(matches!(&args[0], CallArg::Keyword(name, _) if name == "x")),
            _ => panic!(),
        }
    }

    #[test]
    fn attribute_chain() {
        match expr_stmt(&parse("a.b.c\n")) {
            ExprKind::Attr { obj, attr } => {
                assert_eq!(attr, "c");
                assert!(matches!(obj.kind, ExprKind::Attr { .. }));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn index_expr() {
        assert!(matches!(expr_stmt(&parse("a[0]\n")), ExprKind::Index { .. }));
    }

    // ── collections ──────────────────────────────────────────────────────────

    #[test]
    fn list_literal() {
        match expr_stmt(&parse("[1,2,3]\n")) {
            ExprKind::List(v) => assert_eq!(v.len(), 3),
            _ => panic!(),
        }
    }

    #[test]
    fn set_literal() {
        match expr_stmt(&parse("{1,2,3}\n")) {
            ExprKind::Set(v) => assert_eq!(v.len(), 3),
            _ => panic!(),
        }
    }

    #[test]
    fn tuple_forms() {
        assert!(matches!(expr_stmt(&parse("()\n")), ExprKind::Tuple(v) if v.is_empty()));
        assert!(matches!(expr_stmt(&parse("(1,)\n")), ExprKind::Tuple(v) if v.len() == 1));
        assert!(matches!(expr_stmt(&parse("(1)\n")), ExprKind::Integer(1)));
    }

    #[test]
    fn dict_empty_and_literal() {
        assert!(matches!(expr_stmt(&parse("{}\n")), ExprKind::Dict(p) if p.is_empty()));
        match expr_stmt(&parse("{\"a\":1,\"b\":2}\n")) {
            ExprKind::Dict(pairs) => assert_eq!(pairs.len(), 2),
            _ => panic!(),
        }
    }

    // ── comprehensions ────────────────────────────────────────────────────────

    #[test]
    fn list_comprehension() {
        match expr_stmt(&parse("[x * 2 for x in items]\n")) {
            ExprKind::ListComp { clauses, .. } => assert_eq!(clauses.len(), 1),
            _ => panic!(),
        }
    }

    #[test]
    fn list_comp_with_condition() {
        match expr_stmt(&parse("[x for x in items if x > 0]\n")) {
            ExprKind::ListComp { clauses, .. } => assert!(clauses[0].condition.is_some()),
            _ => panic!(),
        }
    }

    #[test]
    fn nested_list_comp() {
        match expr_stmt(&parse("[x for row in grid for x in row]\n")) {
            ExprKind::ListComp { clauses, .. } => assert_eq!(clauses.len(), 2),
            _ => panic!(),
        }
    }

    #[test]
    fn set_comprehension() {
        assert!(matches!(expr_stmt(&parse("{x for x in items}\n")), ExprKind::SetComp { .. }));
    }

    #[test]
    fn dict_comprehension() {
        match expr_stmt(&parse("{k: v for k, v in pairs}\n")) {
            ExprKind::DictComp { clauses, .. } =>
                assert!(matches!(clauses[0].target, ForTarget::Tuple(..))),
            _ => panic!(),
        }
    }

    // ── statements ───────────────────────────────────────────────────────────

    #[test]
    fn let_stmt() {
        assert!(matches!(first(&parse("let x = 42\n")), StmtKind::Let { name, .. } if name == "x"));
    }

    #[test]
    fn let_with_type_ann() {
        match first(&parse("let x: i64 = 42\n")) {
            StmtKind::Let { name, type_ann, .. } => {
                assert_eq!(name, "x");
                assert!(matches!(type_ann, Some(TypeExpr::Named(t)) if t == "i64"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn assign_stmt() {
        assert!(matches!(first(&parse("x = 1\n")), StmtKind::Assign { .. }));
    }

    #[test]
    fn tuple_unpack_assign() {
        match first(&parse("a, b = 1, 2\n")) {
            StmtKind::Assign { target, value } => {
                assert!(matches!(target.kind, ExprKind::Tuple(ref lhs) if lhs.len() == 2));
                assert!(matches!(value.kind, ExprKind::Tuple(ref rhs) if rhs.len() == 2));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn tuple_unpack_trailing_comma() {
        match first(&parse("a, b, = 1, 2\n")) {
            StmtKind::Assign { target, .. } =>
                assert!(matches!(target.kind, ExprKind::Tuple(ref lhs) if lhs.len() == 2)),
            _ => panic!(),
        }
    }

    #[test]
    fn tuple_unpack_paren_lhs() {
        match first(&parse("(a, b) = 1, 2\n")) {
            StmtKind::Assign { target, value } => {
                assert!(matches!(target.kind, ExprKind::Tuple(ref lhs) if lhs.len() == 2));
                assert!(matches!(value.kind, ExprKind::Tuple(ref rhs) if rhs.len() == 2));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn invalid_assign_target() {
        assert!(parse_err("1 + 2 = 3\n").contains("invalid assignment target"));
    }

    #[test]
    fn invalid_tuple_assign_target() {
        assert!(parse_err("a, 1 = x, y\n").contains("invalid assignment target"));
    }

    #[test]
    fn aug_assign_all_ops() {
        for (src, expected) in [
            ("x += 1\n",  AugOp::Add),
            ("x -= 1\n",  AugOp::Sub),
            ("x *= 1\n",  AugOp::Mul),
            ("x /= 1\n",  AugOp::Div),
            ("x //= 1\n", AugOp::FloorDiv),
            ("x %= 1\n",  AugOp::Mod),
            ("x **= 1\n", AugOp::Pow),
        ] {
            match first(&parse(src)) {
                StmtKind::AugAssign { op, .. } => assert_eq!(*op, expected, "src={src}"),
                _ => panic!("not AugAssign for {src}"),
            }
        }
    }

    #[test]
    fn invalid_aug_assign_target() {
        assert!(parse_err("1 += 2\n").contains("invalid augmented assignment target"));
    }

    #[test]
    fn pass_break_continue() {
        assert!(matches!(first(&parse("pass\n")),     StmtKind::Pass));
        assert!(matches!(first(&parse("break\n")),    StmtKind::Break));
        assert!(matches!(first(&parse("continue\n")), StmtKind::Continue));
    }

    #[test]
    fn return_stmt() {
        assert!(matches!(first(&parse("fn f():\n    return 1\n")),
            StmtKind::Fn { body, .. } if matches!(body[0].kind, StmtKind::Return(Some(_)))));
        assert!(matches!(first(&parse("fn f():\n    return\n")),
            StmtKind::Fn { body, .. } if matches!(body[0].kind, StmtKind::Return(None))));
    }

    #[test]
    fn raise_stmt() {
        assert!(matches!(first(&parse("raise ValueError\n")),  StmtKind::Raise(Some(_))));
        assert!(matches!(first(&parse("raise\n")),             StmtKind::Raise(None)));
    }

    #[test]
    fn assert_stmt() {
        assert!(matches!(first(&parse("assert x > 0\n")), StmtKind::Assert { msg: None, .. }));
        assert!(matches!(first(&parse("assert x > 0, \"bad\"\n")), StmtKind::Assert { msg: Some(_), .. }));
    }

    // ── single-line blocks ────────────────────────────────────────────────────

    #[test]
    fn single_line_if() {
        match first(&parse("if x: pass\n")) {
            StmtKind::If { then_body, .. } => assert!(matches!(then_body[0].kind, StmtKind::Pass)),
            _ => panic!(),
        }
    }

    #[test]
    fn single_line_while() {
        match first(&parse("while True: break\n")) {
            StmtKind::While { body, .. } => assert!(matches!(body[0].kind, StmtKind::Break)),
            _ => panic!(),
        }
    }

    // ── control flow ─────────────────────────────────────────────────────────

    #[test]
    fn if_else_if_else() {
        match first(&parse("if a:\n    x\nelse if b:\n    y\nelse:\n    z\n")) {
            StmtKind::If { elif_clauses, else_body, .. } => {
                assert_eq!(elif_clauses.len(), 1);
                assert!(else_body.is_some());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn while_else() {
        match first(&parse("while x > 0:\n    x -= 1\nelse:\n    pass\n")) {
            StmtKind::While { else_body, .. } => assert!(else_body.is_some()),
            _ => panic!(),
        }
    }

    #[test]
    fn for_loop_simple() {
        match first(&parse("for i in items:\n    pass\n")) {
            StmtKind::For { target: ForTarget::Name(n, _), .. } => assert_eq!(n, "i"),
            _ => panic!(),
        }
    }

    #[test]
    fn for_tuple_unpacking_paren() {
        match first(&parse("for (a, b) in pairs:\n    pass\n")) {
            StmtKind::For { target: ForTarget::Tuple(names, _), .. } => {
                let ns: Vec<&str> = names.iter().map(|(n, _)| n.as_str()).collect();
                assert_eq!(ns, vec!["a", "b"]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn for_tuple_unpacking_bare() {
        match first(&parse("for a, b in pairs:\n    pass\n")) {
            StmtKind::For { target: ForTarget::Tuple(names, _), .. } => {
                let ns: Vec<&str> = names.iter().map(|(n, _)| n.as_str()).collect();
                assert_eq!(ns, vec!["a", "b"]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn for_else() {
        match first(&parse("for i in x:\n    pass\nelse:\n    pass\n")) {
            StmtKind::For { else_body, .. } => assert!(else_body.is_some()),
            _ => panic!(),
        }
    }

    // ── try / except / finally ────────────────────────────────────────────────

    #[test]
    fn try_except_basic() {
        match first(&parse("try:\n    x\nexcept:\n    pass\n")) {
            StmtKind::Try { handlers, else_body, finally_body, .. } => {
                assert_eq!(handlers.len(), 1);
                assert!(handlers[0].exc_type.is_none());
                assert!(else_body.is_none());
                assert!(finally_body.is_none());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn try_except_typed_as() {
        match first(&parse("try:\n    x\nexcept ValueError as e:\n    pass\n")) {
            StmtKind::Try { handlers, .. } => {
                assert!(handlers[0].exc_type.is_some());
                assert_eq!(handlers[0].name.as_deref(), Some("e"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn try_multiple_except() {
        match first(&parse("try:\n    x\nexcept A:\n    pass\nexcept B:\n    pass\n")) {
            StmtKind::Try { handlers, .. } => assert_eq!(handlers.len(), 2),
            _ => panic!(),
        }
    }

    #[test]
    fn try_except_else_finally() {
        match first(&parse(
            "try:\n    x\nexcept E:\n    pass\nelse:\n    pass\nfinally:\n    pass\n"
        )) {
            StmtKind::Try { handlers, else_body, finally_body, .. } => {
                assert_eq!(handlers.len(), 1);
                assert!(else_body.is_some());
                assert!(finally_body.is_some());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn try_finally_only() {
        match first(&parse("try:\n    x\nfinally:\n    cleanup\n")) {
            StmtKind::Try { handlers, finally_body, .. } => {
                assert!(handlers.is_empty());
                assert!(finally_body.is_some());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn try_without_except_or_finally_errors() {
        assert!(parse_err("try:\n    x\n").contains("try without except or finally"));
    }

    // ── functions & classes ───────────────────────────────────────────────────

    #[test]
    fn fn_vararg_kwarg() {
        match first(&parse("fn f(a, *args, **kwargs):\n    pass\n")) {
            StmtKind::Fn { params, .. } => {
                assert_eq!(params[0].kind, ParamKind::Regular);
                assert_eq!(params[1].kind, ParamKind::VarArg);
                assert_eq!(params[2].kind, ParamKind::KwArg);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn fn_with_defaults_and_return_type() {
        match first(&parse("fn add(x, y = 0) -> int:\n    return x\n")) {
            StmtKind::Fn { name, params, return_type, .. } => {
                assert_eq!(name, "add");
                assert_eq!(params.len(), 2);
                assert!(params[1].default.is_some());
                assert!(return_type.is_some());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn fn_typed_params() {
        match first(&parse("fn add(x: i64, y: i64) -> i64:\n    return x\n")) {
            StmtKind::Fn { params, return_type, .. } => {
                assert!(matches!(&params[0].type_ann, Some(TypeExpr::Named(t)) if t == "i64"));
                assert!(matches!(return_type, Some(TypeExpr::Named(t)) if t == "i64"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn class_with_bases() {
        match first(&parse("class Dog(Animal):\n    pass\n")) {
            StmtKind::Class { name, bases, .. } => {
                assert_eq!(name, "Dog");
                assert_eq!(bases.len(), 1);
            }
            _ => panic!(),
        }
    }

    // ── imports ───────────────────────────────────────────────────────────────

    #[test]
    fn import_dotted() {
        match first(&parse("import std.io\n")) {
            StmtKind::Import(path) => assert_eq!(path, &["std", "io"]),
            _ => panic!(),
        }
    }

    #[test]
    fn from_import_multi() {
        match first(&parse("from std.io import read, write\n")) {
            StmtKind::FromImport { module, names } => {
                assert_eq!(module, &["std", "io"]);
                assert_eq!(names, &["read", "write"]);
            }
            _ => panic!(),
        }
    }

    // ── walrus ────────────────────────────────────────────────────────────────

    #[test]
    fn walrus_operator() {
        assert!(matches!(expr_stmt(&parse("x := 5\n")),
            ExprKind::Walrus { name, .. } if name == "x"));
    }

    // ── spans ─────────────────────────────────────────────────────────────────

    #[test]
    fn span_line_col() {
        let p = parse("let x = 42\n");
        let stmt = p.stmts.first().unwrap();
        assert_eq!(stmt.span.line, 1);
        assert_eq!(stmt.span.col, 1);
    }

    #[test]
    fn expr_span_identifier() {
        let p = parse("foo\n");
        match expr_stmt(&p) {
            ExprKind::Identifier(name) => assert_eq!(name, "foo"),
            _ => panic!(),
        }
        let stmt = p.stmts.first().unwrap();
        assert_eq!(stmt.span.line, 1);
    }

    // ── lexer errors ─────────────────────────────────────────────────────────

    #[test]
    fn unclosed_paren_lex_error() {
        let r = Lexer::new("f(1", 0).tokenise();
        assert!(r.is_err());
        assert!(r.unwrap_err().message.contains("unclosed '('"));
    }

    #[test]
    fn mismatched_delimiter_lex_error() {
        assert!(Lexer::new("f(1]", 0).tokenise().is_err());
    }

    #[test]
    fn invalid_hex_literal() {
        assert!(Lexer::new("0xGG", 0).tokenise().is_err());
    }

    #[test]
    fn unexpected_token_parse_error() {
        let tokens = Lexer::new("fn 42():\n    pass\n", 0).tokenise().unwrap();
        assert!(Parser::new(tokens).parse_program().is_err());
    }
}
