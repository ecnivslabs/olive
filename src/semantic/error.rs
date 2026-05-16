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
