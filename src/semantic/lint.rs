//! Source-level lints that run after name resolution succeeds. These are
//! warnings, not errors: they never block compilation, but they catch the
//! quiet mistakes a type checker is happy to accept, like an import nothing uses
//! or code that can never run. Each lint is a pure pass over the AST and reports
//! through the same rich diagnostic channel as every other message.

use crate::compile::errors::Diagnostic;
use crate::parser::ast::{
    CallArg, CompClause, Expr, ExprKind, MatchPattern, Program, Stmt, StmtKind, TypeExpr,
    TypeExprKind, WithItem,
};
use crate::span::Span;
use rustc_hash::FxHashSet;

/// Runs every lint over a resolved program and returns the warnings to emit.
pub fn lint_program(program: &Program) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut referenced = FxHashSet::default();
    collect_refs(&program.stmts, &mut referenced);
    check_unused_imports(program, &referenced, &mut out);
    check_dead_code(program, &referenced, &mut out);
    check_unreachable(&program.stmts, &mut out);
    check_ffi_conventions(&program.stmts, &mut out);
    out
}

/// Reports a top-level function that is module-private (its name begins with
/// `_`, the convention for "not exported") yet is never referenced anywhere in
/// the program. Restricting to private functions mirrors how Rust only flags
/// non-`pub` dead code: a public function might be called by an importer the
/// pass cannot see, so it is left alone.
fn check_dead_code(program: &Program, referenced: &FxHashSet<String>, out: &mut Vec<Diagnostic>) {
    for stmt in &program.stmts {
        let StmtKind::Fn { name, .. } = &stmt.kind else {
            continue;
        };
        if !name.starts_with('_') || referenced.contains(name) {
            continue;
        }
        out.push(
            Diagnostic::error(
                "W0650",
                format!("function `{name}` is never used"),
                stmt.span,
            )
            .into_warning()
            .label("defined here but never called")
            .note(
                "the name is private to its module (it begins with `_`) and nothing references it",
            )
            .help("remove it, or call it where it is needed"),
        );
    }
}

/// Flags a foreign function whose declared calling convention has no effect on
/// the target. `stdcall`/`fastcall` apply only to 32-bit Windows; elsewhere the
/// platform ABI is used regardless, so the annotation silently does nothing.
fn check_ffi_conventions(stmts: &[Stmt], out: &mut Vec<Diagnostic>) {
    let honored = cfg!(target_os = "windows");
    for stmt in stmts {
        let StmtKind::NativeImport { functions, .. } = &stmt.kind else {
            continue;
        };
        for sig in functions {
            let Some(conv) = sig.call_conv.as_deref() else {
                continue;
            };
            if honored || !matches!(conv, "stdcall" | "fastcall") {
                continue;
            }
            out.push(
                Diagnostic::error(
                    "W0630",
                    format!("calling convention `{conv}` has no effect on this target"),
                    sig.span,
                )
                .into_warning()
                .label("declared here but ignored")
                .note("`stdcall` and `fastcall` only apply to 32-bit Windows; every other target uses its platform ABI")
                .help("remove the annotation, or use `@cdecl` for the portable default"),
            );
        }
    }
}

/// An import binding introduced at the top level, paired with where it was
/// written so an unused one can be pointed at precisely.
struct Import {
    name: String,
    span: Span,
}

/// Reports top-level imports whose bound name is never referenced anywhere in
/// the program. A glob import (`from m import *`) binds names the pass cannot
/// see, so modules carrying one are left alone. Underscore-prefixed bindings
/// are treated as deliberately unused.
fn check_unused_imports(
    program: &Program,
    referenced: &FxHashSet<String>,
    out: &mut Vec<Diagnostic>,
) {
    let mut imports = Vec::new();
    let mut has_glob = false;
    for stmt in &program.stmts {
        collect_imports(stmt, &mut imports, &mut has_glob);
    }
    if imports.is_empty() {
        return;
    }

    for imp in imports {
        if imp.name.starts_with('_') {
            continue;
        }
        if has_glob || referenced.contains(&imp.name) {
            continue;
        }
        out.push(
            Diagnostic::error("W0610", format!("unused import `{}`", imp.name), imp.span)
                .into_warning()
                .label("imported here but never used")
                .note("the binding is brought into scope but nothing references it")
                .help(format!(
                    "remove the import, or prefix it with `_` to keep it deliberately: `_{}`",
                    imp.name
                )),
        );
    }
}

fn collect_imports(stmt: &Stmt, out: &mut Vec<Import>, has_glob: &mut bool) {
    match &stmt.kind {
        StmtKind::Import { module, alias } => {
            let name = alias
                .clone()
                .unwrap_or_else(|| module.last().cloned().unwrap_or_default());
            out.push(Import {
                name,
                span: stmt.span,
            });
        }
        StmtKind::NativeImport { alias, .. } | StmtKind::PyImport { alias, .. } => {
            out.push(Import {
                name: alias.clone(),
                span: stmt.span,
            });
        }
        StmtKind::FromImport { names, is_star, .. } => {
            if *is_star {
                *has_glob = true;
                return;
            }
            for (name, alias) in names {
                let bound = alias.clone().unwrap_or_else(|| name.clone());
                out.push(Import {
                    name: bound,
                    span: stmt.span,
                });
            }
        }
        _ => {}
    }
}

/// Flags statements that follow an unconditional `return`, `break`, or
/// `continue` in the same block. Only these three divergent forms are
/// considered, which keeps the lint free of false positives. Recurses into
/// nested blocks first so dead code inside a loop or branch is caught too.
fn check_unreachable(stmts: &[Stmt], out: &mut Vec<Diagnostic>) {
    for stmt in stmts {
        for block in child_blocks(stmt) {
            check_unreachable(block, out);
        }
    }

    for (i, stmt) in stmts.iter().enumerate() {
        if diverges(&stmt.kind) && i + 1 < stmts.len() {
            let dead = &stmts[i + 1];
            out.push(
                Diagnostic::error("W0620", "unreachable statement", dead.span)
                    .into_warning()
                    .label("this code can never run")
                    .secondary(stmt.span, "any code after this point is unreachable")
                    .help("remove the dead code, or restructure the control flow above it"),
            );
            break;
        }
    }
}

fn diverges(kind: &StmtKind) -> bool {
    matches!(
        kind,
        StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue
    )
}

/// The nested statement lists owned directly by a statement. Used to drive
/// recursive lints without each one re-deriving the shape of every control
/// construct.
fn child_blocks(stmt: &Stmt) -> Vec<&[Stmt]> {
    let mut blocks: Vec<&[Stmt]> = Vec::new();
    match &stmt.kind {
        StmtKind::Fn { body, .. }
        | StmtKind::Struct { body, .. }
        | StmtKind::Impl { body, .. }
        | StmtKind::Enum { body, .. }
        | StmtKind::UnsafeBlock(body)
        | StmtKind::With { body, .. } => blocks.push(body),
        StmtKind::Trait { methods, .. } => blocks.push(methods),
        StmtKind::If {
            then_body,
            elif_clauses,
            else_body,
            ..
        } => {
            blocks.push(then_body);
            for (_, body) in elif_clauses {
                blocks.push(body);
            }
            if let Some(body) = else_body {
                blocks.push(body);
            }
        }
        StmtKind::While {
            body, else_body, ..
        }
        | StmtKind::For {
            body, else_body, ..
        } => {
            blocks.push(body);
            if let Some(body) = else_body {
                blocks.push(body);
            }
        }
        _ => {}
    }
    blocks
}

/// Records every name the program references, so an unused binding is exactly a
/// name that never appears here. Covers value positions (identifiers, the base
/// of an attribute access) and type positions (annotations, generics), since an
/// import can be used solely as a type.
fn collect_refs(stmts: &[Stmt], out: &mut FxHashSet<String>) {
    for stmt in stmts {
        collect_refs_stmt(stmt, out);
    }
}

fn collect_refs_stmt(stmt: &Stmt, out: &mut FxHashSet<String>) {
    match &stmt.kind {
        StmtKind::Let {
            type_ann, value, ..
        }
        | StmtKind::Const {
            type_ann, value, ..
        }
        | StmtKind::MultiLet {
            type_ann, value, ..
        }
        | StmtKind::MultiConst {
            type_ann, value, ..
        } => {
            if let Some(t) = type_ann {
                collect_refs_type(t, out);
            }
            collect_refs_expr(value, out);
        }
        StmtKind::Assign { target, value } => {
            collect_refs_expr(target, out);
            collect_refs_expr(value, out);
        }
        StmtKind::AugAssign { target, value, .. } => {
            collect_refs_expr(target, out);
            collect_refs_expr(value, out);
        }
        StmtKind::Fn {
            params,
            return_type,
            body,
            ..
        } => {
            for p in params {
                if let Some(t) = &p.type_ann {
                    collect_refs_type(t, out);
                }
                if let Some(d) = &p.default {
                    collect_refs_expr(d, out);
                }
            }
            if let Some(t) = return_type {
                collect_refs_type(t, out);
            }
            collect_refs(body, out);
        }
        StmtKind::Struct { fields, body, .. } => {
            for f in fields {
                if let Some(t) = &f.type_ann {
                    collect_refs_type(t, out);
                }
            }
            collect_refs(body, out);
        }
        StmtKind::Impl {
            trait_name,
            type_name,
            body,
            ..
        } => {
            if let Some(t) = trait_name {
                collect_refs_type(t, out);
            }
            collect_refs_type(type_name, out);
            collect_refs(body, out);
        }
        StmtKind::Trait { methods, .. } => collect_refs(methods, out),
        StmtKind::Enum { variants, body, .. } => {
            for v in variants {
                for t in &v.types {
                    collect_refs_type(t, out);
                }
                if let Some(e) = &v.value {
                    collect_refs_expr(e, out);
                }
            }
            collect_refs(body, out);
        }
        StmtKind::If {
            condition,
            then_body,
            elif_clauses,
            else_body,
        } => {
            collect_refs_expr(condition, out);
            collect_refs(then_body, out);
            for (cond, body) in elif_clauses {
                collect_refs_expr(cond, out);
                collect_refs(body, out);
            }
            if let Some(body) = else_body {
                collect_refs(body, out);
            }
        }
        StmtKind::While {
            condition,
            body,
            else_body,
        } => {
            collect_refs_expr(condition, out);
            collect_refs(body, out);
            if let Some(body) = else_body {
                collect_refs(body, out);
            }
        }
        StmtKind::For {
            iter,
            body,
            else_body,
            ..
        } => {
            collect_refs_expr(iter, out);
            collect_refs(body, out);
            if let Some(body) = else_body {
                collect_refs(body, out);
            }
        }
        StmtKind::With { items, body } => {
            for WithItem {
                context_expr,
                alias,
            } in items
            {
                collect_refs_expr(context_expr, out);
                if let Some(a) = alias {
                    collect_refs_expr(a, out);
                }
            }
            collect_refs(body, out);
        }
        StmtKind::Return(expr) => {
            if let Some(e) = expr {
                collect_refs_expr(e, out);
            }
        }
        StmtKind::Assert { test, msg } => {
            collect_refs_expr(test, out);
            if let Some(m) = msg {
                collect_refs_expr(m, out);
            }
        }
        StmtKind::ExprStmt(expr) | StmtKind::Defer(expr) => collect_refs_expr(expr, out),
        StmtKind::UnsafeBlock(body) => collect_refs(body, out),
        StmtKind::Import { .. }
        | StmtKind::NativeImport { .. }
        | StmtKind::FromImport { .. }
        | StmtKind::PyImport { .. }
        | StmtKind::Pass
        | StmtKind::Break
        | StmtKind::Continue => {}
    }
}

fn collect_refs_expr(expr: &Expr, out: &mut FxHashSet<String>) {
    match &expr.kind {
        ExprKind::Identifier(name) => {
            out.insert(name.clone());
        }
        ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Null => {}
        ExprKind::FStr(parts) => {
            for e in parts {
                collect_refs_expr(e, out);
            }
        }
        ExprKind::BinOp { left, right, .. } => {
            collect_refs_expr(left, out);
            collect_refs_expr(right, out);
        }
        ExprKind::Range { start, end, .. } => {
            collect_refs_expr(start, out);
            collect_refs_expr(end, out);
        }
        ExprKind::UnaryOp { operand, .. } => collect_refs_expr(operand, out),
        ExprKind::Cast(inner, ty) => {
            collect_refs_expr(inner, out);
            collect_refs_type(ty, out);
        }
        ExprKind::Call { callee, args } => {
            collect_refs_expr(callee, out);
            for arg in args {
                match arg {
                    CallArg::Positional(e)
                    | CallArg::Keyword(_, e)
                    | CallArg::Splat(e)
                    | CallArg::KwSplat(e) => collect_refs_expr(e, out),
                }
            }
        }
        ExprKind::Index { obj, index } => {
            collect_refs_expr(obj, out);
            collect_refs_expr(index, out);
        }
        ExprKind::Attr { obj, .. } => collect_refs_expr(obj, out),
        ExprKind::List(elems) | ExprKind::Tuple(elems) | ExprKind::Set(elems) => {
            for e in elems {
                collect_refs_expr(e, out);
            }
        }
        ExprKind::Dict(pairs) => {
            for (k, v) in pairs {
                collect_refs_expr(k, out);
                collect_refs_expr(v, out);
            }
        }
        ExprKind::ListComp { elt, clauses } | ExprKind::SetComp { elt, clauses } => {
            collect_refs_expr(elt, out);
            collect_refs_clauses(clauses, out);
        }
        ExprKind::DictComp {
            key,
            value,
            clauses,
        } => {
            collect_refs_expr(key, out);
            collect_refs_expr(value, out);
            collect_refs_clauses(clauses, out);
        }
        ExprKind::Borrow(inner)
        | ExprKind::MutBorrow(inner)
        | ExprKind::Deref(inner)
        | ExprKind::Try(inner)
        | ExprKind::Await(inner) => collect_refs_expr(inner, out),
        ExprKind::Slice { start, stop, step } => {
            for e in [start, stop, step].into_iter().flatten() {
                collect_refs_expr(e, out);
            }
        }
        ExprKind::AsyncBlock(body) => collect_refs(body, out),
        ExprKind::Match { expr, cases } => {
            collect_refs_expr(expr, out);
            for case in cases {
                collect_refs_pattern(&case.pattern, out);
                collect_refs(&case.body, out);
            }
        }
    }
}

fn collect_refs_clauses(clauses: &[CompClause], out: &mut FxHashSet<String>) {
    for clause in clauses {
        collect_refs_expr(&clause.iter, out);
        if let Some(cond) = &clause.condition {
            collect_refs_expr(cond, out);
        }
    }
}

fn collect_refs_pattern(pattern: &MatchPattern, out: &mut FxHashSet<String>) {
    match pattern {
        MatchPattern::Variant(name, inner) => {
            out.insert(name.clone());
            for p in inner {
                collect_refs_pattern(p, out);
            }
        }
        MatchPattern::Literal(expr) => collect_refs_expr(expr, out),
        MatchPattern::Identifier(..) | MatchPattern::Wildcard => {}
    }
}

fn collect_refs_type(ty: &TypeExpr, out: &mut FxHashSet<String>) {
    match &ty.kind {
        TypeExprKind::Name(name) => {
            out.insert(name.clone());
        }
        TypeExprKind::Qualified(segments) => {
            if let Some(first) = segments.first() {
                out.insert(first.clone());
            }
        }
        TypeExprKind::Generic(name, args) => {
            out.insert(name.clone());
            for a in args {
                collect_refs_type(a, out);
            }
        }
        TypeExprKind::Tuple(items) => {
            for t in items {
                collect_refs_type(t, out);
            }
        }
        TypeExprKind::List(inner)
        | TypeExprKind::Ref(inner)
        | TypeExprKind::MutRef(inner)
        | TypeExprKind::Ptr(inner)
        | TypeExprKind::FixedArray(inner, _) => collect_refs_type(inner, out),
        TypeExprKind::Dict(k, v) | TypeExprKind::Union(k, v) => {
            collect_refs_type(k, out);
            collect_refs_type(v, out);
        }
        TypeExprKind::Fn { params, ret } => {
            for p in params {
                collect_refs_type(p, out);
            }
            collect_refs_type(ret, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn lint(src: &str) -> Vec<Diagnostic> {
        let tokens = Lexer::new(src, 0).tokenise().expect("lex");
        let program = Parser::new(tokens).parse_program().expect("parse");
        lint_program(&program)
    }

    fn codes(diags: &[Diagnostic]) -> Vec<&str> {
        diags.iter().filter_map(|d| d.code()).collect()
    }

    #[test]
    fn unused_import_flagged() {
        let d = lint("import os\nprint(1)\n");
        assert!(codes(&d).contains(&"W0610"));
    }

    #[test]
    fn used_import_not_flagged() {
        let d = lint("import os\nprint(os.getpid())\n");
        assert!(!codes(&d).contains(&"W0610"));
    }

    #[test]
    fn import_used_only_as_type_not_flagged() {
        let d = lint("from typing import Vec\nlet x: Vec = make()\n");
        assert!(!codes(&d).contains(&"W0610"));
    }

    #[test]
    fn underscore_import_not_flagged() {
        let d = lint("import os as _os\nprint(1)\n");
        assert!(!codes(&d).contains(&"W0610"));
    }

    #[test]
    fn glob_import_silences_unused() {
        let d = lint("from os import *\nimport sys\nprint(1)\n");
        assert!(!codes(&d).contains(&"W0610"));
    }

    #[test]
    fn from_import_unused_flagged() {
        let d = lint("from os import getpid, getppid\nprint(getpid())\n");
        let unused: Vec<_> = d.iter().filter(|x| x.code() == Some("W0610")).collect();
        assert_eq!(unused.len(), 1);
    }

    #[test]
    fn unreachable_after_return_flagged() {
        let d = lint("fn f() -> i64:\n    return 1\n    let x = 2\n");
        assert!(codes(&d).contains(&"W0620"));
    }

    #[test]
    fn no_unreachable_when_return_is_last() {
        let d = lint("fn f() -> i64:\n    let x = 1\n    return x\n");
        assert!(!codes(&d).contains(&"W0620"));
    }

    #[test]
    fn unreachable_after_break_in_loop() {
        let d = lint("while true:\n    break\n    print(1)\n");
        assert!(codes(&d).contains(&"W0620"));
    }

    #[test]
    fn code_in_other_branch_is_reachable() {
        let d = lint("fn f() -> i64:\n    if cond():\n        return 1\n    return 2\n");
        assert!(!codes(&d).contains(&"W0620"));
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn stdcall_flagged_on_non_windows() {
        let d = lint(
            "import \"/usr/lib/libc.so.6\" as libc:\n    @stdcall\n    fn puts(s: str) -> i64\n",
        );
        assert!(codes(&d).contains(&"W0630"));
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn cdecl_not_flagged() {
        let d = lint(
            "import \"/usr/lib/libc.so.6\" as libc:\n    @cdecl\n    fn puts(s: str) -> i64\n",
        );
        assert!(!codes(&d).contains(&"W0630"));
    }

    #[test]
    fn private_unused_fn_flagged() {
        let d = lint("fn _helper() -> i64:\n    return 1\n\nfn main():\n    print(2)\n");
        assert!(codes(&d).contains(&"W0650"));
    }

    #[test]
    fn private_used_fn_not_flagged() {
        let d = lint("fn _helper() -> i64:\n    return 1\n\nfn main():\n    print(_helper())\n");
        assert!(!codes(&d).contains(&"W0650"));
    }

    #[test]
    fn public_unused_fn_not_flagged() {
        let d = lint("fn helper() -> i64:\n    return 1\n\nfn main():\n    print(2)\n");
        assert!(!codes(&d).contains(&"W0650"));
    }

    #[test]
    fn only_first_unreachable_reported_per_block() {
        let d = lint("fn f() -> i64:\n    return 1\n    let a = 2\n    let b = 3\n");
        let count = d.iter().filter(|x| x.code() == Some("W0620")).count();
        assert_eq!(count, 1);
    }
}
