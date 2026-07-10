use crate::parser::ast::*;

/// Surface spelling of a binary operator.
pub(super) fn binop_str(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Pow => "**",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::LtEq => "<=",
        BinOp::Gt => ">",
        BinOp::GtEq => ">=",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::Coalesce => "??",
        BinOp::In => "in",
        BinOp::NotIn => "not in",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::BitOr => "|",
        BinOp::BitAnd => "&",
        BinOp::BitXor => "^",
    }
}

/// Binding power of a binary operator, mirroring the precedence ladder in
/// `parser/expr/ops.rs`. Higher binds tighter. Used to insert the minimal set of
/// parentheses needed to keep the same parse.
pub(super) fn binop_prec(op: &BinOp) -> u8 {
    match op {
        BinOp::Or | BinOp::Coalesce => 1,
        BinOp::And => 2,
        BinOp::Eq
        | BinOp::NotEq
        | BinOp::Lt
        | BinOp::LtEq
        | BinOp::Gt
        | BinOp::GtEq
        | BinOp::In
        | BinOp::NotIn => 4,
        BinOp::BitOr => 5,
        BinOp::BitXor => 6,
        BinOp::BitAnd => 7,
        BinOp::Shl | BinOp::Shr => 8,
        BinOp::Add | BinOp::Sub => 9,
        BinOp::Mul | BinOp::Div | BinOp::Mod => 10,
        BinOp::Pow => 12,
    }
}

/// Surface spelling of an augmented-assignment operator.
pub(super) fn augop_str(op: &AugOp) -> &'static str {
    match op {
        AugOp::Add => "+=",
        AugOp::Sub => "-=",
        AugOp::Mul => "*=",
        AugOp::Div => "/=",
        AugOp::Mod => "%=",
        AugOp::Pow => "**=",
        AugOp::Shl => "<<=",
        AugOp::Shr => ">>=",
        AugOp::BitOr => "|=",
        AugOp::BitAnd => "&=",
        AugOp::BitXor => "^=",
    }
}

/// Binding power of an expression node, for parenthesization decisions. Prefix
/// unary and postfix groups sit above all binary operators; atoms are highest.
pub(super) fn expr_prec(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::BinOp { op, .. } => binop_prec(op),
        ExprKind::UnaryOp {
            op: UnaryOp::Not, ..
        } => 3,
        ExprKind::UnaryOp { .. }
        | ExprKind::Borrow(_)
        | ExprKind::MutBorrow(_)
        | ExprKind::Deref(_)
        | ExprKind::Try(_)
        | ExprKind::Await(_) => 11,
        ExprKind::Cast(..)
        | ExprKind::Call { .. }
        | ExprKind::Index { .. }
        | ExprKind::Attr { .. } => 13,
        _ => 14,
    }
}

/// Top-level declarations that should be separated by a blank line.
pub(super) fn is_decl(k: &StmtKind) -> bool {
    matches!(
        k,
        StmtKind::Fn { .. }
            | StmtKind::Struct { .. }
            | StmtKind::Enum { .. }
            | StmtKind::Impl { .. }
            | StmtKind::Trait { .. }
    )
}
