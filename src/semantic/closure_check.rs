use super::error::SemanticError;
use super::free_vars::free_variables;
use crate::compile::errors::Diagnostic;
use crate::parser::ast::{
    CallArg, CompClause, Expr, ExprKind, ForTarget, Param, Program, Stmt, StmtKind,
};
use crate::span::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Validates capturing nested fns: callable only where captures are in scope,
/// never usable as a bare value. Precise diagnostics instead of a segfault.
pub fn check_closures(program: &Program) -> Vec<SemanticError> {
    let mut c = Checker::default();
    for stmt in &program.stmts {
        c.top_stmt(stmt);
    }
    c.errors
}

#[derive(Default)]
struct Frame {
    /// Parameters and value bindings declared directly in this function body.
    locals: HashSet<String>,
    /// Names captured from an enclosing fn, available as synthesized params.
    captured: HashSet<String>,
    /// Capturing fns in this body with their capture names, for call-site checks.
    closures: HashMap<String, Vec<String>>,
}

#[derive(Default)]
struct Checker {
    frames: Vec<Frame>,
    errors: Vec<SemanticError>,
}

impl Checker {
    fn top_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Fn {
                params,
                body,
                type_params,
                ..
            } => {
                if type_params.is_empty() {
                    self.enter_function(params, body);
                }
            }
            StmtKind::Impl { body, .. } | StmtKind::Trait { methods: body, .. } => {
                for m in body {
                    self.top_stmt(m);
                }
            }
            _ => {}
        }
    }

    fn enter_function(&mut self, params: &[Param], body: &[Stmt]) {
        let mut locals: HashSet<String> = params.iter().map(|p| p.name.clone()).collect();
        collect_bindings(body, &mut locals);

        // Captures resolved against enclosing frames before this one is pushed.
        let captured: HashSet<String> = free_variables(params, body)
            .into_iter()
            .filter(|n| self.outer_has_local(n))
            .collect();

        self.frames.push(Frame {
            locals,
            captured,
            closures: HashMap::default(),
        });

        let mut defs = Vec::new();
        collect_fn_defs(body, &mut defs);
        for def in defs {
            if let StmtKind::Fn {
                name,
                params,
                body,
                type_params,
                ..
            } = &def.kind
            {
                if !type_params.is_empty() {
                    continue;
                }
                let caps: Vec<String> = free_variables(params, body)
                    .into_iter()
                    .filter(|n| self.any_frame_has_local(n))
                    .collect();
                if !caps.is_empty() {
                    self.frames
                        .last_mut()
                        .unwrap()
                        .closures
                        .insert(name.clone(), caps);
                }
            }
        }

        self.block(body);
        self.frames.pop();
    }

    fn outer_has_local(&self, name: &str) -> bool {
        self.frames.iter().any(|f| f.locals.contains(name))
    }

    fn any_frame_has_local(&self, name: &str) -> bool {
        self.frames.iter().any(|f| f.locals.contains(name))
    }

    /// A capture is reachable when it is one of this fn's locals or its own captures.
    fn accessible(&self, name: &str) -> bool {
        let f = self.frames.last().unwrap();
        f.locals.contains(name) || f.captured.contains(name)
    }

    fn find_closure(&self, name: &str) -> Option<Vec<String>> {
        self.frames
            .iter()
            .rev()
            .find_map(|f| f.closures.get(name).cloned())
    }

    fn use_ident(&mut self, name: &str, span: Span, is_call: bool) {
        let Some(caps) = self.find_closure(name) else {
            return;
        };
        if !is_call {
            self.errors.push(SemanticError::rich(
                Diagnostic::error(
                    "E0423",
                    format!("capturing closure `{name}` cannot be used as a value"),
                    span,
                )
                .label("used as a value here")
                .note(format!(
                    "`{name}` reads variables from its enclosing function, so it has no \
                     standalone value to pass around or return"
                ))
                .help(
                    "call it directly, or rewrite it as a top-level function that takes the \
                     captured values as parameters",
                ),
            ));
            return;
        }
        if let Some(missing) = caps.iter().find(|c| !self.accessible(c)) {
            self.errors.push(SemanticError::rich(
                Diagnostic::error(
                    "E0424",
                    format!("capturing closure `{name}` is called outside its defining scope"),
                    span,
                )
                .label("called here")
                .note(format!(
                    "it captures `{missing}`, which is not in scope at this call"
                ))
                .help(
                    "call it from the function that defines it, or pass the captured values as \
                     explicit parameters",
                ),
            ));
        }
    }

    fn block(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Fn {
                params,
                body,
                type_params,
                ..
            } => {
                if type_params.is_empty() {
                    self.enter_function(params, body);
                }
            }
            StmtKind::Let { value, .. } | StmtKind::Const { value, .. } => self.expr(value),
            StmtKind::MultiLet { value, .. } | StmtKind::MultiConst { value, .. } => {
                self.expr(value)
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
                iter,
                body,
                else_body,
                ..
            } => {
                self.expr(iter);
                self.block(body);
                if let Some(b) = else_body {
                    self.block(b);
                }
            }
            StmtKind::With { items, body } => {
                for item in items {
                    self.expr(&item.context_expr);
                }
                self.block(body);
            }
            StmtKind::UnsafeBlock(body) => self.block(body),
            StmtKind::Return(Some(e)) | StmtKind::ExprStmt(e) | StmtKind::Defer(e) => self.expr(e),
            StmtKind::Assert { test, msg } => {
                self.expr(test);
                if let Some(m) = msg {
                    self.expr(m);
                }
            }
            _ => {}
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Identifier(name) => self.use_ident(name, expr.span, false),
            ExprKind::Call { callee, args } => {
                if let ExprKind::Identifier(name) = &callee.kind {
                    self.use_ident(name, callee.span, true);
                } else {
                    self.expr(callee);
                }
                for a in args {
                    match a {
                        CallArg::Positional(e)
                        | CallArg::Keyword(_, e)
                        | CallArg::Splat(e)
                        | CallArg::KwSplat(e) => self.expr(e),
                    }
                }
            }
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
            ExprKind::Cast(e, _)
            | ExprKind::Borrow(e)
            | ExprKind::MutBorrow(e)
            | ExprKind::Deref(e)
            | ExprKind::Try(e)
            | ExprKind::Await(e) => self.expr(e),
            ExprKind::Index { obj, index } => {
                self.expr(obj);
                self.expr(index);
            }
            ExprKind::Attr { obj, .. } => self.expr(obj),
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
                self.comp_clauses(clauses);
                self.expr(elt);
            }
            ExprKind::DictComp {
                key,
                value,
                clauses,
            } => {
                self.comp_clauses(clauses);
                self.expr(key);
                self.expr(value);
            }
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
                    if let Some(g) = &case.guard {
                        self.expr(g);
                    }
                    self.block(&case.body);
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
            ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Null => {}
        }
    }

    fn comp_clauses(&mut self, clauses: &[CompClause]) {
        for c in clauses {
            self.expr(&c.iter);
            if let Some(cond) = &c.condition {
                self.expr(cond);
            }
        }
    }
}

/// Value bindings in `stmts`, not descending into nested fn bodies.
fn collect_bindings(stmts: &[Stmt], out: &mut HashSet<String>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Let { name, .. } | StmtKind::Const { name, .. } => {
                out.insert(name.clone());
            }
            StmtKind::MultiLet { names, .. } | StmtKind::MultiConst { names, .. } => {
                out.extend(names.iter().cloned());
            }
            StmtKind::If {
                then_body,
                elif_clauses,
                else_body,
                ..
            } => {
                collect_bindings(then_body, out);
                for (_, b) in elif_clauses {
                    collect_bindings(b, out);
                }
                if let Some(b) = else_body {
                    collect_bindings(b, out);
                }
            }
            StmtKind::While {
                body, else_body, ..
            }
            | StmtKind::For {
                body, else_body, ..
            } => {
                if let StmtKind::For { target, .. } = &stmt.kind {
                    bind_for_target(target, out);
                }
                collect_bindings(body, out);
                if let Some(b) = else_body {
                    collect_bindings(b, out);
                }
            }
            StmtKind::With { items, body } => {
                for item in items {
                    if let Some(alias) = &item.alias
                        && let ExprKind::Identifier(n) = &alias.kind
                    {
                        out.insert(n.clone());
                    }
                }
                collect_bindings(body, out);
            }
            StmtKind::UnsafeBlock(body) => collect_bindings(body, out),
            _ => {}
        }
    }
}

fn bind_for_target(target: &ForTarget, out: &mut HashSet<String>) {
    match target {
        ForTarget::Name(n, _) => {
            out.insert(n.clone());
        }
        ForTarget::Tuple(names) => {
            for (n, _) in names {
                out.insert(n.clone());
            }
        }
    }
}

/// Functions in `stmts`, descending control-flow but not fn bodies.
fn collect_fn_defs<'s>(stmts: &'s [Stmt], out: &mut Vec<&'s Stmt>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Fn { .. } => out.push(stmt),
            StmtKind::If {
                then_body,
                elif_clauses,
                else_body,
                ..
            } => {
                collect_fn_defs(then_body, out);
                for (_, b) in elif_clauses {
                    collect_fn_defs(b, out);
                }
                if let Some(b) = else_body {
                    collect_fn_defs(b, out);
                }
            }
            StmtKind::While {
                body, else_body, ..
            }
            | StmtKind::For {
                body, else_body, ..
            } => {
                collect_fn_defs(body, out);
                if let Some(b) = else_body {
                    collect_fn_defs(b, out);
                }
            }
            StmtKind::With { body, .. } | StmtKind::UnsafeBlock(body) => collect_fn_defs(body, out),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn codes(src: &str) -> Vec<String> {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        check_closures(&prog)
            .iter()
            .filter_map(|e| e.to_diagnostic().code().map(str::to_string))
            .collect()
    }

    #[test]
    fn direct_call_in_parent_is_ok() {
        assert!(codes("fn o() -> i64:\n    let n = 1\n    fn i() -> i64:\n        return n\n    return i()\n").is_empty());
    }

    #[test]
    fn non_capturing_value_use_is_ok() {
        // A nested function that captures nothing is a plain pointer and may be a value.
        assert!(codes("fn o():\n    fn i() -> i64:\n        return 1\n    let g = i\n").is_empty());
    }

    #[test]
    fn capturing_closure_as_value_errors() {
        assert_eq!(
            codes("fn o():\n    let n = 1\n    fn i() -> i64:\n        return n\n    let g = i\n"),
            vec!["E0423"]
        );
    }

    #[test]
    fn capturing_closure_returned_errors() {
        assert_eq!(
            codes(
                "fn o() -> fn() -> i64:\n    let n = 1\n    fn i() -> i64:\n        return n\n    return i\n"
            ),
            vec!["E0423"]
        );
    }

    #[test]
    fn capturing_closure_as_argument_errors() {
        assert_eq!(
            codes(
                "fn apply(f: fn(i64) -> i64, v: i64) -> i64:\n    return f(v)\nfn o() -> i64:\n    let n = 1\n    fn i(x: i64) -> i64:\n        return x + n\n    return apply(i, 5)\n"
            ),
            vec!["E0423"]
        );
    }

    #[test]
    fn sibling_cross_scope_call_errors() {
        assert_eq!(
            codes(
                "fn o() -> i64:\n    let p = 1\n    fn a() -> i64:\n        return p\n    fn b() -> i64:\n        return a()\n    return b()\n"
            ),
            vec!["E0424"]
        );
    }

    #[test]
    fn recursion_is_ok() {
        assert!(codes("fn o() -> i64:\n    let base = 0\n    fn down(n: i64) -> i64:\n        if n <= 0:\n            return base\n        return down(n - 1)\n    return down(3)\n").is_empty());
    }

    #[test]
    fn capturing_global_is_not_a_capture() {
        // Module globals are reachable directly, so reading one is not a capture.
        assert!(
            codes("let g = 5\nfn o():\n    fn i() -> i64:\n        return g\n    let h = i\n")
                .is_empty()
        );
    }
}
