//! Checker arms for the E3.5 string method surface (`count`, `rfind`,
//! `splitlines`, `title`, `capitalize`, `zfill`, `ljust`/`rjust`/`center`,
//! `partition`, `removeprefix`/`removesuffix`, the `isX` family, and the
//! optional-argument forms of `strip`/`lstrip`/`rstrip`/`split`). Kept out of
//! `expr.rs`, which is already oversized, so this method surface lands here.

use super::TypeChecker;
use crate::semantic::types::Type;
use crate::span::Span;

impl TypeChecker {
    /// Validates `args` against `[min, max]` positional arguments of the
    /// given expected types (checked where an expected type is provided;
    /// extra optional args beyond `param_tys.len()` go unchecked), then
    /// returns `ret`. Emits E0403 for arity, E0404 per mistyped argument.
    #[allow(clippy::too_many_arguments)]
    fn check_arity_and_return(
        &mut self,
        name: &str,
        arg_tys: &[Type],
        span: Span,
        min: usize,
        max: usize,
        param_tys: &[Type],
        ret: Type,
    ) -> Option<Type> {
        if arg_tys.len() < min || arg_tys.len() > max {
            let expected = if min == max {
                format!("{min}")
            } else {
                format!("{min} to {max}")
            };
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0403",
                        format!("wrong number of arguments to `{name}`"),
                        span,
                    )
                    .label(format!(
                        "expected {expected} argument(s), found {}",
                        arg_tys.len()
                    )),
                ));
            return Some(ret);
        }
        for (i, expected) in param_tys.iter().enumerate() {
            let Some(got) = arg_tys.get(i) else { break };
            let got = self.apply_subst(got.clone());
            let compatible = got == *expected
                || got == Type::Any
                || (*expected == Type::Int && matches!(got, Type::IntegerLiteral(_)));
            if !compatible {
                self.errors
                    .push(crate::semantic::error::SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0404",
                            format!(
                                "argument {} of `{name}` must be `{expected}`, got `{got}`",
                                i + 1
                            ),
                            span,
                        )
                        .label(format!("expected `{expected}`")),
                    ));
            }
        }
        Some(ret)
    }

    /// Entry point for the E3.5 string method surface. `None` when `attr`
    /// isn't one of these methods, so the pre-E3.5 method match in
    /// `builtin_collection_method` still handles everything else.
    pub(super) fn check_string_method_ext(
        &mut self,
        attr: &str,
        arg_tys: &[Type],
        span: Span,
    ) -> Option<Type> {
        let str_list = Type::List(Box::new(Type::Str));
        let str3 = Type::Tuple(vec![Type::Str, Type::Str, Type::Str]);
        match attr {
            "count" | "rfind" => {
                self.check_arity_and_return(attr, arg_tys, span, 1, 1, &[Type::Str], Type::Int)
            }
            "splitlines" => self.check_arity_and_return(attr, arg_tys, span, 0, 0, &[], str_list),
            "title" | "capitalize" => {
                self.check_arity_and_return(attr, arg_tys, span, 0, 0, &[], Type::Str)
            }
            "zfill" => {
                self.check_arity_and_return(attr, arg_tys, span, 1, 1, &[Type::Int], Type::Str)
            }
            "ljust" | "rjust" | "center" => self.check_arity_and_return(
                attr,
                arg_tys,
                span,
                1,
                2,
                &[Type::Int, Type::Str],
                Type::Str,
            ),
            "partition" => {
                self.check_arity_and_return(attr, arg_tys, span, 1, 1, &[Type::Str], str3)
            }
            "removeprefix" | "removesuffix" => {
                self.check_arity_and_return(attr, arg_tys, span, 1, 1, &[Type::Str], Type::Str)
            }
            "isdigit" | "isalpha" | "isspace" | "isupper" | "islower" => {
                self.check_arity_and_return(attr, arg_tys, span, 0, 0, &[], Type::Bool)
            }
            "strip" | "lstrip" | "rstrip" => {
                self.check_arity_and_return(attr, arg_tys, span, 0, 1, &[Type::Str], Type::Str)
            }
            "split" => {
                self.check_arity_and_return(attr, arg_tys, span, 0, 1, &[Type::Str], str_list)
            }
            "to_int" => self.check_arity_and_return(
                attr,
                arg_tys,
                span,
                0,
                0,
                &[],
                Type::Union(vec![Type::Int, Type::Null]),
            ),
            "to_float" => self.check_arity_and_return(
                attr,
                arg_tys,
                span,
                0,
                0,
                &[],
                Type::Union(vec![Type::Float, Type::Null]),
            ),
            _ => None,
        }
    }
}
