use crate::parser::{
    CallArg, CompClause, Expr, ExprKind, ForTarget, MatchPattern, Param, Stmt, StmtKind,
};
use std::borrow::Cow;
use std::collections::HashSet;

/// Qualifies an imported module's own top-level definitions with `prefix`
/// (`chan` -> `aio::chan`) and rewrites every reference to one of them
/// throughout the module, so a flat, single-namespace MIR/codegen layer
/// never collides two modules' same-named symbols.
///
/// A reference is only rewritten while it is actually visible under that
/// name -- a parameter, a `let`, a `for` target, a match binding, or a
/// nested `fn`/`struct`/`enum` that happens to share a top-level symbol's
/// name shadows it for the rest of that lexical scope, mirroring the
/// resolver's own scope discipline. Without this, a function whose own
/// parameter is named the same as some unrelated sibling top-level
/// definition would have every read of that parameter rewritten into a
/// call to the sibling instead.
pub fn mangle_statements(stmts: &mut [Stmt], prefix: &str, names: &HashSet<String>) {
    for stmt in stmts {
        mangle_stmt(stmt, prefix, names, true);
    }
}

/// Mangles one statement. `is_top_level` is true only for a statement
/// sitting directly in the list `mangle_statements` was called with --
/// only there does a `fn`/`struct`/`enum`/`let`/`const`/import declare a
/// genuine module-level symbol whose own name gets qualified. The same
/// statement shapes reached while descending into a body are always
/// local bindings, so their own name is left alone; only shadowing them
/// changes what gets mangled inside that body.
fn mangle_stmt(stmt: &mut Stmt, prefix: &str, names: &HashSet<String>, is_top_level: bool) {
    match &mut stmt.kind {
        StmtKind::Fn {
            name,
            body,
            params,
            return_type,
            ..
        } => {
            if is_top_level && names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
            for p in params.iter_mut() {
                if let Some(ty) = &mut p.type_ann {
                    mangle_type_expr(ty, prefix, names);
                }
            }
            if let Some(ty) = return_type {
                mangle_type_expr(ty, prefix, names);
            }
            mangle_param_defaults(params, prefix, names);
            let param_names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
            mangle_scoped(body, prefix, names, &param_names);
        }
        StmtKind::Struct {
            name, body, fields, ..
        } => {
            if is_top_level && names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
            for f in fields.iter_mut() {
                if let Some(ty) = &mut f.type_ann {
                    mangle_type_expr(ty, prefix, names);
                }
            }
            mangle_param_defaults(fields, prefix, names);
            for s in body {
                mangle_stmt(s, prefix, names, false);
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
                mangle_stmt(s, prefix, names, false);
            }
        }
        StmtKind::Trait { .. } => {}
        StmtKind::Enum { name, variants, .. } => {
            if is_top_level && names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
            for variant in variants {
                if is_top_level && names.contains(&variant.name) {
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
            mangle_scoped(then_body, prefix, names, &[]);
            for (cond, body) in elif_clauses {
                mangle_expr(cond, prefix, names);
                mangle_scoped(body, prefix, names, &[]);
            }
            if let Some(body) = else_body {
                mangle_scoped(body, prefix, names, &[]);
            }
        }
        StmtKind::While {
            condition,
            body,
            else_body,
        } => {
            mangle_expr(condition, prefix, names);
            mangle_scoped(body, prefix, names, &[]);
            if let Some(body) = else_body {
                mangle_scoped(body, prefix, names, &[]);
            }
        }
        StmtKind::For {
            target,
            iter,
            body,
            else_body,
        } => {
            mangle_expr(iter, prefix, names);
            let target_names = for_target_names(target);
            mangle_scoped(body, prefix, names, &target_names);
            if let Some(body) = else_body {
                mangle_scoped(body, prefix, names, &[]);
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
            if is_top_level && names.contains(name) {
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
            if is_top_level {
                for name in var_names {
                    if names.contains(name) {
                        *name = format!("{}::{}", prefix, name);
                    }
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
                mangle_stmt(s, prefix, names, false);
            }
        }
        StmtKind::Defer(expr) => {
            mangle_expr(expr, prefix, names);
        }
        StmtKind::Import { module, alias } => {
            let name = alias
                .as_deref()
                .unwrap_or_else(|| module.last().unwrap().as_str());
            if is_top_level && names.contains(name) {
                *alias = Some(format!("{}::{}", prefix, name));
            }
        }
        StmtKind::PyImport { alias, .. } | StmtKind::NativeImport { alias, .. }
            if is_top_level && names.contains(alias) =>
        {
            *alias = format!("{}::{}", prefix, alias);
        }
        StmtKind::FromImport {
            names: import_names,
            ..
        } => {
            if is_top_level {
                for (name, alias) in import_names {
                    let bound = alias.as_deref().unwrap_or(name.as_str());
                    if names.contains(bound) {
                        *alias = Some(format!("{}::{}", prefix, bound));
                    }
                }
            }
        }
        _ => {}
    }
}

/// Mangles a nested body that opens its own lexical scope (an `if`/`while`/
/// `for` body, a fn's own body, a match case's body, ...), given any names
/// that scope binds up front (a `for` target, a fn's params, ...). Nothing
/// shadowed here escapes back to the caller's own active set.
fn mangle_scoped(stmts: &mut [Stmt], prefix: &str, names: &HashSet<String>, shadow: &[&str]) {
    let active = shadow_names(names, shadow.iter().copied());
    mangle_block_inline(stmts, prefix, active);
}

/// Mangles each parameter's default-value expression, whether it belongs to
/// a `fn`'s parameter list or a `struct`'s field list -- both share `Param`.
/// A default is resolved in the scope of the params/fields already declared
/// before it (mirrors the resolver defining each one before checking its
/// default), so earlier names shadow a same-named top-level definition just
/// as they do in the function body.
fn mangle_param_defaults(params: &mut [Param], prefix: &str, names: &HashSet<String>) {
    let mut bound = Vec::with_capacity(params.len());
    for p in params.iter_mut() {
        bound.push(p.name.clone());
        if let Some(default) = &mut p.default {
            let active = shadow_names(names, bound.iter().map(String::as_str));
            mangle_expr(default, prefix, &active);
        }
    }
}

/// Mangles a statement sequence that shares one lexical scope with its
/// caller (a fn body, or an `unsafe:` block spliced transparently into its
/// parent): each `let`/`const`/nested `fn`/`struct`/`enum` shadows the rest
/// of the sequence but never what came before it, matching the resolver's
/// own hoist-then-execute-in-order discipline.
fn mangle_block_inline(stmts: &mut [Stmt], prefix: &str, mut active: Cow<HashSet<String>>) {
    for s in stmts.iter() {
        if let Some(n) = hoisted_name(&s.kind)
            && active.contains(n)
        {
            active.to_mut().remove(n);
        }
    }
    for s in stmts.iter_mut() {
        if let StmtKind::UnsafeBlock(body) = &mut s.kind {
            // Transparent to scoping (mirrors the resolver: no new scope),
            // so its bindings keep shadowing the rest of this same sequence.
            let taken = std::mem::take(&mut active).into_owned();
            active = Cow::Owned(mangle_unsafe_block(body, prefix, taken));
            continue;
        }
        mangle_stmt(s, prefix, &active, false);
        for n in sequential_bound_names(&s.kind) {
            if active.contains(n) {
                active.to_mut().remove(n);
            }
        }
    }
}

/// Runs `mangle_block_inline` over an `unsafe:` block's own statements and
/// hands back the (possibly further-shadowed) active set, since bindings
/// made inside it are visible to whatever follows it in the parent block.
fn mangle_unsafe_block(
    body: &mut [Stmt],
    prefix: &str,
    active: HashSet<String>,
) -> HashSet<String> {
    let mut remaining = active;
    for s in body.iter() {
        if let Some(n) = hoisted_name(&s.kind) {
            remaining.remove(n);
        }
    }
    for s in body.iter_mut() {
        mangle_stmt(s, prefix, &remaining, false);
        for n in sequential_bound_names(&s.kind) {
            remaining.remove(n);
        }
    }
    remaining
}

fn shadow_names<'a>(
    names: &'a HashSet<String>,
    shadow: impl Iterator<Item = &'a str>,
) -> Cow<'a, HashSet<String>> {
    let mut active = Cow::Borrowed(names);
    for n in shadow {
        if active.contains(n) {
            active.to_mut().remove(n);
        }
    }
    active
}

fn hoisted_name(kind: &StmtKind) -> Option<&str> {
    match kind {
        StmtKind::Fn { name, .. } | StmtKind::Struct { name, .. } | StmtKind::Enum { name, .. } => {
            Some(name.as_str())
        }
        _ => None,
    }
}

fn sequential_bound_names(kind: &StmtKind) -> Vec<&str> {
    match kind {
        StmtKind::Let { name, .. } | StmtKind::Const { name, .. } => vec![name.as_str()],
        StmtKind::MultiLet { names, .. } | StmtKind::MultiConst { names, .. } => {
            names.iter().map(String::as_str).collect()
        }
        _ => Vec::new(),
    }
}

fn for_target_names(target: &ForTarget) -> Vec<&str> {
    match target {
        ForTarget::Name(name, _) => vec![name.as_str()],
        ForTarget::Tuple(items) => items.iter().map(|(n, _)| n.as_str()).collect(),
    }
}

fn pattern_bound_names(pattern: &MatchPattern) -> Vec<&str> {
    let mut out = Vec::new();
    collect_pattern_names(pattern, &mut out);
    out
}

fn collect_pattern_names<'a>(pattern: &'a MatchPattern, out: &mut Vec<&'a str>) {
    match pattern {
        MatchPattern::Identifier(name, _) => out.push(name.as_str()),
        MatchPattern::Variant(_, args) | MatchPattern::Tuple(args) => {
            for p in args {
                collect_pattern_names(p, out);
            }
        }
        MatchPattern::StructFields(_, fields, _) => {
            for (_, p) in fields {
                collect_pattern_names(p, out);
            }
        }
        MatchPattern::List {
            before,
            rest,
            after,
        } => {
            for p in before.iter().chain(after) {
                collect_pattern_names(p, out);
            }
            if let Some((name, _)) = rest {
                out.push(name.as_str());
            }
        }
        MatchPattern::Or(alts) => {
            // Every alternative binds the same names (checker-enforced), so
            // the first alternative alone is representative.
            if let Some(first) = alts.first() {
                collect_pattern_names(first, out);
            }
        }
        MatchPattern::Literal(_) | MatchPattern::Wildcard | MatchPattern::Range(..) => {}
    }
}

/// Mangles a comprehension's clauses left to right, each clause's target
/// shadowing the rest for later clauses and for the final element/key/value
/// expression, the same cascading visibility a chain of nested `for`s has.
fn mangle_comp_clauses<'a>(
    clauses: &'a mut [CompClause],
    prefix: &str,
    names: &'a HashSet<String>,
) -> Cow<'a, HashSet<String>> {
    let mut active = Cow::Borrowed(names);
    for clause in clauses.iter_mut() {
        mangle_expr(&mut clause.iter, prefix, &active);
        for n in for_target_names(&clause.target) {
            if active.contains(n) {
                active.to_mut().remove(n);
            }
        }
        if let Some(cond) = &mut clause.condition {
            mangle_expr(cond, prefix, &active);
        }
    }
    active
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
                let bound = pattern_bound_names(&case.pattern);
                let active = shadow_names(names, bound.into_iter());
                if let Some(g) = &mut case.guard {
                    mangle_expr(g, prefix, &active);
                }
                mangle_block_inline(&mut case.body, prefix, active);
            }
        }
        ExprKind::ListComp { elt, clauses } | ExprKind::SetComp { elt, clauses } => {
            let active = mangle_comp_clauses(clauses, prefix, names);
            mangle_expr(elt, prefix, &active);
        }
        ExprKind::DictComp {
            key,
            value,
            clauses,
        } => {
            let active = mangle_comp_clauses(clauses, prefix, names);
            mangle_expr(key, prefix, &active);
            mangle_expr(value, prefix, &active);
        }
        ExprKind::AsyncBlock(body) => {
            mangle_block_inline(body, prefix, Cow::Borrowed(names));
        }
        ExprKind::Lambda { params, body } => {
            for p in params.iter_mut() {
                if let Some(ty) = &mut p.type_ann {
                    mangle_type_expr(ty, prefix, names);
                }
            }
            let param_names = params.iter().map(|p| p.name.as_str());
            let active = shadow_names(names, param_names);
            mangle_expr(body, prefix, &active);
        }
        _ => {}
    }
}

pub fn mangle_type_expr(ty: &mut crate::parser::TypeExpr, prefix: &str, names: &HashSet<String>) {
    use crate::parser::TypeExprKind;
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

    fn param(name: &str) -> Param {
        Param {
            name: name.into(),
            type_ann: None,
            default: None,
            kind: ParamKind::Regular,
            is_mut: false,
            span: sp(),
        }
    }

    #[test]
    fn mangle_fn_name() {
        let mut s = fn_stmt("f", vec![]);
        mangle_stmt(&mut s, "m", &ids(&["f"]), true);
        assert_eq!(name_of(&s), "m::f");
    }

    #[test]
    fn mangle_fn_skip_nonmatch() {
        let mut s = fn_stmt("g", vec![]);
        mangle_stmt(&mut s, "m", &ids(&["f"]), true);
        assert_eq!(name_of(&s), "g");
    }

    #[test]
    fn mangle_fn_body_cascades() {
        let mut s = fn_stmt("f", vec![stmt(StmtKind::ExprStmt(val("x")))]);
        mangle_stmt(&mut s, "m", &ids(&["x"]), true);
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
    fn mangle_fn_nested_name_not_renamed() {
        // A nested fn is a local binding, not a second export -- its own
        // name must never be qualified even if it collides with a sibling
        // top-level definition.
        let mut s = fn_stmt("outer", vec![fn_stmt("chan", vec![])]);
        mangle_stmt(&mut s, "m", &ids(&["chan"]), true);
        let body = match s.kind {
            StmtKind::Fn { body, .. } => body,
            _ => unreachable!(),
        };
        assert_eq!(name_of(&body[0]), "chan");
    }

    #[test]
    fn mangle_param_shadows_body_reference() {
        // A parameter named the same as an unrelated top-level `chan`
        // function must shadow it for the whole body, not get treated as
        // a reference to that function.
        let mut s = stmt(StmtKind::Fn {
            name: "_chan_len".into(),
            type_params: vec![],
            params: vec![param("chan")],
            return_type: None,
            body: vec![stmt(StmtKind::Return(Some(val("chan"))))],
            decorators: vec![],
            is_async: false,
        });
        mangle_stmt(&mut s, "aio", &ids(&["chan", "_chan_len"]), true);
        let body = match s.kind {
            StmtKind::Fn { body, .. } => body,
            _ => unreachable!(),
        };
        let e = match &body[0].kind {
            StmtKind::Return(Some(e)) => e,
            _ => unreachable!(),
        };
        assert_eq!(as_id(e), "chan");
    }

    #[test]
    fn mangle_let_local_shadows_and_keeps_its_own_name() {
        // Inside a body, `let chan = ...` is a local, not a redeclaration
        // of the top-level `chan` -- its own name stays plain, and it
        // shadows later references in the same body.
        let mut s = fn_stmt(
            "f",
            vec![
                stmt(StmtKind::Let {
                    name: "chan".into(),
                    name_span: sp(),
                    type_ann: None,
                    value: val("5"),
                    is_mut: false,
                }),
                stmt(StmtKind::ExprStmt(val("chan"))),
            ],
        );
        mangle_stmt(&mut s, "m", &ids(&["chan"]), true);
        let body = match s.kind {
            StmtKind::Fn { body, .. } => body,
            _ => unreachable!(),
        };
        assert_eq!(name_of(&body[0]), "chan");
        let e = match &body[1].kind {
            StmtKind::ExprStmt(e) => e,
            _ => unreachable!(),
        };
        assert_eq!(as_id(e), "chan");
    }

    #[test]
    fn mangle_let_before_shadow_still_mangled() {
        // A reference before the shadowing `let` still refers to the
        // top-level symbol.
        let mut s = fn_stmt(
            "f",
            vec![
                stmt(StmtKind::ExprStmt(val("chan"))),
                stmt(StmtKind::Let {
                    name: "chan".into(),
                    name_span: sp(),
                    type_ann: None,
                    value: val("5"),
                    is_mut: false,
                }),
            ],
        );
        mangle_stmt(&mut s, "m", &ids(&["chan"]), true);
        let body = match s.kind {
            StmtKind::Fn { body, .. } => body,
            _ => unreachable!(),
        };
        let e = match &body[0].kind {
            StmtKind::ExprStmt(e) => e,
            _ => unreachable!(),
        };
        assert_eq!(as_id(e), "m::chan");
    }

    #[test]
    fn mangle_for_target_shadows_body() {
        let mut s = stmt(StmtKind::For {
            target: ForTarget::Name("x".into(), sp()),
            iter: val("xs"),
            body: vec![stmt(StmtKind::ExprStmt(val("x")))],
            else_body: None,
        });
        mangle_stmt(&mut s, "m", &ids(&["x", "xs"]), true);
        let body = match s.kind {
            StmtKind::For { body, .. } => body,
            _ => unreachable!(),
        };
        let e = match &body[0].kind {
            StmtKind::ExprStmt(e) => e,
            _ => unreachable!(),
        };
        assert_eq!(as_id(e), "x");
    }

    #[test]
    fn mangle_if_scope_does_not_leak() {
        // A `let` inside one `if` branch must not shadow a sibling branch.
        let mut s = stmt(StmtKind::If {
            condition: val("c"),
            then_body: vec![
                stmt(StmtKind::Let {
                    name: "chan".into(),
                    name_span: sp(),
                    type_ann: None,
                    value: val("1"),
                    is_mut: false,
                }),
                stmt(StmtKind::ExprStmt(val("chan"))),
            ],
            elif_clauses: vec![],
            else_body: Some(vec![stmt(StmtKind::ExprStmt(val("chan")))]),
        });
        mangle_stmt(&mut s, "m", &ids(&["chan"]), true);
        let (then_body, else_body) = match s.kind {
            StmtKind::If {
                then_body,
                else_body,
                ..
            } => (then_body, else_body.unwrap()),
            _ => unreachable!(),
        };
        let then_ref = match &then_body[1].kind {
            StmtKind::ExprStmt(e) => e,
            _ => unreachable!(),
        };
        assert_eq!(as_id(then_ref), "chan");
        let else_ref = match &else_body[0].kind {
            StmtKind::ExprStmt(e) => e,
            _ => unreachable!(),
        };
        assert_eq!(as_id(else_ref), "m::chan");
    }

    #[test]
    fn mangle_lambda_param_shadows_body() {
        let mut e = expr(ExprKind::Lambda {
            params: vec![param("chan")],
            body: bx("chan"),
        });
        mangle_expr(&mut e, "m", &ids(&["chan"]));
        let body = match &e.kind {
            ExprKind::Lambda { body, .. } => body,
            _ => unreachable!(),
        };
        assert_eq!(as_id(body), "chan");
    }

    #[test]
    fn mangle_match_binding_shadows_case_body() {
        let mut e = expr(ExprKind::Match {
            expr: bx("scrutinee"),
            cases: vec![MatchCase {
                pattern: MatchPattern::Identifier("chan".into(), sp()),
                guard: None,
                body: vec![stmt(StmtKind::ExprStmt(val("chan")))],
                span: sp(),
            }],
        });
        mangle_expr(&mut e, "m", &ids(&["chan", "scrutinee"]));
        let cases = match &e.kind {
            ExprKind::Match { cases, .. } => cases,
            _ => unreachable!(),
        };
        let e = match &cases[0].body[0].kind {
            StmtKind::ExprStmt(e) => e,
            _ => unreachable!(),
        };
        assert_eq!(as_id(e), "chan");
    }

    #[test]
    fn mangle_listcomp_target_shadows_element() {
        let mut e = expr(ExprKind::ListComp {
            elt: bx("chan"),
            clauses: vec![CompClause {
                target: ForTarget::Name("chan".into(), sp()),
                iter: val("xs"),
                condition: None,
            }],
        });
        mangle_expr(&mut e, "m", &ids(&["chan", "xs"]));
        let elt = match &e.kind {
            ExprKind::ListComp { elt, .. } => elt,
            _ => unreachable!(),
        };
        assert_eq!(as_id(elt), "chan");
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
        mangle_stmt(&mut s, "m", &ids(&["P"]), true);
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
        mangle_stmt(&mut s, "m", &ids(&["O"]), true);
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
        mangle_stmt(&mut s, "m", &ids(&["V"]), true);
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
        mangle_stmt(&mut s, "m", &ids(&["x"]), true);
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
        mangle_stmt(&mut s, "m", &ids(&["F"]), true);
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
        mangle_stmt(&mut s, "m", &ids(&["c"]), true);
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
        mangle_stmt(&mut s, "m", &ids(&["c"]), true);
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
        mangle_stmt(&mut s, "m", &ids(&["a", "b"]), true);
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
        mangle_stmt(&mut s, "m", &ids(&["foo"]), true);
        assert!(matches!(s.kind, StmtKind::Import { alias, .. } if alias == Some("m::foo".into())));
    }

    #[test]
    fn mangle_defer() {
        let mut s = stmt(StmtKind::Defer(val("x")));
        mangle_stmt(&mut s, "m", &ids(&["x"]), true);
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
        mangle_stmt(&mut s, "m", &ids(&["t", "m"]), true);
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
        mangle_stmt(&mut s, "m", &ids(&["C"]), true);
        assert_eq!(name_of(&s), "m::C");
    }

    #[test]
    fn mangle_unsafe() {
        let mut s = stmt(StmtKind::UnsafeBlock(vec![stmt(StmtKind::ExprStmt(val(
            "x",
        )))]));
        mangle_stmt(&mut s, "m", &ids(&["x"]), true);
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
            starred: None,
        });
        mangle_stmt(&mut s, "m", &ids(&["a"]), true);
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
