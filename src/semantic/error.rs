use crate::compile::errors::Diagnostic;
use crate::span::Span;

#[derive(Debug, Clone)]
pub enum SemanticError {
    UndefinedName {
        name: String,
        span: Span,
        suggestion: Option<String>,
    },
    DuplicateParam {
        name: String,
        span: Span,
        first: Span,
    },
    AssignToUndefined {
        name: String,
        span: Span,
        suggestion: Option<String>,
    },
    PrivateAccess {
        name: String,
        span: Span,
    },
    /// Any check whose diagnostic is built directly at the call site, carrying
    /// its own stable code, labels, notes, and fixes. This is the path for type
    /// errors, exhaustiveness, mutability, FFI safety, and borrow violations.
    Rich(Box<Diagnostic>),
}

impl SemanticError {
    pub fn span(&self) -> Span {
        match self {
            SemanticError::UndefinedName { span, .. } => *span,
            SemanticError::DuplicateParam { span, .. } => *span,
            SemanticError::AssignToUndefined { span, .. } => *span,
            SemanticError::PrivateAccess { span, .. } => *span,
            SemanticError::Rich(d) => d.primary_span(),
        }
    }

    /// Wrap a fully built diagnostic.
    pub fn rich(diag: Diagnostic) -> Self {
        SemanticError::Rich(Box::new(diag))
    }

    /// Two types required to be equal are not. Uses the canonical
    /// `expected`/`found` framing, anchored under the caret, with the concrete
    /// types restated as aligned notes for quick scanning.
    pub fn type_mismatch(
        span: Span,
        expected: impl Into<String>,
        found: impl Into<String>,
    ) -> Self {
        let (expected, found) = (expected.into(), found.into());
        SemanticError::rich(
            Diagnostic::error("E0400", "mismatched types", span)
                .label(format!("expected `{expected}`, found `{found}`"))
                .note(format!("expected type `{expected}`"))
                .note(format!("   found type `{found}`")),
        )
    }

    /// A type mismatch where the source value is an untyped numeric literal.
    /// Adds a concrete cast suggestion that resolves it.
    pub fn literal_mismatch(span: Span, expected: impl Into<String>, literal: &str) -> Self {
        let expected = expected.into();
        SemanticError::rich(
            Diagnostic::error("E0400", "mismatched types", span)
                .label(format!("expected `{expected}`, found {literal}"))
                .note(format!(
                    "the {literal} has no fixed type until it is constrained"
                ))
                .help(format!(
                    "annotate or cast the value so it is a `{expected}`, e.g. `value as {expected}`"
                )),
        )
    }

    /// Lower this error into a rich, renderable diagnostic with a stable code,
    /// a span label, explanatory notes, and an actionable fix where possible.
    pub fn to_diagnostic(&self) -> Diagnostic {
        match self {
            SemanticError::UndefinedName {
                name,
                span,
                suggestion,
            } => {
                let d = Diagnostic::error(
                    "E0001",
                    format!("cannot find name `{name}` in this scope"),
                    *span,
                )
                .label("not found in this scope");
                attach_name_fix(d, name, *span, suggestion)
            }
            SemanticError::DuplicateParam { name, span, first } => Diagnostic::error(
                "E0002",
                format!("parameter `{name}` is declared twice"),
                *span,
            )
            .label("redeclared here")
            .secondary(*first, "first declared here")
            .help("rename one of the parameters so each name is unique"),
            SemanticError::AssignToUndefined {
                name,
                span,
                suggestion,
            } => {
                let d = Diagnostic::error(
                    "E0003",
                    format!("cannot assign to `{name}` because it is not defined"),
                    *span,
                )
                .label("not defined in this scope")
                .note("Olive bindings must be introduced with `let` before assignment")
                .help(format!("introduce it with `let {name} = ...`"));
                attach_name_fix(d, name, *span, suggestion)
            }
            SemanticError::PrivateAccess { name, span } => {
                Diagnostic::error("E0004", format!("`{name}` is private to its module"), *span)
                    .label("private name used here")
                    .note("names prefixed with `_` are visible only inside their defining module")
                    .help(format!(
                        "remove the leading underscore or export `{name}` from its module"
                    ))
            }
            SemanticError::Rich(d) => (**d).clone(),
        }
    }
}

/// Attaches the `did you mean` help for a misspelled name. When the nearest
/// match is a plain identifier (not a `module::member` path, whose source form
/// uses `.` rather than `::`), the suggestion is promoted to a machine-applicable
/// fix that rewrites the exact identifier span, so `pit fix` can apply it.
fn attach_name_fix(
    d: Diagnostic,
    name: &str,
    span: Span,
    suggestion: &Option<String>,
) -> Diagnostic {
    match suggestion {
        Some(s) if !name.contains("::") && !s.contains("::") => {
            d.fix(span, s.clone(), "did you mean")
        }
        _ => d.suggest(suggestion),
    }
}

impl std::fmt::Display for SemanticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SemanticError::UndefinedName { name, span, .. } => {
                write!(f, "{}:{}: undefined name `{}`", span.line, span.col, name)
            }
            SemanticError::DuplicateParam { name, span, .. } => write!(
                f,
                "{}:{}: duplicate parameter `{}`",
                span.line, span.col, name
            ),
            SemanticError::AssignToUndefined { name, span, .. } => write!(
                f,
                "{}:{}: assignment to undefined variable `{}` (use `let`)",
                span.line, span.col, name
            ),
            SemanticError::PrivateAccess { name, span } => write!(
                f,
                "{}:{}: cannot access private name `{}` from outside its module",
                span.line, span.col, name
            ),
            SemanticError::Rich(d) => {
                let s = d.primary_span();
                write!(f, "{}:{}: {}", s.line, s.col, d.headline())
            }
        }
    }
}

impl std::error::Error for SemanticError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn span() -> Span {
        Span {
            file_id: 0,
            line: 2,
            col: 8,
            start: 12,
            end: 20,
        }
    }

    #[test]
    fn display_undefined_name() {
        let e = SemanticError::UndefinedName {
            name: "x".into(),
            span: span(),
            suggestion: None,
        };
        assert_eq!(e.to_string(), "2:8: undefined name `x`");
    }

    #[test]
    fn display_duplicate_param() {
        let e = SemanticError::DuplicateParam {
            name: "a".into(),
            span: span(),
            first: span(),
        };
        assert_eq!(e.to_string(), "2:8: duplicate parameter `a`");
    }

    #[test]
    fn display_assign_to_undefined() {
        let e = SemanticError::AssignToUndefined {
            name: "y".into(),
            span: span(),
            suggestion: None,
        };
        assert_eq!(
            e.to_string(),
            "2:8: assignment to undefined variable `y` (use `let`)"
        );
    }

    #[test]
    fn diagnostic_undefined_includes_suggestion() {
        let e = SemanticError::UndefinedName {
            name: "totl".into(),
            span: span(),
            suggestion: Some("total".into()),
        };
        let mut sources = crate::compile::errors::Sources::default();
        sources.insert(0, ("m.liv".into(), "print(totl)\n".into()));
        e.to_diagnostic().emit(&sources);
    }

    #[test]
    fn display_private_access() {
        let e = SemanticError::PrivateAccess {
            name: "secret".into(),
            span: span(),
        };
        assert_eq!(
            e.to_string(),
            "2:8: cannot access private name `secret` from outside its module"
        );
    }

    #[test]
    fn display_rich() {
        let e = SemanticError::rich(Diagnostic::error("E0400", "type mismatch", span()));
        assert_eq!(e.to_string(), "2:8: type mismatch");
    }

    #[test]
    fn type_mismatch_carries_code_and_notes() {
        let d = SemanticError::type_mismatch(span(), "i64", "str").to_diagnostic();
        assert_eq!(d.headline(), "mismatched types");
        let mut sources = crate::compile::errors::Sources::default();
        sources.insert(0, ("m.liv".into(), "let x: i64 = \"s\"\n".into()));
        d.emit(&sources);
    }

    #[test]
    fn literal_mismatch_suggests_cast() {
        let e = SemanticError::literal_mismatch(span(), "str", "integer literal");
        assert_eq!(e.to_string(), "2:8: mismatched types");
    }

    #[test]
    fn span_undefined_name() {
        let e = SemanticError::UndefinedName {
            name: "x".into(),
            span: span(),
            suggestion: None,
        };
        assert_eq!(e.span().line, 2);
        assert_eq!(e.span().col, 8);
    }

    #[test]
    fn span_duplicate_param() {
        let e = SemanticError::DuplicateParam {
            name: "a".into(),
            span: span(),
            first: span(),
        };
        assert_eq!(e.span().start, 12);
    }

    #[test]
    fn span_assign_to_undefined() {
        let e = SemanticError::AssignToUndefined {
            name: "y".into(),
            span: span(),
            suggestion: None,
        };
        assert_eq!(e.span().end, 20);
    }

    #[test]
    fn span_private_access() {
        let e = SemanticError::PrivateAccess {
            name: "secret".into(),
            span: span(),
        };
        assert_eq!(e.span().file_id, 0);
    }

    #[test]
    fn span_rich() {
        let e = SemanticError::rich(Diagnostic::error("E0400", "err", span()));
        assert_eq!(e.span().line, 2);
        assert_eq!(e.span().col, 8);
    }
}
