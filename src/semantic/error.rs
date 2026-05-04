use crate::span::Span;

#[derive(Debug, Clone)]
pub enum SemanticError {
    UndefinedName { name: String, span: Span },
    DuplicateParam { name: String, span: Span },
    AssignToUndefined { name: String, span: Span },
    PrivateAccess { name: String, span: Span },
    Custom { msg: String, span: Span },
}

impl SemanticError {
    pub fn span(&self) -> Span {
        match self {
            SemanticError::UndefinedName { span, .. } => *span,
            SemanticError::DuplicateParam { span, .. } => *span,
            SemanticError::AssignToUndefined { span, .. } => *span,
            SemanticError::PrivateAccess { span, .. } => *span,
            SemanticError::Custom { span, .. } => *span,
        }
    }
}

impl std::fmt::Display for SemanticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SemanticError::UndefinedName { name, span } => {
                write!(f, "{}:{}: undefined name `{}`", span.line, span.col, name)
            }
            SemanticError::DuplicateParam { name, span } => write!(
                f,
                "{}:{}: duplicate parameter `{}`",
                span.line, span.col, name
            ),
            SemanticError::AssignToUndefined { name, span } => write!(
                f,
                "{}:{}: assignment to undefined variable `{}` (use `let`)",
                span.line, span.col, name
            ),
            SemanticError::PrivateAccess { name, span } => write!(
                f,
                "{}:{}: cannot access private name `{}` from outside its module",
                span.line, span.col, name
            ),
            SemanticError::Custom { msg, span } => write!(f, "{}:{}: {}", span.line, span.col, msg),
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
        };
        assert_eq!(e.to_string(), "2:8: undefined name `x`");
    }

    #[test]
    fn display_duplicate_param() {
        let e = SemanticError::DuplicateParam {
            name: "a".into(),
            span: span(),
        };
        assert_eq!(e.to_string(), "2:8: duplicate parameter `a`");
    }

    #[test]
    fn display_assign_to_undefined() {
        let e = SemanticError::AssignToUndefined {
            name: "y".into(),
            span: span(),
        };
        assert_eq!(
            e.to_string(),
            "2:8: assignment to undefined variable `y` (use `let`)"
        );
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
    fn display_custom() {
        let e = SemanticError::Custom {
            msg: "type mismatch".into(),
            span: span(),
        };
        assert_eq!(e.to_string(), "2:8: type mismatch");
    }

    #[test]
    fn span_undefined_name() {
        let e = SemanticError::UndefinedName {
            name: "x".into(),
            span: span(),
        };
        assert_eq!(e.span().line, 2);
        assert_eq!(e.span().col, 8);
    }

    #[test]
    fn span_duplicate_param() {
        let e = SemanticError::DuplicateParam {
            name: "a".into(),
            span: span(),
        };
        assert_eq!(e.span().start, 12);
    }

    #[test]
    fn span_assign_to_undefined() {
        let e = SemanticError::AssignToUndefined {
            name: "y".into(),
            span: span(),
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
    fn span_custom() {
        let e = SemanticError::Custom {
            msg: "err".into(),
            span: span(),
        };
        assert_eq!(e.span().line, 2);
        assert_eq!(e.span().col, 8);
    }
}
