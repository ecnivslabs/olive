use crate::parser::{CallArg, Expr, ExprKind, Stmt, StmtKind};
use std::collections::HashSet;

pub fn mangle_statements(stmts: &mut [Stmt], prefix: &str, names: &HashSet<String>) {
    for stmt in stmts {
        mangle_stmt(stmt, prefix, names);
    }
}

pub fn mangle_stmt(stmt: &mut Stmt, prefix: &str, names: &HashSet<String>) {
    match &mut stmt.kind {
        StmtKind::Fn { name, body, .. } => {
            if names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
            for s in body {
                mangle_stmt(s, prefix, names);
            }
        }
        StmtKind::Struct { name, body, .. } => {
            if names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
            for s in body {
                mangle_stmt(s, prefix, names);
            }
        }
        StmtKind::Impl {
            type_name, body, ..
        } => {
            if let crate::parser::TypeExprKind::Name(n) = &mut type_name.kind {
                if names.contains(n) {
                    *n = format!("{}::{}", prefix, n);
                }
            }
            for s in body {
                mangle_stmt(s, prefix, names);
            }
        }
        StmtKind::Trait { .. } => {}
        StmtKind::Enum { name, variants, .. } => {
            if names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
            for variant in variants {
                if names.contains(&variant.name) {
                    variant.name = format!("{}::{}", prefix, variant.name);
                }
            }
        }
        StmtKind::If {
            then_body,
            elif_clauses,
            else_body,
            condition,
        } => {
            mangle_expr(condition, prefix, names);
            for s in then_body {
                mangle_stmt(s, prefix, names);
            }
            for (cond, body) in elif_clauses {
                mangle_expr(cond, prefix, names);
                for s in body {
                    mangle_stmt(s, prefix, names);
                }
            }
            if let Some(body) = else_body {
                for s in body {
                    mangle_stmt(s, prefix, names);
                }
            }
        }
        StmtKind::While {
            condition,
            body,
            else_body,
        } => {
            mangle_expr(condition, prefix, names);
            for s in body {
                mangle_stmt(s, prefix, names);
            }
            if let Some(body) = else_body {
                for s in body {
                    mangle_stmt(s, prefix, names);
                }
            }
        }
        StmtKind::For {
            iter,
            body,
            else_body,
            ..
        } => {
            mangle_expr(iter, prefix, names);
            for s in body {
                mangle_stmt(s, prefix, names);
            }
            if let Some(body) = else_body {
                for s in body {
                    mangle_stmt(s, prefix, names);
                }
            }
        }
        StmtKind::Let { name, value, .. } | StmtKind::Const { name, value, .. } => {
            if names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
            mangle_expr(value, prefix, names);
        }
        StmtKind::MultiLet {
            names: var_names,
            value,
            ..
        }
        | StmtKind::MultiConst {
            names: var_names,
            value,
            ..
        } => {
            for name in var_names {
                if names.contains(name) {
                    *name = format!("{}::{}", prefix, name);
                }
            }
            mangle_expr(value, prefix, names);
        }
        StmtKind::Assign { target, value } | StmtKind::AugAssign { target, value, .. } => {
            mangle_expr(target, prefix, names);
            mangle_expr(value, prefix, names);
        }
        StmtKind::Return(Some(e)) | StmtKind::ExprStmt(e) => {
            mangle_expr(e, prefix, names);
        }
        StmtKind::Import { module, alias } => {
            let name = alias
                .as_deref()
                .unwrap_or_else(|| module.last().unwrap().as_str());
            if names.contains(name) {
                *alias = Some(format!("{}::{}", prefix, name));
            }
        }
        StmtKind::PyImport { alias, .. } | StmtKind::NativeImport { alias, .. } => {
            if names.contains(alias) {
                *alias = format!("{}::{}", prefix, alias);
            }
        }
        StmtKind::FromImport {
            names: import_names,
            ..
        } => {
            for (name, alias) in import_names {
                let bound = alias.as_deref().unwrap_or(name.as_str());
                if names.contains(bound) {
                    *alias = Some(format!("{}::{}", prefix, bound));
                }
            }
        }
        _ => {}
    }
}

pub fn mangle_expr(expr: &mut Expr, prefix: &str, names: &HashSet<String>) {
    match &mut expr.kind {
        ExprKind::Identifier(name) => {
            if names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
        }
        ExprKind::BinOp { left, right, .. } => {
            mangle_expr(left, prefix, names);
            mangle_expr(right, prefix, names);
        }
        ExprKind::UnaryOp { operand, .. } => mangle_expr(operand, prefix, names),
        ExprKind::Call { callee, args } => {
            mangle_expr(callee, prefix, names);
            for arg in args {
                match arg {
                    CallArg::Positional(e)
                    | CallArg::Keyword(_, e)
                    | CallArg::Splat(e)
                    | CallArg::KwSplat(e) => {
                        mangle_expr(e, prefix, names);
                    }
                }
            }
        }
        ExprKind::Index { obj, index } => {
            mangle_expr(obj, prefix, names);
            mangle_expr(index, prefix, names);
        }
        ExprKind::Attr { obj, .. } => mangle_expr(obj, prefix, names),
        ExprKind::List(elems) | ExprKind::Tuple(elems) | ExprKind::Set(elems) => {
            for e in elems {
                mangle_expr(e, prefix, names);
            }
        }
        ExprKind::Dict(pairs) => {
            for (k, v) in pairs {
                mangle_expr(k, prefix, names);
                mangle_expr(v, prefix, names);
            }
        }
        _ => {}
    }
}
