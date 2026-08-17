//! Checker arms for the E3.5 string method surface (`count`, `rfind`,
//! `splitlines`, `title`, `capitalize`, `zfill`, `ljust`/`rjust`/`center`,
//! `partition`, `removeprefix`/`removesuffix`, the `isX` family, and the
//! optional-argument forms of `strip`/`lstrip`/`rstrip`/`split`). Kept out of
//! `expr.rs`, which is already oversized, so this method surface lands here.

use super::TypeChecker;
use super::collection_methods::arity_error;
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
            // Pre-E3.5 surface without arity checks: a wrong count reached
            // codegen, whose fixed signatures abort the compiler instead of
            // reporting E0403. Return types match the main match below.
            "upper" | "lower" => {
                self.check_arity_and_return(attr, arg_tys, span, 0, 0, &[], Type::Str)
            }
            "find" => {
                self.check_arity_and_return(attr, arg_tys, span, 1, 1, &[Type::Str], Type::Int)
            }
            "replace" => self.check_arity_and_return(
                attr,
                arg_tys,
                span,
                2,
                2,
                &[Type::Str, Type::Str],
                Type::Str,
            ),
            "repeat" => {
                self.check_arity_and_return(attr, arg_tys, span, 1, 1, &[Type::Int], Type::Str)
            }
            "join" => {
                if arg_tys.len() != 1 {
                    arity_error(self, attr, span, 1, 1, arg_tys.len());
                    return Some(Type::Str);
                }
                // An empty literal has no element type yet, and a dynamic
                // list's elements are only known at runtime: neither can be
                // judged here. Concrete non-string elements misread as
                // string pointers at runtime (out-of-bounds reads).
                let got = self.apply_subst(arg_tys[0].clone());
                let ok = match &got {
                    Type::List(e) => {
                        matches!(&**e, Type::Str | Type::Var(_) | Type::Param(_) | Type::Any)
                    }
                    Type::Var(_) | Type::Param(_) | Type::Any => true,
                    _ => false,
                };
                if !ok {
                    self.errors
                        .push(crate::semantic::error::SemanticError::rich(
                            crate::compile::errors::Diagnostic::error(
                                "E0404",
                                format!("argument 1 of `join` must be `[str]`, got `{got}`"),
                                span,
                            )
                            .label("expected `[str]`"),
                        ));
                }
                Some(Type::Str)
            }
            "contains" | "startswith" | "endswith" => {
                self.check_arity_and_return(attr, arg_tys, span, 1, 1, &[Type::Str], Type::Bool)
            }
            _ => None,
        }
    }
}
