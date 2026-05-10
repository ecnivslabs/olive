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
        let start = self.parse_or()?;
        if matches!(
            self.peek().kind,
            crate::lexer::TokenKind::DotDot | crate::lexer::TokenKind::DotDotEq
        ) {
            let inclusive = self.peek().kind == crate::lexer::TokenKind::DotDotEq;
            self.advance();
            let end = self.parse_or()?;
            let span = start.span.merge(end.span);
            return Ok(Expr::new(
                ExprKind::Range {
                    start: Box::new(start),
                    end: Box::new(end),
                    inclusive,
                },
                span,
            ));
        }
        Ok(start)
    }
}
