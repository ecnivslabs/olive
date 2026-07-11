//! Checker arms for the E3.6 list/dict/set method surface (`count`, `index`,
//! `clear` on lists; `update`, `pop`, `setdefault`, `clear` on dicts;
//! `discard`, `clear` on sets). Kept out of `expr.rs`, which is already
//! oversized, so this method surface lands here.

use super::TypeChecker;
use crate::semantic::types::Type;
use crate::span::Span;

fn arity_error(
    checker: &mut TypeChecker,
    name: &str,
    span: Span,
    min: usize,
    max: usize,
    got: usize,
) {
    let expected = if min == max {
        format!("{min}")
    } else {
        format!("{min} to {max}")
    };
    checker
        .errors
        .push(crate::semantic::error::SemanticError::rich(
            crate::compile::errors::Diagnostic::error(
                "E0403",
                format!("wrong number of arguments to `{name}`"),
                span,
            )
            .label(format!("expected {expected} argument(s), found {got}")),
        ));
}

impl TypeChecker {
    /// `[T]` methods: `count(x) -> int`, `index(x) -> int` (faults when `x`
    /// is absent), `clear() -> [T]`.
    pub(super) fn check_list_method_ext(
        &mut self,
        elem: &Type,
        attr: &str,
        arg_count: usize,
        span: Span,
        base: &Type,
    ) -> Option<Type> {
        match attr {
            "count" | "index" => {
                if arg_count != 1 {
                    arity_error(self, attr, span, 1, 1, arg_count);
                }
                let _ = elem;
                Some(Type::Int)
            }
            "clear" => {
                if arg_count != 0 {
                    arity_error(self, attr, span, 0, 0, arg_count);
                }
                Some(base.clone())
            }
            _ => None,
        }
    }

    /// `{K: V}` methods: `update(other) -> {K: V}`, `pop(k) -> V` (faults),
    /// `pop(k, default) -> V`, `setdefault(k, v) -> V`, `clear() -> {K: V}`.
    pub(super) fn check_dict_method_ext(
        &mut self,
        val_ty: &Type,
        attr: &str,
        arg_count: usize,
        span: Span,
        base: &Type,
    ) -> Option<Type> {
        match attr {
            "update" => {
                if arg_count != 1 {
                    arity_error(self, attr, span, 1, 1, arg_count);
                }
                Some(base.clone())
            }
            "pop" => {
                if arg_count != 1 && arg_count != 2 {
                    arity_error(self, attr, span, 1, 2, arg_count);
                }
                Some(val_ty.clone())
            }
            "setdefault" => {
                if arg_count != 2 {
                    arity_error(self, attr, span, 2, 2, arg_count);
                }
                Some(val_ty.clone())
            }
            "clear" => {
                if arg_count != 0 {
                    arity_error(self, attr, span, 0, 0, arg_count);
                }
                Some(base.clone())
            }
            _ => None,
        }
    }

    /// `{T}` methods added in E3.6: `discard(x) -> T` (never faults, unlike
    /// `remove` which now does), `clear() -> {T}`.
    pub(super) fn check_set_method_ext(
        &mut self,
        elem: &Type,
        attr: &str,
        arg_count: usize,
        span: Span,
        base: &Type,
    ) -> Option<Type> {
        match attr {
            "discard" => {
                if arg_count != 1 {
                    arity_error(self, attr, span, 1, 1, arg_count);
                }
                Some(elem.clone())
            }
            "clear" => {
                if arg_count != 0 {
                    arity_error(self, attr, span, 0, 0, arg_count);
                }
                Some(base.clone())
            }
            _ => None,
        }
    }
}
