//! Innermost AST node covering a source position; shared by hover and go-to-definition.

use crate::parser::ast::{
    CallArg, CompClause, EnumVariant, Expr, ForTarget, MatchCase, MatchPattern, Param, Program,
    Stmt, StmtKind, WithItem,
};
use crate::span::Span;

fn contains(span: Span, file_id: usize, offset: usize) -> bool {
    span.file_id == file_id && span.start <= offset && offset <= span.end
}

fn width(span: Span) -> usize {
    span.end.saturating_sub(span.start)
}

fn consider<'a>(e: &'a Expr, file_id: usize, offset: usize, best: &mut Option<&'a Expr>) {
    if !contains(e.span, file_id, offset) {
        return;
    }
    let better = match best {
        Some(cur) => width(e.span) <= width(cur.span),
        None => true,
    };
    if better {
        *best = Some(e);
    }
}

/// Smallest expression whose span covers `offset` (char offset), or `None`.
pub fn find_expr_at(program: &Program, file_id: usize, offset: usize) -> Option<&Expr> {
    let mut best = None;
    for stmt in &program.stmts {
        walk_stmt(stmt, file_id, offset, &mut best);
    }
    best
}

fn walk_block<'a>(stmts: &'a [Stmt], file_id: usize, offset: usize, best: &mut Option<&'a Expr>) {
    for s in stmts {
        walk_stmt(s, file_id, offset, best);
    }
}

fn walk_stmt<'a>(stmt: &'a Stmt, file_id: usize, offset: usize, best: &mut Option<&'a Expr>) {
    match &stmt.kind {
        StmtKind::Fn { params, body, .. } => {
            walk_params(params, file_id, offset, best);
            walk_block(body, file_id, offset, best);
        }
        StmtKind::Struct { fields, body, .. } => {
            walk_params(fields, file_id, offset, best);
            walk_block(body, file_id, offset, best);
        }
        StmtKind::Impl { body, .. } => walk_block(body, file_id, offset, best),
        StmtKind::Trait { methods, .. } => walk_block(methods, file_id, offset, best),
        StmtKind::Enum { variants, body, .. } => {
            for v in variants {
                walk_enum_variant(v, file_id, offset, best);
            }
            walk_block(body, file_id, offset, best);
        }
        StmtKind::If {
            condition,
            then_body,
            elif_clauses,
            else_body,
        } => {
            walk_expr(condition, file_id, offset, best);
            walk_block(then_body, file_id, offset, best);
            for (cond, body) in elif_clauses {
                walk_expr(cond, file_id, offset, best);
                walk_block(body, file_id, offset, best);
            }
            if let Some(b) = else_body {
                walk_block(b, file_id, offset, best);
            }
        }
        StmtKind::While {
            condition,
            body,
            else_body,
        } => {
            walk_expr(condition, file_id, offset, best);
            walk_block(body, file_id, offset, best);
            if let Some(b) = else_body {
                walk_block(b, file_id, offset, best);
            }
        }
        StmtKind::For {
            iter,
            body,
            else_body,
            ..
        } => {
            walk_expr(iter, file_id, offset, best);
            walk_block(body, file_id, offset, best);
            if let Some(b) = else_body {
                walk_block(b, file_id, offset, best);
            }
        }
        StmtKind::Return(e) => {
            if let Some(e) = e {
                walk_expr(e, file_id, offset, best);
            }
        }
        StmtKind::Assert { test, msg } => {
            walk_expr(test, file_id, offset, best);
            if let Some(m) = msg {
                walk_expr(m, file_id, offset, best);
            }
        }
        StmtKind::With { items, body } => {
            for WithItem {
                context_expr,
                alias,
            } in items
            {
                walk_expr(context_expr, file_id, offset, best);
                if let Some(a) = alias {
                    walk_expr(a, file_id, offset, best);
                }
            }
            walk_block(body, file_id, offset, best);
        }
        StmtKind::Let { value, .. }
        | StmtKind::MultiLet { value, .. }
        | StmtKind::Const { value, .. }
        | StmtKind::MultiConst { value, .. } => walk_expr(value, file_id, offset, best),
        StmtKind::Assign { target, value } => {
            walk_expr(target, file_id, offset, best);
            walk_expr(value, file_id, offset, best);
        }
        StmtKind::AugAssign { target, value, .. } => {
            walk_expr(target, file_id, offset, best);
            walk_expr(value, file_id, offset, best);
        }
        StmtKind::ExprStmt(e) => walk_expr(e, file_id, offset, best),
        StmtKind::UnsafeBlock(body) => walk_block(body, file_id, offset, best),
        StmtKind::Defer(e) => walk_expr(e, file_id, offset, best),
        StmtKind::Import { .. }
        | StmtKind::NativeImport { .. }
        | StmtKind::FromImport { .. }
        | StmtKind::PyImport { .. }
        | StmtKind::Pass
        | StmtKind::Break
        | StmtKind::Continue
        | StmtKind::TypeAlias { .. } => {}
    }
}

fn walk_params<'a>(
    params: &'a [Param],
    file_id: usize,
    offset: usize,
    best: &mut Option<&'a Expr>,
) {
    for p in params {
        if let Some(d) = &p.default {
            walk_expr(d, file_id, offset, best);
        }
    }
}

fn walk_enum_variant<'a>(
    v: &'a EnumVariant,
    file_id: usize,
    offset: usize,
    best: &mut Option<&'a Expr>,
) {
    if let Some(val) = &v.value {
        walk_expr(val, file_id, offset, best);
    }
}

fn walk_comp_clauses<'a>(
    clauses: &'a [CompClause],
    file_id: usize,
    offset: usize,
    best: &mut Option<&'a Expr>,
) {
    for c in clauses {
        walk_expr(&c.iter, file_id, offset, best);
        if let Some(cond) = &c.condition {
            walk_expr(cond, file_id, offset, best);
        }
        if let ForTarget::Tuple(_) = &c.target {
            // Loop-var spans belong to identifiers, not sub-expressions here.
        }
    }
}

fn walk_call_args<'a>(
    args: &'a [CallArg],
    file_id: usize,
    offset: usize,
    best: &mut Option<&'a Expr>,
) {
    for a in args {
        match a {
            CallArg::Positional(e)
            | CallArg::Keyword(_, e)
            | CallArg::Splat(e)
            | CallArg::KwSplat(e) => walk_expr(e, file_id, offset, best),
        }
    }
}

fn walk_match_case<'a>(
    case: &'a MatchCase,
    file_id: usize,
    offset: usize,
    best: &mut Option<&'a Expr>,
) {
    walk_pattern(&case.pattern, file_id, offset, best);
    if let Some(g) = &case.guard {
        walk_expr(g, file_id, offset, best);
    }
    walk_block(&case.body, file_id, offset, best);
}

fn walk_pattern<'a>(
    pattern: &'a MatchPattern,
    file_id: usize,
    offset: usize,
    best: &mut Option<&'a Expr>,
) {
    match pattern {
        MatchPattern::Variant(_, sub) => {
            for p in sub {
                walk_pattern(p, file_id, offset, best);
            }
        }
        MatchPattern::Literal(e) => walk_expr(e, file_id, offset, best),
        MatchPattern::Identifier(..) | MatchPattern::Wildcard => {}
    }
}

fn walk_expr<'a>(expr: &'a Expr, file_id: usize, offset: usize, best: &mut Option<&'a Expr>) {
    consider(expr, file_id, offset, best);
    use crate::parser::ast::ExprKind::*;
    match &expr.kind {
        Integer(_) | Float(_) | Str(_) | Bool(_) | Null | Identifier(_) => {}
        FStr(parts) => {
            for p in parts {
                walk_expr(&p.expr, file_id, offset, best);
            }
        }
        BinOp { left, right, .. } => {
            walk_expr(left, file_id, offset, best);
            walk_expr(right, file_id, offset, best);
        }
        UnaryOp { operand, .. } => walk_expr(operand, file_id, offset, best),
        Cast(e, _) => walk_expr(e, file_id, offset, best),
        Call { callee, args } => {
            walk_expr(callee, file_id, offset, best);
            walk_call_args(args, file_id, offset, best);
        }
        Index { obj, index } => {
            walk_expr(obj, file_id, offset, best);
            walk_expr(index, file_id, offset, best);
        }
        Attr { obj, .. } | OptAttr { obj, .. } => walk_expr(obj, file_id, offset, best),
        List(elems) | Tuple(elems) | Set(elems) => {
            for e in elems {
                walk_expr(e, file_id, offset, best);
            }
        }
        Dict(pairs) => {
            for (k, v) in pairs {
                walk_expr(k, file_id, offset, best);
                walk_expr(v, file_id, offset, best);
            }
        }
        Starred(e) => walk_expr(e, file_id, offset, best),
        ListComp { elt, clauses } | SetComp { elt, clauses } => {
            walk_expr(elt, file_id, offset, best);
            walk_comp_clauses(clauses, file_id, offset, best);
        }
        DictComp {
            key,
            value,
            clauses,
        } => {
            walk_expr(key, file_id, offset, best);
            walk_expr(value, file_id, offset, best);
            walk_comp_clauses(clauses, file_id, offset, best);
        }
        Borrow(e) | MutBorrow(e) | Deref(e) | Try(e) | Await(e) => {
            walk_expr(e, file_id, offset, best)
        }
        AsyncBlock(body) => walk_block(body, file_id, offset, best),
        Slice { start, stop, step } => {
            for e in [start, stop, step].into_iter().flatten() {
                walk_expr(e, file_id, offset, best);
            }
        }
        Range {
            start, end, step, ..
        } => {
            walk_expr(start, file_id, offset, best);
            walk_expr(end, file_id, offset, best);
            if let Some(s) = step {
                walk_expr(s, file_id, offset, best);
            }
        }
        Match { expr: e, cases } => {
            walk_expr(e, file_id, offset, best);
            for c in cases {
                walk_match_case(c, file_id, offset, best);
            }
        }
        Ternary {
            cond,
            then,
            otherwise,
        } => {
            walk_expr(cond, file_id, offset, best);
            walk_expr(then, file_id, offset, best);
            walk_expr(otherwise, file_id, offset, best);
        }
        Lambda { params, body } => {
            walk_params(params, file_id, offset, best);
            walk_expr(body, file_id, offset, best);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn parse(src: &str) -> Program {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        Parser::new(tokens).parse_program().unwrap()
    }

    #[test]
    fn finds_identifier_in_simple_expr() {
        let program = parse("let x = 1\nprint(x)\n");
        // "print(x)" -- offset of `x` inside the call.
        let src = "let x = 1\nprint(x)\n";
        let offset = src.chars().position(|c| c == 'x').unwrap() + 1; // second `x`, the call arg
        let offset = src.chars().skip(offset).position(|c| c == 'x').unwrap() + offset;
        let found = find_expr_at(&program, 0, offset).expect("expr found");
        assert!(matches!(found.kind, crate::parser::ast::ExprKind::Identifier(ref n) if n == "x"));
    }

    #[test]
    fn finds_innermost_of_nested_binop() {
        let program = parse("let x = 1 + 2 * 3\n");
        let src = "let x = 1 + 2 * 3\n";
        let offset = src.find('2').unwrap();
        let found = find_expr_at(&program, 0, offset).expect("expr found");
        assert!(matches!(
            found.kind,
            crate::parser::ast::ExprKind::Integer(2)
        ));
    }

    #[test]
    fn out_of_range_offset_finds_nothing() {
        let program = parse("let x = 1\n");
        assert!(find_expr_at(&program, 0, 9999).is_none());
    }
}
