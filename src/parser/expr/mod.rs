use super::{Parser, ast::*, error::ParseResult};
use crate::lexer::TokenKind;

pub(crate) mod ops;
pub(crate) mod postfix;
pub(crate) mod primary;

#[cfg(test)]
pub(crate) mod tests;

impl Parser {
    pub(crate) fn is_valid_assign_target(expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Identifier(_)
            | ExprKind::Attr { .. }
            | ExprKind::Index { .. }
            | ExprKind::Deref(_) => true,
            ExprKind::Starred(inner) => matches!(inner.kind, ExprKind::Identifier(_)),
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

    /// `by` is contextual, not a reserved word: only consumed right after a
    /// range end, recognized by text on an ordinary `Identifier` token.
    pub(crate) fn parse_optional_range_step(&mut self) -> ParseResult<Option<Box<Expr>>> {
        if self.peek().kind == TokenKind::Identifier && self.peek().value == "by" {
            self.advance();
            return Ok(Some(Box::new(self.parse_or()?)));
        }
        Ok(None)
    }

    /// Parses an `or`-expression with an optional `..`/`..=` range suffix, but
    /// no ternary. Used where a trailing `if` belongs to an enclosing construct
    /// (a comprehension's filter clause) rather than a conditional expression.
    pub(crate) fn parse_range(&mut self) -> ParseResult<Expr> {
        let start = self.parse_or()?;
        if matches!(
            self.peek().kind,
            crate::lexer::TokenKind::DotDot | crate::lexer::TokenKind::DotDotEq
        ) {
            let inclusive = self.peek().kind == crate::lexer::TokenKind::DotDotEq;
            self.advance();
            let end = self.parse_or()?;
            let step = self.parse_optional_range_step()?;
            let span = match &step {
                Some(s) => start.span.merge(s.span),
                None => start.span.merge(end.span),
            };
            return Ok(Expr::new(
                ExprKind::Range {
                    start: Box::new(start),
                    end: Box::new(end),
                    inclusive,
                    step,
                },
                span,
            ));
        }
        Ok(start)
    }

    pub(crate) fn parse_expr(&mut self) -> ParseResult<Expr> {
        let start = self.parse_range()?;
        // Conditional expression `value if cond else other` (Python ternary).
        if self.peek().kind == crate::lexer::TokenKind::If {
            self.advance();
            let cond = self.parse_or()?;
            self.expect(crate::lexer::TokenKind::Else)?;
            let otherwise = self.parse_expr()?;
            let span = start.span.merge(otherwise.span);
            return Ok(Expr::new(
                ExprKind::Ternary {
                    cond: Box::new(cond),
                    then: Box::new(start),
                    otherwise: Box::new(otherwise),
                },
                span,
            ));
        }
        Ok(start)
    }
}
