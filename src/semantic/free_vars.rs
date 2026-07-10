use crate::parser::ast::{
    CallArg, CompClause, Expr, ExprKind, ForTarget, MatchPattern, Param, Stmt, StmtKind,
};
use rustc_hash::FxHashSet as HashSet;

/// Free vars of a nested fn: identifiers used but not bound within it.
/// Over-approximates builtins/globals; caller intersects with enclosing locals.
/// First-use order keeps capture lowering deterministic.
pub fn free_variables(params: &[Param], body: &[Stmt]) -> Vec<String> {
    let mut fv = FreeVars::default();
    fv.scopes.push(HashSet::default());
    for p in params {
        fv.bind(&p.name);
    }
    fv.block(body);
    fv.free
}

#[derive(Default)]
struct FreeVars {
    scopes: Vec<HashSet<String>>,
    free: Vec<String>,
    seen: HashSet<String>,
}

impl FreeVars {
    fn bind(&mut self, name: &str) {
        self.scopes.last_mut().unwrap().insert(name.to_string());
    }

    fn is_bound(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains(name))
    }

    fn use_name(&mut self, name: &str) {
        if !self.is_bound(name) && self.seen.insert(name.to_string()) {
            self.free.push(name.to_string());
        }
    }

    fn block(&mut self, stmts: &[Stmt]) {
        self.scopes.push(HashSet::default());
        self.hoist(stmts);
        for s in stmts {
            self.stmt(s);
        }
        self.scopes.pop();
    }

    /// Hoist fn/struct/enum names before walking, matching the resolver.
    fn hoist(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            match &s.kind {
                StmtKind::Fn { name, .. }
                | StmtKind::Struct { name, .. }
                | StmtKind::Enum { name, .. } => self.bind(name),
                _ => {}
            }
        }
    }

    fn for_target(&mut self, target: &ForTarget) {
        match target {
            ForTarget::Name(n, _) => self.bind(n),
            ForTarget::Tuple(names) => {
                for (n, _) in names {
                    self.bind(n);
                }
            }
        }
    }

    fn pattern(&mut self, pat: &MatchPattern) {
        match pat {
            MatchPattern::Identifier(n, _) => self.bind(n),
            MatchPattern::Variant(_, sub) => {
                for p in sub {
                    self.pattern(p);
                }
            }
            MatchPattern::Literal(e) => self.expr(e),
            MatchPattern::Wildcard => {}
        }
    }

    fn comp_clauses(&mut self, clauses: &[CompClause]) {
        for c in clauses {
            self.expr(&c.iter);
            self.for_target(&c.target);
            if let Some(cond) = &c.condition {
                self.expr(cond);
            }
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Fn { params, body, .. } => {
                // Nested fn opens its own scope; outer names it reads stay free for the parent.
                self.scopes.push(HashSet::default());
                for p in params {
                    self.bind(&p.name);
                }
                self.block(body);
                self.scopes.pop();
            }
            StmtKind::Let { name, value, .. } | StmtKind::Const { name, value, .. } => {
                self.expr(value);
                self.bind(name);
            }
            StmtKind::MultiLet { names, value, .. } | StmtKind::MultiConst { names, value, .. } => {
                self.expr(value);
                for n in names {
                    self.bind(n);
                }
            }
            StmtKind::Assign { target, value } => {
                self.expr(value);
                self.expr(target);
            }
            StmtKind::AugAssign { target, value, .. } => {
                self.expr(value);
                self.expr(target);
            }
            StmtKind::If {
                condition,
                then_body,
                elif_clauses,
                else_body,
            } => {
                self.expr(condition);
                self.block(then_body);
                for (c, b) in elif_clauses {
                    self.expr(c);
                    self.block(b);
                }
                if let Some(b) = else_body {
                    self.block(b);
                }
            }
            StmtKind::While {
                condition,
                body,
                else_body,
            } => {
                self.expr(condition);
                self.block(body);
                if let Some(b) = else_body {
                    self.block(b);
                }
            }
            StmtKind::For {
                target,
                iter,
                body,
                else_body,
            } => {
                self.expr(iter);
                self.scopes.push(HashSet::default());
                self.for_target(target);
                self.block(body);
                self.scopes.pop();
                if let Some(b) = else_body {
                    self.block(b);
                }
            }
            StmtKind::With { items, body } => {
                self.scopes.push(HashSet::default());
                for item in items {
                    self.expr(&item.context_expr);
                    if let Some(alias) = &item.alias
                        && let ExprKind::Identifier(n) = &alias.kind
                    {
                        self.bind(n);
                    }
                }
                self.block(body);
                self.scopes.pop();
            }
            StmtKind::Return(Some(e)) | StmtKind::ExprStmt(e) | StmtKind::Defer(e) => self.expr(e),
            StmtKind::Return(None) | StmtKind::Pass | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Assert { test, msg } => {
                self.expr(test);
                if let Some(m) = msg {
                    self.expr(m);
                }
            }
            StmtKind::UnsafeBlock(body) => self.block(body),
            StmtKind::Struct { body, .. } | StmtKind::Impl { body, .. } => self.block(body),
            StmtKind::Enum { body, .. } => self.block(body),
            StmtKind::Trait { methods, .. } => self.block(methods),
            StmtKind::Import { .. }
            | StmtKind::NativeImport { .. }
            | StmtKind::FromImport { .. }
            | StmtKind::PyImport { .. }
            | StmtKind::TypeAlias { .. } => {}
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Identifier(name) => self.use_name(name),
            ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Null => {}
            ExprKind::FStr(parts) => {
                for p in parts {
                    self.expr(&p.expr);
                }
            }
            ExprKind::BinOp { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            ExprKind::UnaryOp { operand, .. } => self.expr(operand),
            ExprKind::Cast(e, _) => self.expr(e),
            ExprKind::Call { callee, args } => {
                self.expr(callee);
                for a in args {
                    match a {
                        CallArg::Positional(e)
                        | CallArg::Keyword(_, e)
                        | CallArg::Splat(e)
                        | CallArg::KwSplat(e) => self.expr(e),
                    }
                }
            }
            ExprKind::Index { obj, index } => {
                self.expr(obj);
                self.expr(index);
            }
            ExprKind::Attr { obj, .. } | ExprKind::OptAttr { obj, .. } => self.expr(obj),
            ExprKind::List(es) | ExprKind::Tuple(es) | ExprKind::Set(es) => {
                for e in es {
                    self.expr(e);
                }
            }
            ExprKind::Dict(pairs) => {
                for (k, v) in pairs {
                    self.expr(k);
                    self.expr(v);
                }
            }
            ExprKind::ListComp { elt, clauses } | ExprKind::SetComp { elt, clauses } => {
                self.scopes.push(HashSet::default());
                self.comp_clauses(clauses);
                self.expr(elt);
                self.scopes.pop();
            }
            ExprKind::DictComp {
                key,
                value,
                clauses,
            } => {
                self.scopes.push(HashSet::default());
                self.comp_clauses(clauses);
                self.expr(key);
                self.expr(value);
                self.scopes.pop();
            }
            ExprKind::Borrow(e)
            | ExprKind::MutBorrow(e)
            | ExprKind::Deref(e)
            | ExprKind::Try(e)
            | ExprKind::Await(e) => self.expr(e),
            ExprKind::AsyncBlock(body) => self.block(body),
            ExprKind::Slice { start, stop, step } => {
                for e in [start, stop, step].into_iter().flatten() {
                    self.expr(e);
                }
            }
            ExprKind::Range { start, end, .. } => {
                self.expr(start);
                self.expr(end);
            }
            ExprKind::Match { expr, cases } => {
                self.expr(expr);
                for case in cases {
                    self.scopes.push(HashSet::default());
                    self.pattern(&case.pattern);
                    if let Some(g) = &case.guard {
                        self.expr(g);
                    }
                    self.block(&case.body);
                    self.scopes.pop();
                }
            }
            ExprKind::Ternary {
                cond,
                then,
                otherwise,
            } => {
                self.expr(cond);
                self.expr(then);
                self.expr(otherwise);
            }
            ExprKind::Lambda { params, body } => {
                self.scopes
                    .push(params.iter().map(|p| p.name.clone()).collect());
                for p in params {
                    if let Some(d) = &p.default {
                        self.expr(d);
                    }
                }
                self.expr(body);
                self.scopes.pop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    /// Free vars of the first nested fn in the first top-level fn of `src`.
    fn nested_free(src: &str) -> Vec<String> {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        for stmt in &prog.stmts {
            if let StmtKind::Fn { body, .. } = &stmt.kind {
                for inner in body {
                    if let StmtKind::Fn { params, body, .. } = &inner.kind {
                        return free_variables(params, body);
                    }
                }
            }
        }
        panic!("no nested fn found");
    }

    #[test]
    fn captures_scalar() {
        assert_eq!(
            nested_free("fn o():\n    let n = 1\n    fn i() -> i64:\n        return n\n"),
            vec!["n"]
        );
    }

    #[test]
    fn own_param_not_free() {
        assert_eq!(
            nested_free("fn o():\n    fn i(x: i64) -> i64:\n        return x\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn own_let_shadows_capture() {
        assert_eq!(
            nested_free(
                "fn o():\n    let n = 1\n    fn i() -> i64:\n        let n = 2\n        return n\n"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn builtin_is_free_but_filtered_by_caller() {
        // `print` and `n` both surface; the MIR builder keeps only real locals.
        assert_eq!(
            nested_free("fn o():\n    let n = 1\n    fn i():\n        print(n)\n"),
            vec!["print", "n"]
        );
    }

    #[test]
    fn first_use_order_preserved() {
        assert_eq!(
            nested_free(
                "fn o():\n    let a = 1\n    let b = 2\n    fn i() -> i64:\n        return b + a\n"
            ),
            vec!["b", "a"]
        );
    }

    #[test]
    fn loop_var_in_nested_fn_body_is_local() {
        assert_eq!(
            nested_free(
                "fn o():\n    fn i() -> i64:\n        let mut s = 0\n        for k in [1, 2]:\n            s = s + k\n        return s\n"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn captures_across_inner_block() {
        assert_eq!(
            nested_free(
                "fn o():\n    let n = 1\n    fn i() -> i64:\n        if True:\n            return n\n        return 0\n"
            ),
            vec!["n"]
        );
    }
}
