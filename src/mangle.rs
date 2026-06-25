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
            if let crate::parser::TypeExprKind::Name(n) = &mut type_name.kind
                && names.contains(n)
            {
                *n = format!("{}::{}", prefix, n);
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
            if names.contains(name) {
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
                *alias = Some(format!("{}::{}", prefix, name));
            }
        }
        StmtKind::PyImport { alias, .. } | StmtKind::NativeImport { alias, .. }
            if names.contains(alias) =>
        {
            *alias = format!("{}::{}", prefix, alias);
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
        ExprKind::Identifier(name) if names.contains(name) => {
            *name = format!("{}::{}", prefix, name);
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
        ExprKind::FStr(parts) => {
            for p in parts {
                mangle_expr(&mut p.expr, prefix, names);
            }
        }
        ExprKind::Borrow(inner) | ExprKind::MutBorrow(inner) | ExprKind::Deref(inner) => {
            mangle_expr(inner, prefix, names);
        }
        ExprKind::Try(inner) | ExprKind::Await(inner) => {
            mangle_expr(inner, prefix, names);
        }
        ExprKind::Ternary {
            cond,
            then,
            otherwise,
        } => {
            mangle_expr(cond, prefix, names);
            mangle_expr(then, prefix, names);
            mangle_expr(otherwise, prefix, names);
        }
        ExprKind::Match {
            expr: match_expr,
            cases,
        } => {
            mangle_expr(match_expr, prefix, names);
            for case in cases {
                if let Some(g) = &mut case.guard {
                    mangle_expr(g, prefix, names);
                }
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
        TypeExprKind::Qualified(_) => {}
        TypeExprKind::Name(name) => {
            if names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
        }
        TypeExprKind::Generic(name, args) => {
            if names.contains(name) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ast::*;
    use crate::span::Span;
    use std::collections::HashSet;

    fn sp() -> Span {
        Span {
            file_id: 0,
            line: 0,
            col: 0,
            start: 0,
            end: 0,
        }
    }

    fn val(name: &str) -> Expr {
        Expr {
            id: 0,
            kind: ExprKind::Identifier(name.to_string()),
            span: sp(),
        }
    }

    fn bx(name: &str) -> Box<Expr> {
        Box::new(val(name))
    }

    fn expr(kind: ExprKind) -> Box<Expr> {
        Box::new(Expr {
            id: 0,
            kind,
            span: sp(),
        })
    }

    fn ids(ns: &[&str]) -> HashSet<String> {
        ns.iter().map(|s| s.to_string()).collect()
    }

    fn stmt(kind: StmtKind) -> Stmt {
        Stmt { kind, span: sp() }
    }

    fn name_of(s: &Stmt) -> &str {
        match &s.kind {
            StmtKind::Fn { name, .. }
            | StmtKind::Struct { name, .. }
            | StmtKind::Enum { name, .. }
            | StmtKind::Let { name, .. }
            | StmtKind::Const { name, .. } => name,
            _ => panic!("no name"),
        }
    }

    fn as_id(e: &Expr) -> &str {
        match &e.kind {
            ExprKind::Identifier(s) => s,
            _ => panic!("not ident"),
        }
    }

    fn fn_stmt(name: &str, body: Vec<Stmt>) -> Stmt {
        stmt(StmtKind::Fn {
            name: name.into(),
            type_params: vec![],
            params: vec![],
            return_type: None,
            body,
            decorators: vec![],
            is_async: false,
        })
    }

    #[test]
    fn mangle_fn_name() {
        let mut s = fn_stmt("f", vec![]);
        mangle_stmt(&mut s, "m", &ids(&["f"]));
        assert_eq!(name_of(&s), "m::f");
    }

    #[test]
    fn mangle_fn_skip_nonmatch() {
        let mut s = fn_stmt("g", vec![]);
        mangle_stmt(&mut s, "m", &ids(&["f"]));
        assert_eq!(name_of(&s), "g");
    }

    #[test]
    fn mangle_fn_body_cascades() {
        let mut s = fn_stmt("f", vec![stmt(StmtKind::ExprStmt(val("x")))]);
        mangle_stmt(&mut s, "m", &ids(&["x"]));
        let body = match s.kind {
            StmtKind::Fn { body, .. } => body,
            _ => unreachable!(),
        };
        let e = match &body[0].kind {
            StmtKind::ExprStmt(e) => e,
            _ => unreachable!(),
        };
        assert_eq!(as_id(e), "m::x");
    }

    #[test]
    fn mangle_struct() {
        let mut s = stmt(StmtKind::Struct {
            name: "P".into(),
            type_params: vec![],
            fields: vec![],
            body: vec![],
            decorators: vec![],
        });
        mangle_stmt(&mut s, "m", &ids(&["P"]));
        assert_eq!(name_of(&s), "m::P");
    }

    #[test]
    fn mangle_enum() {
        let mut s = stmt(StmtKind::Enum {
            name: "O".into(),
            type_params: vec![],
            variants: vec![],
            body: vec![],
            decorators: vec![],
        });
        mangle_stmt(&mut s, "m", &ids(&["O"]));
        assert_eq!(name_of(&s), "m::O");
    }

    #[test]
    fn mangle_enum_variant() {
        let mut s = stmt(StmtKind::Enum {
            name: "O".into(),
            type_params: vec![],
            variants: vec![EnumVariant {
                name: "V".into(),
                types: vec![],
                value: None,
            }],
            body: vec![],
            decorators: vec![],
        });
        mangle_stmt(&mut s, "m", &ids(&["V"]));
        let vs = match s.kind {
            StmtKind::Enum { variants, .. } => variants,
            _ => unreachable!(),
        };
        assert_eq!(vs[0].name, "m::V");
    }

    #[test]
    fn mangle_let() {
        let mut s = stmt(StmtKind::Let {
            name: "x".into(),
            name_span: crate::span::Span::default(),
            value: val("1"),
            type_ann: None,
            is_mut: false,
        });
        mangle_stmt(&mut s, "m", &ids(&["x"]));
        assert_eq!(name_of(&s), "m::x");
    }

    #[test]
    fn mangle_impl() {
        let mut s = stmt(StmtKind::Impl {
            type_params: vec![],
            trait_name: None,
            type_name: TypeExpr {
                kind: TypeExprKind::Name("F".into()),
                span: sp(),
            },
            body: vec![],
        });
        mangle_stmt(&mut s, "m", &ids(&["F"]));
        let tn = match s.kind {
            StmtKind::Impl { type_name, .. } => type_name,
            _ => unreachable!(),
        };
        assert!(matches!(&tn.kind, TypeExprKind::Name(n) if n == "m::F"));
    }

    #[test]
    fn mangle_if() {
        let mut s = stmt(StmtKind::If {
            condition: val("c"),
            then_body: vec![],
            elif_clauses: vec![],
            else_body: None,
        });
        mangle_stmt(&mut s, "m", &ids(&["c"]));
        let c = match s.kind {
            StmtKind::If { condition, .. } => condition,
            _ => unreachable!(),
        };
        assert_eq!(as_id(&c), "m::c");
    }

    #[test]
    fn mangle_while() {
        let mut s = stmt(StmtKind::While {
            condition: val("c"),
            body: vec![],
            else_body: None,
        });
        mangle_stmt(&mut s, "m", &ids(&["c"]));
        let c = match s.kind {
            StmtKind::While { condition, .. } => condition,
            _ => unreachable!(),
        };
        assert_eq!(as_id(&c), "m::c");
    }

    #[test]
    fn mangle_assign() {
        let mut s = stmt(StmtKind::Assign {
            target: val("a"),
            value: val("b"),
        });
        mangle_stmt(&mut s, "m", &ids(&["a", "b"]));
        let (t, v) = match s.kind {
            StmtKind::Assign { target, value } => (target, value),
            _ => unreachable!(),
        };
        assert_eq!(as_id(&t), "m::a");
        assert_eq!(as_id(&v), "m::b");
    }

    #[test]
    fn mangle_expr_id() {
        let mut e = bx("foo");
        mangle_expr(&mut e, "m", &ids(&["foo"]));
        assert_eq!(as_id(&e), "m::foo");
    }

    #[test]
    fn mangle_expr_binop() {
        let mut e = expr(ExprKind::BinOp {
            left: bx("a"),
            op: BinOp::Add,
            right: bx("b"),
        });
        mangle_expr(&mut e, "m", &ids(&["a", "b"]));
        let (l, r) = match &e.kind {
            ExprKind::BinOp { left, right, .. } => (left, right),
            _ => unreachable!(),
        };
        assert_eq!(as_id(l), "m::a");
        assert_eq!(as_id(r), "m::b");
    }

    #[test]
    fn mangle_expr_call() {
        let mut e = expr(ExprKind::Call {
            callee: bx("f"),
            args: vec![CallArg::Positional(val("x"))],
        });
        mangle_expr(&mut e, "m", &ids(&["f", "x"]));
        let callee = match &e.kind {
            ExprKind::Call { callee, .. } => callee,
            _ => unreachable!(),
        };
        assert_eq!(as_id(callee), "m::f");
    }

    #[test]
    fn mangle_type_name() {
        let mut ty = TypeExpr {
            kind: TypeExprKind::Name("T".into()),
            span: sp(),
        };
        mangle_type_expr(&mut ty, "m", &ids(&["T"]));
        assert!(matches!(&ty.kind, TypeExprKind::Name(n) if n == "m::T"));
    }

    #[test]
    fn mangle_type_generic() {
        let mut ty = TypeExpr {
            kind: TypeExprKind::Generic(
                "B".into(),
                vec![TypeExpr {
                    kind: TypeExprKind::Name("T".into()),
                    span: sp(),
                }],
            ),
            span: sp(),
        };
        mangle_type_expr(&mut ty, "m", &ids(&["B", "T"]));
        let (n, args) = match ty.kind {
            TypeExprKind::Generic(n, a) => (n, a),
            _ => unreachable!(),
        };
        assert_eq!(n, "m::B");
        assert!(matches!(&args[0].kind, TypeExprKind::Name(s) if s == "m::T"));
    }

    #[test]
    fn mangle_import() {
        let mut s = stmt(StmtKind::Import {
            module: vec!["foo".into()],
            alias: None,
        });
        mangle_stmt(&mut s, "m", &ids(&["foo"]));
        assert!(matches!(s.kind, StmtKind::Import { alias, .. } if alias == Some("m::foo".into())));
    }

    #[test]
    fn mangle_defer() {
        let mut s = stmt(StmtKind::Defer(val("x")));
        mangle_stmt(&mut s, "m", &ids(&["x"]));
        let e = match s.kind {
            StmtKind::Defer(e) => e,
            _ => unreachable!(),
        };
        assert_eq!(as_id(&e), "m::x");
    }

    #[test]
    fn mangle_assert() {
        let mut s = stmt(StmtKind::Assert {
            test: val("t"),
            msg: Some(val("m")),
        });
        mangle_stmt(&mut s, "m", &ids(&["t", "m"]));
        let (t, msg) = match s.kind {
            StmtKind::Assert { test, msg } => (test, msg),
            _ => unreachable!(),
        };
        assert_eq!(as_id(&t), "m::t");
        assert_eq!(as_id(&msg.unwrap()), "m::m");
    }

    #[test]
    fn mangle_const() {
        let mut s = stmt(StmtKind::Const {
            name: "C".into(),
            name_span: crate::span::Span::default(),
            value: val("1"),
            type_ann: None,
        });
        mangle_stmt(&mut s, "m", &ids(&["C"]));
        assert_eq!(name_of(&s), "m::C");
    }

    #[test]
    fn mangle_unsafe() {
        let mut s = stmt(StmtKind::UnsafeBlock(vec![stmt(StmtKind::ExprStmt(val(
            "x",
        )))]));
        mangle_stmt(&mut s, "m", &ids(&["x"]));
        let body = match s.kind {
            StmtKind::UnsafeBlock(b) => b,
            _ => unreachable!(),
        };
        let e = match &body[0].kind {
            StmtKind::ExprStmt(e) => e,
            _ => unreachable!(),
        };
        assert_eq!(as_id(e), "m::x");
    }

    #[test]
    fn mangle_multi_let() {
        let mut s = stmt(StmtKind::MultiLet {
            names: vec!["a".into(), "b".into()],
            name_spans: vec![crate::span::Span::default(), crate::span::Span::default()],
            value: val("1"),
            type_ann: None,
            is_mut: false,
        });
        mangle_stmt(&mut s, "m", &ids(&["a"]));
        let names = match s.kind {
            StmtKind::MultiLet { names, .. } => names,
            _ => unreachable!(),
        };
        assert_eq!(names[0], "m::a");
        assert_eq!(names[1], "b");
    }

    #[test]
    fn mangle_expr_attr() {
        let mut e = expr(ExprKind::Attr {
            obj: bx("o"),
            attr: "f".into(),
        });
        mangle_expr(&mut e, "m", &ids(&["o"]));
        let obj = match &e.kind {
            ExprKind::Attr { obj, .. } => obj,
            _ => unreachable!(),
        };
        assert_eq!(as_id(obj), "m::o");
    }

    #[test]
    fn mangle_expr_index() {
        let mut e = expr(ExprKind::Index {
            obj: bx("a"),
            index: bx("i"),
        });
        mangle_expr(&mut e, "m", &ids(&["a", "i"]));
        let (obj, idx) = match &e.kind {
            ExprKind::Index { obj, index } => (obj, index),
            _ => unreachable!(),
        };
        assert_eq!(as_id(obj), "m::a");
        assert_eq!(as_id(idx), "m::i");
    }

    #[test]
    fn mangle_expr_borrow() {
        let mut e = expr(ExprKind::Borrow(bx("x")));
        mangle_expr(&mut e, "m", &ids(&["x"]));
        let inner = match &e.kind {
            ExprKind::Borrow(i) => i,
            _ => unreachable!(),
        };
        assert_eq!(as_id(inner), "m::x");
    }

    #[test]
    fn mangle_expr_list() {
        let mut e = expr(ExprKind::List(vec![val("x"), val("y")]));
        mangle_expr(&mut e, "m", &ids(&["x"]));
        let elems = match &e.kind {
            ExprKind::List(elems) => elems,
            _ => unreachable!(),
        };
        assert_eq!(as_id(&elems[0]), "m::x");
        assert_eq!(as_id(&elems[1]), "y");
    }

    #[test]
    fn mangle_expr_dict() {
        let mut e = expr(ExprKind::Dict(vec![(val("k"), val("v"))]));
        mangle_expr(&mut e, "m", &ids(&["k", "v"]));
        let pairs = match &e.kind {
            ExprKind::Dict(p) => p,
            _ => unreachable!(),
        };
        assert_eq!(as_id(&pairs[0].0), "m::k");
        assert_eq!(as_id(&pairs[0].1), "m::v");
    }

    #[test]
    fn mangle_expr_cast() {
        let mut e = expr(ExprKind::Cast(
            bx("x"),
            TypeExpr {
                kind: TypeExprKind::Name("i64".into()),
                span: sp(),
            },
        ));
        mangle_expr(&mut e, "m", &ids(&["x"]));
        let inner = match &e.kind {
            ExprKind::Cast(inner, _) => inner,
            _ => unreachable!(),
        };
        assert_eq!(as_id(inner), "m::x");
    }

    #[test]
    fn mangle_type_list() {
        let mut ty = TypeExpr {
            kind: TypeExprKind::List(Box::new(TypeExpr {
                kind: TypeExprKind::Name("T".into()),
                span: sp(),
            })),
            span: sp(),
        };
        mangle_type_expr(&mut ty, "m", &ids(&["T"]));
        assert!(
            matches!(&ty.kind, TypeExprKind::List(i) if matches!(&i.kind, TypeExprKind::Name(n) if n == "m::T"))
        );
    }

    #[test]
    fn mangle_type_fn() {
        let mut ty = TypeExpr {
            kind: TypeExprKind::Fn {
                params: vec![TypeExpr {
                    kind: TypeExprKind::Name("T".into()),
                    span: sp(),
                }],
                ret: Box::new(TypeExpr {
                    kind: TypeExprKind::Name("R".into()),
                    span: sp(),
                }),
            },
            span: sp(),
        };
        mangle_type_expr(&mut ty, "m", &ids(&["T", "R"]));
        let (params, ret) = match ty.kind {
            TypeExprKind::Fn { params, ret } => (params, ret),
            _ => unreachable!(),
        };
        assert!(matches!(&params[0].kind, TypeExprKind::Name(n) if n == "m::T"));
        assert!(matches!(&ret.kind, TypeExprKind::Name(n) if n == "m::R"));
    }

    #[test]
    fn mangle_stmts_toplevel() {
        let mut xs = vec![stmt(StmtKind::ExprStmt(val("x")))];
        mangle_statements(&mut xs, "m", &ids(&["x"]));
        let e = match &xs[0].kind {
            StmtKind::ExprStmt(e) => e,
            _ => unreachable!(),
        };
        assert_eq!(as_id(e), "m::x");
    }
}
