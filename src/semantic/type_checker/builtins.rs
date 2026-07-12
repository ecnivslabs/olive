//! Checker arms for bare-function sequence builtins added in E3.3 onward
//! (`sorted`, `reversed`, `any`, `all`). Kept out of `expr.rs`, which is
//! already oversized, so new builtins land here instead of growing it further.

use super::TypeChecker;
use crate::parser::{CallArg, Expr};
use crate::semantic::types::Type;
use crate::span::Span;

fn arg_expr(a: &CallArg) -> &Expr {
    match a {
        CallArg::Positional(e)
        | CallArg::Keyword(_, e)
        | CallArg::Splat(e)
        | CallArg::KwSplat(e) => e,
    }
}

impl TypeChecker {
    /// `sorted(xs) -> [T]` and `reversed(xs) -> [T]`: both accept any list and
    /// return the same element type, leaving the source list untouched.
    fn check_sorted_reversed(&mut self, name: &str, arg: &Expr, span: Span) -> Type {
        let raw = self.check_expr(arg);
        let arg_ty = self.apply_subst(raw);
        match &arg_ty {
            Type::List(_) => arg_ty,
            Type::Any => Type::List(Box::new(Type::Any)),
            _ => {
                self.errors
                    .push(crate::semantic::error::SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0404",
                            format!("`{name}` requires a list argument, got `{arg_ty}`"),
                            span,
                        )
                        .label("expected a list"),
                    ));
                arg_ty
            }
        }
    }

    /// `any(xs) -> bool` and `all(xs) -> bool`: over `[bool]` or an
    /// `[Any]`/`Any` list, whose elements are checked for truthiness at
    /// runtime the same way `if x:` on an `Any` condition is (E1 step 1).
    fn check_any_all(&mut self, name: &str, arg: &Expr, span: Span) -> Type {
        let raw = self.check_expr(arg);
        let arg_ty = self.apply_subst(raw);
        let ok = match &arg_ty {
            Type::List(elem) => matches!(elem.as_ref(), Type::Bool | Type::Any),
            Type::Any => true,
            _ => false,
        };
        if !ok {
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0404",
                        format!("`{name}` requires a list of bool, got `{arg_ty}`"),
                        span,
                    )
                    .label("expected [bool]"),
                ));
        }
        Type::Bool
    }

    /// `str * int` / `[T] * int` and their int-on-left mirrors (E3.4).
    /// `None` when `l`/`r` isn't a repeat-operator combination, so
    /// `check_binop`'s general numeric path still handles plain `*`.
    pub(super) fn check_repeat_mul(&self, l: &Type, r: &Type) -> Option<Type> {
        let is_int = |t: &Type| matches!(t, Type::Int | Type::IntegerLiteral(_));
        let is_seq = |t: &Type| matches!(t, Type::Str | Type::List(_));
        match (l, r) {
            (a, b) if is_seq(a) && is_int(b) => Some(a.clone()),
            (a, b) if is_int(a) && is_seq(b) => Some(b.clone()),
            _ => None,
        }
    }

    /// `x in a..b` / `x not in a..b` (E3.9): the left operand must be `int`
    /// (or `Any`). `right` is only inspected syntactically here, so this is a
    /// no-op unless it's literally a range expression.
    pub(super) fn check_in_range_operand(
        &mut self,
        left_ty: &Type,
        right: &crate::parser::Expr,
        span: Span,
    ) {
        if !matches!(right.kind, crate::parser::ExprKind::Range { .. }) {
            return;
        }
        let resolved = self.apply_subst(left_ty.clone());
        if !matches!(resolved, Type::Int | Type::IntegerLiteral(_) | Type::Any) {
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0404",
                        format!("`in` on an int range requires an int operand, got `{resolved}`"),
                        span,
                    )
                    .label("expected int"),
                ));
        }
    }

    /// Entry point from the `Call` arm: `None` when `name` isn't one of these
    /// builtins, arity is wrong, or a user binding shadows the name.
    pub(super) fn check_sequence_builtin_call(
        &mut self,
        name: &str,
        args: &[CallArg],
        span: Span,
    ) -> Option<Type> {
        if args.len() != 1 || self.lookup_type(name).is_some() {
            return None;
        }
        let arg = arg_expr(&args[0]);
        match name {
            "sorted" | "reversed" => Some(self.check_sorted_reversed(name, arg, span)),
            "any" | "all" => Some(self.check_any_all(name, arg, span)),
            _ => None,
        }
    }
}
