use crate::parser::ast::*;
use rustc_hash::FxHashMap;

/// Copies trait default methods into every `impl Trait for Type` that does not
/// override them. After this runs each impl carries a full method set, so type
/// checking and lowering treat an inherited method exactly like a written one.
pub fn desugar_trait_defaults(program: &mut Program) {
    let mut trait_methods: FxHashMap<String, Vec<Stmt>> = FxHashMap::default();
    for stmt in &program.stmts {
        if let StmtKind::Trait { name, methods, .. } = &stmt.kind {
            trait_methods.insert(name.clone(), methods.clone());
        }
    }
    if trait_methods.is_empty() {
        return;
    }

    for stmt in &mut program.stmts {
        let StmtKind::Impl {
            trait_name: Some(trait_expr),
            body,
            ..
        } = &mut stmt.kind
        else {
            continue;
        };
        let Some(defaults) = trait_methods.get(&type_expr_name(trait_expr)) else {
            continue;
        };

        let present: Vec<String> = body
            .iter()
            .filter_map(|s| match &s.kind {
                StmtKind::Fn { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();

        for method in defaults {
            let StmtKind::Fn { name, .. } = &method.kind else {
                continue;
            };
            if present.iter().any(|p| p == name) {
                continue;
            }
            let mut copy = method.clone();
            refresh_stmt(&mut copy);
            body.push(copy);
        }
    }
}

fn type_expr_name(t: &TypeExpr) -> String {
    match &t.kind {
        TypeExprKind::Name(n) => n.clone(),
        TypeExprKind::Generic(n, _) => n.clone(),
        TypeExprKind::Qualified(parts) => parts.last().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

fn refresh_stmt(stmt: &mut Stmt) {
    match &mut stmt.kind {
        StmtKind::Fn { params, body, .. } => {
            for p in params {
                if let Some(d) = &mut p.default {
                    refresh_expr(d);
                }
            }
            refresh_block(body);
        }
        StmtKind::Struct { fields, body, .. } => {
            for f in fields {
                if let Some(d) = &mut f.default {
                    refresh_expr(d);
                }
            }
            refresh_block(body);
        }
        StmtKind::Impl { body, .. } | StmtKind::UnsafeBlock(body) => refresh_block(body),
        StmtKind::Trait { methods, .. } => refresh_block(methods),
        StmtKind::Enum { body, .. } => refresh_block(body),
        StmtKind::If {
            condition,
            then_body,
            elif_clauses,
            else_body,
        } => {
            refresh_expr(condition);
            refresh_block(then_body);
            for (c, b) in elif_clauses {
                refresh_expr(c);
                refresh_block(b);
            }
            if let Some(b) = else_body {
                refresh_block(b);
            }
        }
        StmtKind::While {
            condition,
            body,
            else_body,
        } => {
            refresh_expr(condition);
            refresh_block(body);
            if let Some(b) = else_body {
                refresh_block(b);
            }
        }
        StmtKind::For {
            iter,
            body,
            else_body,
            ..
        } => {
            refresh_expr(iter);
            refresh_block(body);
            if let Some(b) = else_body {
                refresh_block(b);
            }
        }
        StmtKind::With { items, body } => {
            for item in items {
                refresh_expr(&mut item.context_expr);
                if let Some(a) = &mut item.alias {
                    refresh_expr(a);
                }
            }
            refresh_block(body);
        }
        StmtKind::Return(Some(e))
        | StmtKind::ExprStmt(e)
        | StmtKind::Defer(e)
        | StmtKind::Let { value: e, .. }
        | StmtKind::MultiLet { value: e, .. }
        | StmtKind::Const { value: e, .. }
        | StmtKind::MultiConst { value: e, .. } => refresh_expr(e),
        StmtKind::Assert { test, msg } => {
            refresh_expr(test);
            if let Some(m) = msg {
                refresh_expr(m);
            }
        }
        StmtKind::Assign { target, value } => {
            refresh_expr(target);
            refresh_expr(value);
        }
        StmtKind::AugAssign { target, value, .. } => {
            refresh_expr(target);
            refresh_expr(value);
        }
        StmtKind::Return(None)
        | StmtKind::Pass
        | StmtKind::Break
        | StmtKind::Continue
        | StmtKind::Import { .. }
        | StmtKind::NativeImport { .. }
        | StmtKind::FromImport { .. }
        | StmtKind::PyImport { .. } => {}
    }
}

fn refresh_block(stmts: &mut [Stmt]) {
    for s in stmts {
        refresh_stmt(s);
    }
}

fn refresh_expr(expr: &mut Expr) {
    expr.id = fresh_node_id();
    match &mut expr.kind {
        ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::Identifier(_) => {}
        ExprKind::FStr(parts)
        | ExprKind::List(parts)
        | ExprKind::Tuple(parts)
        | ExprKind::Set(parts) => {
            for e in parts {
                refresh_expr(e);
            }
        }
        ExprKind::Dict(pairs) => {
            for (k, v) in pairs {
                refresh_expr(k);
                refresh_expr(v);
            }
        }
        ExprKind::BinOp { left, right, .. } => {
            refresh_expr(left);
            refresh_expr(right);
        }
        ExprKind::UnaryOp { operand, .. } => refresh_expr(operand),
        ExprKind::Cast(e, _) => refresh_expr(e),
        ExprKind::Call { callee, args } => {
            refresh_expr(callee);
            for a in args {
                match a {
                    CallArg::Positional(e)
                    | CallArg::Keyword(_, e)
                    | CallArg::Splat(e)
                    | CallArg::KwSplat(e) => refresh_expr(e),
                }
            }
        }
        ExprKind::Index { obj, index } => {
            refresh_expr(obj);
            refresh_expr(index);
        }
        ExprKind::Attr { obj, .. } => refresh_expr(obj),
        ExprKind::ListComp { elt, clauses } | ExprKind::SetComp { elt, clauses } => {
            refresh_expr(elt);
            refresh_clauses(clauses);
        }
        ExprKind::DictComp {
            key,
            value,
            clauses,
        } => {
            refresh_expr(key);
            refresh_expr(value);
            refresh_clauses(clauses);
        }
        ExprKind::Range { start, end, .. } => {
            refresh_expr(start);
            refresh_expr(end);
        }
        ExprKind::Borrow(e)
        | ExprKind::MutBorrow(e)
        | ExprKind::Deref(e)
        | ExprKind::Try(e)
        | ExprKind::Await(e) => refresh_expr(e),
        ExprKind::Slice { start, stop, step } => {
            for e in [start, stop, step].into_iter().flatten() {
                refresh_expr(e);
            }
        }
        ExprKind::AsyncBlock(body) => refresh_block(body),
        ExprKind::Match { expr, cases } => {
            refresh_expr(expr);
            for case in cases {
                if let MatchPattern::Literal(e) = &mut case.pattern {
                    refresh_expr(e);
                }
                refresh_block(&mut case.body);
            }
        }
    }
}

fn refresh_clauses(clauses: &mut [CompClause]) {
    for c in clauses {
        refresh_expr(&mut c.iter);
        if let Some(cond) = &mut c.condition {
            refresh_expr(cond);
        }
    }
}
