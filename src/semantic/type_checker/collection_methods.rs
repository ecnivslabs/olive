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
            // Arity only; return types stay with the main match below so
            // valid calls behave exactly as before. Without these, a wrong
            // count reaches codegen, whose fixed signatures abort the
            // compiler on the mismatch instead of reporting E0403.
            "append" | "extend" | "remove" => {
                if arg_count != 1 {
                    arity_error(self, attr, span, 1, 1, arg_count);
                }
                None
            }
            "insert" => {
                if arg_count != 2 {
                    arity_error(self, attr, span, 2, 2, arg_count);
                }
                None
            }
            "pop" => {
                if arg_count != 0 {
                    arity_error(self, attr, span, 0, 0, arg_count);
                }
                None
            }
            "reverse" => {
                if arg_count != 0 {
                    arity_error(self, attr, span, 0, 0, arg_count);
                }
                None
            }
            _ => None,
        }
    }

    /// `sort` takes no positional arguments, only an optional `key=`
    /// function: anything else reaches codegen, whose fixed sort entry
    /// points abort the compiler on the mismatch instead of reporting
    /// E0403. Called from the main method match, which sees the raw
    /// `CallArg` shapes this file does not receive.
    pub(super) fn check_sort_args(&mut self, args: &[crate::parser::CallArg], span: Span) {
        let mut ok = true;
        for a in args {
            match a {
                crate::parser::CallArg::Keyword(name, _) if name == "key" => {}
                _ => {
                    ok = false;
                }
            }
        }
        if !ok {
            arity_error(self, "sort", span, 0, 1, args.len());
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
            "remove" => {
                if arg_count != 1 {
                    arity_error(self, attr, span, 1, 1, arg_count);
                }
                None
            }
            "keys" | "values" | "items" => {
                if arg_count != 0 {
                    arity_error(self, attr, span, 0, 0, arg_count);
                }
                None
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
            "add" | "remove" | "contains" => {
                if arg_count != 1 {
                    arity_error(self, attr, span, 1, 1, arg_count);
                }
                None
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
