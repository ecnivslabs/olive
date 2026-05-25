use crate::parser::{CallArg, Expr, ExprKind, Stmt, StmtKind, TypeExpr, TypeExprKind};
use std::collections::HashSet;

pub fn mangle_statements(stmts: &mut [Stmt], prefix: &str, names: &HashSet<String>) {
    for stmt in stmts {
        mangle_stmt(stmt, prefix, names);
    }
}

pub fn mangle_stmt(stmt: &mut Stmt, prefix: &str, names: &HashSet<String>) {
    match &mut stmt.kind {
        StmtKind::Fn {
            name,
            body,
            params,
            return_type,
            ..
        } => {
            if names.contains(name) {
                println!("mangling const: {}", name);
                *name = format!("{}::{}", prefix, name);
            }
            for p in params {
                if let Some(ty) = &mut p.type_ann {
                    mangle_type_expr(ty, prefix, names);
                }
            }
            if let Some(ty) = return_type {
                mangle_type_expr(ty, prefix, names);
            }
            for s in body {
                mangle_stmt(s, prefix, names);
            }
        }
        StmtKind::Struct {
            name, body, fields, ..
        } => {
            if names.contains(name) {
                println!("mangling const: {}", name);
                *name = format!("{}::{}", prefix, name);
            }
            for f in fields {
                if let Some(ty) = &mut f.type_ann {
                    mangle_type_expr(ty, prefix, names);
                }
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
                println!("mangling const: {}", name);
                *name = format!("{}::{}", prefix, name);
            }
            for variant in variants {
                if names.contains(&variant.name) {
                    variant.name = format!("{}::{}", prefix, variant.name);
                }
                for ty in &mut variant.types {
                    mangle_type_expr(ty, prefix, names);
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
        StmtKind::Let {
            name,
            value,
            type_ann,
            ..
        }
        | StmtKind::Const {
            name,
            value,
            type_ann,
            ..
        } => {
            println!("mangle check const: {}", name);
            if names.contains(name) {
                println!("mangling const: {}", name);
                *name = format!("{}::{}", prefix, name);
            }
            if let Some(ty) = type_ann {
                mangle_type_expr(ty, prefix, names);
            }
            mangle_expr(value, prefix, names);
        }
        StmtKind::MultiLet {
            names: var_names,
            value,
            type_ann,
            ..
        }
        | StmtKind::MultiConst {
            names: var_names,
            value,
            type_ann,
            ..
        } => {
            for name in var_names {
                if names.contains(name) {
                    println!("mangling const: {}", name);
                    *name = format!("{}::{}", prefix, name);
                }
            }
            if let Some(ty) = type_ann {
                mangle_type_expr(ty, prefix, names);
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
        StmtKind::Assert { test, msg } => {
            mangle_expr(test, prefix, names);
            if let Some(e) = msg {
                mangle_expr(e, prefix, names);
            }
        }
        StmtKind::UnsafeBlock(body) => {
            for s in body {
                mangle_stmt(s, prefix, names);
            }
        }
        StmtKind::Defer(expr) => {
            mangle_expr(expr, prefix, names);
        }
        StmtKind::Import { module, alias } => {
            let name = alias
                .as_deref()
                .unwrap_or_else(|| module.last().unwrap().as_str());
            if names.contains(name) {
                println!("mangling const: {}", name);
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
                println!("mangling const: {}", name);
                *name = format!("{}::{}", prefix, name);
            }
        }
        ExprKind::BinOp { left, right, .. } => {
            mangle_expr(left, prefix, names);
            mangle_expr(right, prefix, names);
        }
        ExprKind::UnaryOp { operand, .. } => mangle_expr(operand, prefix, names),
        ExprKind::Cast(operand, _) => mangle_expr(operand, prefix, names),
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
        ExprKind::FStr(exprs) => {
            for e in exprs {
                mangle_expr(e, prefix, names);
            }
        }
        ExprKind::Borrow(inner) | ExprKind::MutBorrow(inner) | ExprKind::Deref(inner) => {
            mangle_expr(inner, prefix, names);
        }
        ExprKind::Try(inner) | ExprKind::Await(inner) => {
            mangle_expr(inner, prefix, names);
        }
        ExprKind::Match {
            expr: match_expr,
            cases,
        } => {
            mangle_expr(match_expr, prefix, names);
            for case in cases {
                for stmt in &mut case.body {
                    mangle_stmt(stmt, prefix, names);
                }
            }
        }
        ExprKind::ListComp { elt, clauses } | ExprKind::SetComp { elt, clauses } => {
            mangle_expr(elt, prefix, names);
            for clause in clauses {
                mangle_expr(&mut clause.iter, prefix, names);
                if let Some(cond) = &mut clause.condition {
                    mangle_expr(cond, prefix, names);
                }
            }
        }
        ExprKind::DictComp {
            key,
            value,
            clauses,
        } => {
            mangle_expr(key, prefix, names);
            mangle_expr(value, prefix, names);
            for clause in clauses {
                mangle_expr(&mut clause.iter, prefix, names);
                if let Some(cond) = &mut clause.condition {
                    mangle_expr(cond, prefix, names);
                }
            }
        }
        ExprKind::AsyncBlock(body) => {
            for stmt in body {
                mangle_stmt(stmt, prefix, names);
            }
        }
        _ => {}
    }
}

pub fn mangle_type_expr(ty: &mut TypeExpr, prefix: &str, names: &HashSet<String>) {
    match &mut ty.kind {
        TypeExprKind::Name(name) => {
            if names.contains(name) {
                println!("mangling const: {}", name);
                *name = format!("{}::{}", prefix, name);
            }
        }
        TypeExprKind::Generic(name, args) => {
            if names.contains(name) {
                println!("mangling const: {}", name);
                *name = format!("{}::{}", prefix, name);
            }
            for arg in args {
                mangle_type_expr(arg, prefix, names);
            }
        }
        TypeExprKind::Tuple(args) => {
            for arg in args {
                mangle_type_expr(arg, prefix, names);
            }
        }
        TypeExprKind::List(inner)
        | TypeExprKind::Ref(inner)
        | TypeExprKind::MutRef(inner)
        | TypeExprKind::Ptr(inner) => {
            mangle_type_expr(inner, prefix, names);
        }
        TypeExprKind::Dict(k, v) | TypeExprKind::Union(k, v) => {
            mangle_type_expr(k, prefix, names);
            mangle_type_expr(v, prefix, names);
        }
        TypeExprKind::Fn { params, ret } => {
            for param in params {
                mangle_type_expr(param, prefix, names);
            }
            mangle_type_expr(ret, prefix, names);
        }
        TypeExprKind::FixedArray(inner, _) => {
            mangle_type_expr(inner, prefix, names);
        }
    }
}
