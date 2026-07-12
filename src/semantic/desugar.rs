use crate::parser::ast::*;
use rustc_hash::{FxHashMap, FxHashSet};

/// Rewrites a bare reference to a nullary enum variant (`Color::Red` or `Red`)
/// in value position into the zero-argument constructor call the rest of the
/// compiler expects (`Color::Red()`). Without this a variant used as a value
/// types as its constructor function, not as the enum. A variant that is already
/// being called is left alone.
pub fn desugar_bare_variants(program: &mut Program) {
    let mut nullary: FxHashSet<String> = FxHashSet::default();
    for stmt in &program.stmts {
        if let StmtKind::Enum { name, variants, .. } = &stmt.kind {
            for v in variants {
                if v.types.is_empty() {
                    nullary.insert(v.name.clone());
                    nullary.insert(format!("{}::{}", name, v.name));
                }
            }
        }
    }
    if nullary.is_empty() {
        return;
    }
    for stmt in &mut program.stmts {
        bare_stmt(stmt, &nullary);
    }
}

fn bare_block(stmts: &mut [Stmt], nullary: &FxHashSet<String>) {
    for s in stmts {
        bare_stmt(s, nullary);
    }
}

fn bare_stmt(stmt: &mut Stmt, nullary: &FxHashSet<String>) {
    match &mut stmt.kind {
        StmtKind::Fn { params, body, .. } => {
            for p in params {
                if let Some(d) = &mut p.default {
                    bare_expr(d, nullary);
                }
            }
            bare_block(body, nullary);
        }
        StmtKind::Struct { fields, body, .. } => {
            for f in fields {
                if let Some(d) = &mut f.default {
                    bare_expr(d, nullary);
                }
            }
            bare_block(body, nullary);
        }
        StmtKind::Impl { body, .. } | StmtKind::UnsafeBlock(body) => bare_block(body, nullary),
        StmtKind::Trait { methods, .. } => bare_block(methods, nullary),
        StmtKind::Enum { body, .. } => bare_block(body, nullary),
        StmtKind::If {
            condition,
            then_body,
            elif_clauses,
            else_body,
        } => {
            bare_expr(condition, nullary);
            bare_block(then_body, nullary);
            for (c, b) in elif_clauses {
                bare_expr(c, nullary);
                bare_block(b, nullary);
            }
            if let Some(b) = else_body {
                bare_block(b, nullary);
            }
        }
        StmtKind::While {
            condition,
            body,
            else_body,
        } => {
            bare_expr(condition, nullary);
            bare_block(body, nullary);
            if let Some(b) = else_body {
                bare_block(b, nullary);
            }
        }
        StmtKind::For {
            iter,
            body,
            else_body,
            ..
        } => {
            bare_expr(iter, nullary);
            bare_block(body, nullary);
            if let Some(b) = else_body {
                bare_block(b, nullary);
            }
        }
        StmtKind::With { items, body } => {
            for item in items {
                bare_expr(&mut item.context_expr, nullary);
                if let Some(a) = &mut item.alias {
                    bare_expr(a, nullary);
                }
            }
            bare_block(body, nullary);
        }
        StmtKind::Return(Some(e))
        | StmtKind::ExprStmt(e)
        | StmtKind::Defer(e)
        | StmtKind::Let { value: e, .. }
        | StmtKind::MultiLet { value: e, .. }
        | StmtKind::Const { value: e, .. }
        | StmtKind::MultiConst { value: e, .. } => bare_expr(e, nullary),
        StmtKind::Assert { test, msg } => {
            bare_expr(test, nullary);
            if let Some(m) = msg {
                bare_expr(m, nullary);
            }
        }
        StmtKind::Assign { target, value } | StmtKind::AugAssign { target, value, .. } => {
            bare_expr(target, nullary);
            bare_expr(value, nullary);
        }
        _ => {}
    }
}

/// Recurses into every sub-expression, then rewrites this node if it is a bare
/// nullary-variant identifier. The callee of a call is skipped when it is a bare
/// identifier so an existing `Variant()` call is not wrapped again.
fn bare_expr(expr: &mut Expr, nullary: &FxHashSet<String>) {
    match &mut expr.kind {
        ExprKind::Call { callee, args } => {
            if !matches!(callee.kind, ExprKind::Identifier(_)) {
                bare_expr(callee, nullary);
            }
            for a in args {
                match a {
                    CallArg::Positional(e)
                    | CallArg::Keyword(_, e)
                    | CallArg::Splat(e)
                    | CallArg::KwSplat(e) => bare_expr(e, nullary),
                }
            }
            return;
        }
        ExprKind::FStr(parts) => {
            for p in parts {
                bare_expr(&mut p.expr, nullary);
            }
        }
        ExprKind::List(parts) | ExprKind::Tuple(parts) | ExprKind::Set(parts) => {
            for e in parts {
                bare_expr(e, nullary);
            }
        }
        ExprKind::Dict(pairs) => {
            for (k, v) in pairs {
                bare_expr(k, nullary);
                bare_expr(v, nullary);
            }
        }
        ExprKind::BinOp { left, right, .. } => {
            bare_expr(left, nullary);
            bare_expr(right, nullary);
        }
        ExprKind::UnaryOp { operand, .. } => bare_expr(operand, nullary),
        ExprKind::Cast(e, _) => bare_expr(e, nullary),
        ExprKind::Index { obj, index } => {
            bare_expr(obj, nullary);
            bare_expr(index, nullary);
        }
        ExprKind::Attr { obj, .. } | ExprKind::OptAttr { obj, .. } => bare_expr(obj, nullary),
        ExprKind::ListComp { elt, clauses } | ExprKind::SetComp { elt, clauses } => {
            bare_expr(elt, nullary);
            bare_clauses(clauses, nullary);
        }
        ExprKind::DictComp {
            key,
            value,
            clauses,
        } => {
            bare_expr(key, nullary);
            bare_expr(value, nullary);
            bare_clauses(clauses, nullary);
        }
        ExprKind::Range { start, end, .. } => {
            bare_expr(start, nullary);
            bare_expr(end, nullary);
        }
        ExprKind::Borrow(e)
        | ExprKind::MutBorrow(e)
        | ExprKind::Deref(e)
        | ExprKind::Try(e)
        | ExprKind::Await(e) => bare_expr(e, nullary),
        ExprKind::Slice { start, stop, step } => {
            for e in [start, stop, step].into_iter().flatten() {
                bare_expr(e, nullary);
            }
        }
        ExprKind::AsyncBlock(body) => bare_block(body, nullary),
        ExprKind::Ternary {
            cond,
            then,
            otherwise,
        } => {
            bare_expr(cond, nullary);
            bare_expr(then, nullary);
            bare_expr(otherwise, nullary);
        }
        ExprKind::Match { expr, cases } => {
            bare_expr(expr, nullary);
            for case in cases {
                if let Some(g) = &mut case.guard {
                    bare_expr(g, nullary);
                }
                bare_block(&mut case.body, nullary);
            }
        }
        ExprKind::Lambda { params, body } => {
            for p in params {
                if let Some(d) = &mut p.default {
                    bare_expr(d, nullary);
                }
            }
            bare_expr(body, nullary);
        }
        _ => {}
    }
    if let ExprKind::Identifier(name) = &expr.kind
        && nullary.contains(name)
    {
        let span = expr.span;
        let callee = Box::new(Expr::new(ExprKind::Identifier(name.clone()), span));
        expr.kind = ExprKind::Call {
            callee,
            args: vec![],
        };
    }
}

fn bare_clauses(clauses: &mut [CompClause], nullary: &FxHashSet<String>) {
    for c in clauses {
        bare_expr(&mut c.iter, nullary);
        if let Some(cond) = &mut c.condition {
            bare_expr(cond, nullary);
        }
    }
}

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
        | StmtKind::PyImport { .. }
        | StmtKind::TypeAlias { .. } => {}
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
        ExprKind::FStr(parts) => {
            for p in parts {
                refresh_expr(&mut p.expr);
            }
        }
        ExprKind::List(parts) | ExprKind::Tuple(parts) | ExprKind::Set(parts) => {
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
        ExprKind::Attr { obj, .. } | ExprKind::OptAttr { obj, .. } => refresh_expr(obj),
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
        | ExprKind::Starred(e)
        | ExprKind::Try(e)
        | ExprKind::Await(e) => refresh_expr(e),
        ExprKind::Slice { start, stop, step } => {
            for e in [start, stop, step].into_iter().flatten() {
                refresh_expr(e);
            }
        }
        ExprKind::AsyncBlock(body) => refresh_block(body),
        ExprKind::Ternary {
            cond,
            then,
            otherwise,
        } => {
            refresh_expr(cond);
            refresh_expr(then);
            refresh_expr(otherwise);
        }
        ExprKind::Match { expr, cases } => {
            refresh_expr(expr);
            for case in cases {
                if let MatchPattern::Literal(e) = &mut case.pattern {
                    refresh_expr(e);
                }
                if let Some(g) = &mut case.guard {
                    refresh_expr(g);
                }
                refresh_block(&mut case.body);
            }
        }
        ExprKind::Lambda { params, body } => {
            for p in params {
                if let Some(d) = &mut p.default {
                    refresh_expr(d);
                }
            }
            refresh_expr(body);
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
