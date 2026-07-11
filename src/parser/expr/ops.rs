use super::super::{Parser, ast::*, error::ParseResult};
use crate::lexer::TokenKind;

impl Parser {
    pub(crate) fn parse_or(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_coalesce()?;
        while self.peek().kind == TokenKind::Or {
            self.advance();
            let right = self.parse_coalesce()?;
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

    pub(crate) fn parse_coalesce(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_and()?;
        while self.peek().kind == TokenKind::QuestionQuestion {
            self.advance();
            let right = self.parse_and()?;
            let span = left.span.merge(right.span);
            left = Expr::new(
                ExprKind::BinOp {
                    left: Box::new(left),
                    op: BinOp::Coalesce,
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
        let mut left = self.parse_bitor()?;
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
            let mut right = self.parse_bitor()?;
            // `x in a..b` / `x not in a..b`: the range binds to `in`'s right
            // operand specifically, without changing `..`'s precedence
            // anywhere else (a bare `a..b` still sits above `or`).
            if matches!(op, BinOp::In | BinOp::NotIn)
                && matches!(self.peek().kind, TokenKind::DotDot | TokenKind::DotDotEq)
            {
                let inclusive = self.peek().kind == TokenKind::DotDotEq;
                self.advance();
                let end = self.parse_bitor()?;
                let range_span = right.span.merge(end.span);
                right = Expr::new(
                    ExprKind::Range {
                        start: Box::new(right),
                        end: Box::new(end),
                        inclusive,
                    },
                    range_span,
                );
            }
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

    pub(crate) fn parse_bitor(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_bitxor()?;
        while self.peek().kind == TokenKind::Pipe {
            self.advance();
            let right = self.parse_bitxor()?;
            let span = left.span.merge(right.span);
            left = Expr::new(
                ExprKind::BinOp {
                    left: Box::new(left),
                    op: BinOp::BitOr,
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    pub(crate) fn parse_bitxor(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_bitand()?;
        while self.peek().kind == TokenKind::Caret {
            self.advance();
            let right = self.parse_bitand()?;
            let span = left.span.merge(right.span);
            left = Expr::new(
                ExprKind::BinOp {
                    left: Box::new(left),
                    op: BinOp::BitXor,
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    pub(crate) fn parse_bitand(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_shift()?;
        while self.peek().kind == TokenKind::Ampersand {
            self.advance();
            let right = self.parse_shift()?;
            let span = left.span.merge(right.span);
            left = Expr::new(
                ExprKind::BinOp {
                    left: Box::new(left),
                    op: BinOp::BitAnd,
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    pub(crate) fn parse_shift(&mut self) -> ParseResult<Expr> {
        let mut left = self.parse_add()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Shl => BinOp::Shl,
                TokenKind::Shr => BinOp::Shr,
                _ => break,
            };
            self.advance();
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
            TokenKind::Await => {
                let start = self.peek().clone();
                self.advance();
                let operand = self.parse_unary()?;
                let span = self.span_from(&start);
                Ok(Expr::new(ExprKind::Await(Box::new(operand)), span))
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
            TokenKind::Tilde => {
                let start = self.peek().clone();
                self.advance();
                let operand = self.parse_unary()?;
                let span = self.span_from(&start);
                Ok(Expr::new(
                    ExprKind::UnaryOp {
                        op: UnaryOp::BitNot,
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }
            TokenKind::Star => {
                let start = self.peek().clone();
                self.advance();
                let operand = self.parse_unary()?;
                let span = self.span_from(&start);
                Ok(Expr::new(ExprKind::Deref(Box::new(operand)), span))
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
}
