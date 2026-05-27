use super::doc::*;
use super::lower::Lowerer;
use super::syntax::{binop_prec, binop_str, expr_prec};
use crate::parser::ast::*;

impl Lowerer<'_> {
    pub(super) fn expr(&mut self, e: &Expr) -> Doc {
        match &e.kind {
            ExprKind::Integer(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::FStr(_) => {
                // Re-emit the original lexeme: the AST stores escape-processed and
                // float-rounded values, so only the source slice is lossless.
                text(self.slice(e.span.start, e.span.end))
            }
            ExprKind::Bool(b) => text(if *b { "True" } else { "False" }),
            ExprKind::Null => text("None"),
            ExprKind::Identifier(name) => text(name.clone()),

            ExprKind::BinOp { left, op, right } => {
                let p = binop_prec(op);
                let right_assoc = matches!(op, BinOp::Pow);
                let lhs = self.child(left, p, right_assoc);
                let rhs = self.child(right, p, !right_assoc);
                concat_all([lhs, text(format!(" {} ", binop_str(op))), rhs])
            }
            ExprKind::UnaryOp { op, operand } => {
                let (sym, prec) = match op {
                    UnaryOp::Neg => ("-", 11),
                    UnaryOp::Pos => ("+", 11),
                    UnaryOp::BitNot => ("~", 11),
                    UnaryOp::Not => ("not ", 3),
                };
                let inner = self.unary_operand(operand, prec);
                concat(text(sym), inner)
            }
            ExprKind::Borrow(x) => concat(text("&"), self.unary_operand(x, 11)),
            ExprKind::MutBorrow(x) => concat(text("&mut "), self.unary_operand(x, 11)),
            ExprKind::Deref(x) => concat(text("*"), self.unary_operand(x, 11)),

            ExprKind::Range {
                start,
                end,
                inclusive,
            } => concat_all([
                self.expr(start),
                text(if *inclusive { "..=" } else { ".." }),
                self.expr(end),
            ]),
            ExprKind::Try(x) => {
                if self.span_starts_with_try(e.span) {
                    concat(text("try "), self.unary_operand(x, 11))
                } else {
                    concat(self.postfix_operand(x), text("?"))
                }
            }
            ExprKind::Await(x) => {
                if self.span_starts_with_await(e.span) {
                    concat(text("await "), self.unary_operand(x, 11))
                } else {
                    concat(self.postfix_operand(x), text(".await"))
                }
            }

            ExprKind::Cast(x, ty) => {
                concat_all([self.postfix_operand(x), text(" as "), text(ty.to_string())])
            }

            ExprKind::Call { callee, args } => {
                let head = self.postfix_operand(callee);
                let arg_docs: Vec<Doc> = args.iter().map(|a| self.call_arg(a)).collect();
                concat(head, bracketed("(", arg_docs, ")"))
            }
            ExprKind::Index { obj, index } => {
                let head = self.postfix_operand(obj);
                concat_all([head, text("["), self.index_inner(index), text("]")])
            }
            ExprKind::Attr { obj, attr } => {
                concat_all([self.postfix_operand(obj), text("."), text(attr.clone())])
            }

            ExprKind::List(items) => {
                let docs: Vec<Doc> = items.iter().map(|x| self.expr(x)).collect();
                bracketed("[", docs, "]")
            }
            ExprKind::Set(items) => {
                let docs: Vec<Doc> = items.iter().map(|x| self.expr(x)).collect();
                bracketed("{", docs, "}")
            }
            ExprKind::Tuple(items) => {
                let docs: Vec<Doc> = items.iter().map(|x| self.expr(x)).collect();
                if docs.len() == 1 {
                    concat_all([text("("), docs.into_iter().next().unwrap(), text(",)")])
                } else {
                    bracketed("(", docs, ")")
                }
            }
            ExprKind::Dict(pairs) => {
                let docs: Vec<Doc> = pairs
                    .iter()
                    .map(|(k, v)| concat_all([self.expr(k), text(": "), self.expr(v)]))
                    .collect();
                bracketed("{", docs, "}")
            }

            ExprKind::ListComp { elt, clauses } => {
                let inner = concat(self.expr(elt), self.comp_clauses(clauses));
                concat_all([text("["), inner, text("]")])
            }
            ExprKind::SetComp { elt, clauses } => {
                let inner = concat(self.expr(elt), self.comp_clauses(clauses));
                concat_all([text("{"), inner, text("}")])
            }
            ExprKind::DictComp {
                key,
                value,
                clauses,
            } => {
                let head = concat_all([self.expr(key), text(": "), self.expr(value)]);
                let inner = concat(head, self.comp_clauses(clauses));
                concat_all([text("{"), inner, text("}")])
            }

            ExprKind::Slice { start, stop, step } => self.slice_expr(start, stop, step),

            ExprKind::Match { expr, cases } => {
                let head = concat_all([text("match "), self.expr(expr), text(":")]);
                let arms: Vec<Doc> = cases
                    .iter()
                    .map(|c| {
                        let pat = match &c.guard {
                            Some(g) => concat_all([
                                self.pattern(&c.pattern),
                                text(" if "),
                                self.expr(g),
                                text(":"),
                            ]),
                            None => concat(self.pattern(&c.pattern), text(":")),
                        };
                        self.suite_expr(pat, &c.body)
                    })
                    .collect();
                concat(
                    head,
                    nest(INDENT, concat(hardline(), join(hardline(), arms))),
                )
            }

            ExprKind::AsyncBlock(body) => self.suite_expr(text("async:"), body),

            ExprKind::Ternary {
                cond,
                then,
                otherwise,
            } => concat_all([
                self.expr(then),
                text(" if "),
                self.expr(cond),
                text(" else "),
                self.expr(otherwise),
            ]),
            ExprKind::Lambda { params, body } => {
                let param_docs: Vec<Doc> = params.iter().map(|p| self.param(p)).collect();
                concat_all([
                    text("lambda"),
                    if params.is_empty() {
                        nil()
                    } else {
                        concat(text(" "), join(text(", "), param_docs))
                    },
                    text(": "),
                    self.expr(body),
                ])
            }
        }
    }

    fn unary_operand(&mut self, e: &Expr, prec: u8) -> Doc {
        let inner = self.expr(e);
        if expr_prec(e) < prec {
            concat_all([text("("), inner, text(")")])
        } else {
            inner
        }
    }

    /// Operand of a postfix construct (call/index/attr/cast). Anything that binds
    /// looser than postfix must be parenthesized to keep the same parse.
    fn postfix_operand(&mut self, e: &Expr) -> Doc {
        let inner = self.expr(e);
        if expr_prec(e) < 13 {
            concat_all([text("("), inner, text(")")])
        } else {
            inner
        }
    }

    fn call_arg(&mut self, a: &CallArg) -> Doc {
        match a {
            CallArg::Positional(e) => self.expr(e),
            CallArg::Keyword(name, e) => concat_all([text(name.clone()), text("="), self.expr(e)]),
            CallArg::Splat(e) => concat(text("*"), self.expr(e)),
            CallArg::KwSplat(e) => concat(text("**"), self.expr(e)),
        }
    }

    fn index_inner(&mut self, index: &Expr) -> Doc {
        if let ExprKind::Slice { start, stop, step } = &index.kind {
            self.slice_expr(start, stop, step)
        } else {
            self.expr(index)
        }
    }

    fn slice_expr(
        &mut self,
        start: &Option<Box<Expr>>,
        stop: &Option<Box<Expr>>,
        step: &Option<Box<Expr>>,
    ) -> Doc {
        let part = |this: &mut Self, p: &Option<Box<Expr>>| match p {
            Some(e) => this.expr(e),
            None => nil(),
        };
        let s = part(self, start);
        let e = part(self, stop);
        let mut d = concat_all([s, text(":"), e]);
        if let Some(st) = step {
            d = concat_all([d, text(":"), self.expr(st)]);
        }
        d
    }

    fn comp_clauses(&mut self, clauses: &[CompClause]) -> Doc {
        let mut d = Doc::Nil;
        for c in clauses {
            d = concat_all([
                d,
                text(" for "),
                self.for_target(&c.target),
                text(" in "),
                self.child(&c.iter, binop_prec(&BinOp::Or), false),
            ]);
            if let Some(cond) = &c.condition {
                d = concat_all([d, text(" if "), self.expr(cond)]);
            }
        }
        d
    }

    fn pattern(&mut self, p: &MatchPattern) -> Doc {
        match p {
            MatchPattern::Wildcard => text("_"),
            MatchPattern::Identifier(name, _) => text(name.clone()),
            MatchPattern::Literal(e) => self.expr(e),
            MatchPattern::Variant(name, sub) => {
                if sub.is_empty() {
                    text(name.clone())
                } else {
                    let docs: Vec<Doc> = sub.iter().map(|s| self.pattern(s)).collect();
                    concat(text(name.clone()), bracketed("(", docs, ")"))
                }
            }
        }
    }
}
