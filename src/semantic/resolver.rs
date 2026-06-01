use super::error::SemanticError;
use super::symbol_table::{ScopeKind, Symbol, SymbolKind, SymbolTable};
use crate::parser::ast::{
    CallArg, CompClause, Expr, ExprKind, ForTarget, MatchPattern, Param, Program, Stmt, StmtKind,
};
use crate::span::Span;
use rustc_hash::FxHashMap as HashMap;

pub struct Resolver {
    pub table: SymbolTable,
    pub errors: Vec<SemanticError>,
    pub warnings: Vec<SemanticError>,
    /// Maps a bare variant name to the list of enum names that define it.
    /// When >1 entry, the variant is ambiguous without qualification.
    enum_variant_origins: HashMap<String, Vec<String>>,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver {
    pub fn new() -> Self {
        let mut table = SymbolTable::new();
        let builtin_fns = [
            "print",
            "str",
            "int",
            "type",
            "len",
            "max",
            "min",
            "sum",
            "list_new",
            "list",
            "dict",
            "slice",
            "ffi_errno",
            "bytes_new",
            "bytes_push",
            "bytes_push_u16_le",
            "bytes_push_u32_le",
            "realize",
            "Error",
            "Error::Error",
            "enumerate",
            "zip",
            "abs",
            "round",
            "input",
        ];
        for name in builtin_fns {
            table.define(Symbol {
                name: name.to_string(),
                kind: SymbolKind::Function,
                span: Span::default(),
                is_private: false,
                used: true,
            });
        }
        for ty_name in [
            "i64", "i32", "i16", "i8", "u64", "u32", "u16", "u8", "float", "f64", "f32", "bool",
        ] {
            table.define(Symbol {
                name: ty_name.to_string(),
                kind: SymbolKind::Function,
                span: Span::default(),
                is_private: false,
                used: true,
            });
        }
        Self {
            table,
            errors: Vec::new(),
            warnings: Vec::new(),
            enum_variant_origins: HashMap::default(),
        }
    }

    pub fn resolve_program(&mut self, program: &Program) {
        self.hoist_fns_and_structs(&program.stmts);
        for stmt in &program.stmts {
            self.resolve_stmt(stmt);
        }
    }

    fn hoist_fns_and_structs(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Fn { name, .. } => {
                    self.define_sym(name, SymbolKind::Function, stmt.span);
                }
                StmtKind::Struct { name, .. } => {
                    self.define_sym(name, SymbolKind::Struct, stmt.span);
                }
                StmtKind::TypeAlias { name, .. } => {
                    self.define_sym(name, SymbolKind::TypeAlias, stmt.span);
                }
                StmtKind::Impl {
                    type_name, body, ..
                } => {
                    for s in body {
                        if let StmtKind::Fn { name: fn_name, .. } = &s.kind {
                            let mangled = format!("{}::{}", type_name, fn_name);
                            self.define_sym(&mangled, SymbolKind::Function, s.span);
                        }
                        if let StmtKind::Const {
                            name: const_name, ..
                        } = &s.kind
                        {
                            let mangled = format!("{}::{}", type_name, const_name);
                            self.define_sym(&mangled, SymbolKind::Variable, s.span);
                        }
                    }
                }
                StmtKind::Trait { .. } => {}
                StmtKind::Enum { name, variants, .. } => {
                    self.define_sym(name, SymbolKind::Enum, stmt.span);
                    for variant in variants {
                        let mangled = format!("{}::{}", name, variant.name);
                        self.define_sym(&mangled, SymbolKind::Function, stmt.span);
                        // Track bare variant name origins for collision detection.
                        self.enum_variant_origins
                            .entry(variant.name.clone())
                            .or_default()
                            .push(name.clone());
                        self.define_sym(&variant.name, SymbolKind::Function, stmt.span);
                    }
                }
                _ => {}
            }
        }
    }

    /// Olive decorators are a fixed directive set, not arbitrary wrappers. An
    /// unrecognized name is silently dropped by codegen, so flag it rather than
    /// let it look effective.
    fn check_decorators(&mut self, decorators: &[crate::parser::ast::Decorator]) {
        const KNOWN: &[&str] = &["memo", "test", "safe"];
        for d in decorators {
            if !KNOWN.contains(&d.name.as_str()) {
                self.warnings.push(SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "W0660",
                        format!("unknown decorator `@{}`", d.name),
                        d.span,
                    )
                    .into_warning()
                    .label("not a recognized decorator")
                    .note("Olive decorators are `@memo`, `#[test]`, and `@safe`; this one has no effect"),
                ));
            }
        }
    }

    fn define_sym(&mut self, name: &str, kind: SymbolKind, span: Span) {
        let is_private = name.starts_with('_');
        let sym = Symbol {
            name: name.to_string(),
            kind,
            span,
            is_private,
            used: false,
        };
        self.table.define(sym);
    }

    /// Pops the innermost scope and turns any binding that was never read into
    /// an unused-variable warning. The structured fix prefixes the name with an
    /// underscore, the conventional way to keep a binding on purpose, but it is
    /// advisory: silently renaming a forgotten variable could hide a real bug,
    /// so the autofixer never applies it on its own.
    fn pop_scope(&mut self) {
        for sym in self.table.pop_unused() {
            let kind = match sym.kind {
                SymbolKind::Parameter => "parameter",
                SymbolKind::LoopVar => "loop variable",
                _ => "variable",
            };
            self.warnings.push(SemanticError::rich(
                crate::compile::errors::Diagnostic::error(
                    "W0640",
                    format!("unused {kind} `{}`", sym.name),
                    sym.span,
                )
                .into_warning()
                .label("bound here but never used")
                .suggestion(
                    sym.span,
                    format!("_{}", sym.name),
                    "remove it, or prefix with `_` to keep it deliberately",
                    crate::compile::errors::Applicability::MaybeIncorrect,
                ),
            ));
        }
    }

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let {
                name,
                name_span,
                value,
                ..
            } => {
                self.resolve_expr(value);
                self.define_sym(name, SymbolKind::Variable, *name_span);
            }

            StmtKind::MultiLet {
                names,
                name_spans,
                value,
                ..
            } => {
                self.resolve_expr(value);
                for (name, span) in names.iter().zip(name_spans) {
                    self.define_sym(name, SymbolKind::Variable, *span);
                }
            }

            StmtKind::Const {
                name,
                name_span,
                value,
                ..
            } => {
                self.resolve_expr(value);
                self.define_sym(name, SymbolKind::Variable, *name_span);
            }

            StmtKind::MultiConst {
                names,
                name_spans,
                value,
                ..
            } => {
                self.resolve_expr(value);
                for (name, span) in names.iter().zip(name_spans) {
                    self.define_sym(name, SymbolKind::Variable, *span);
                }
            }

            StmtKind::Assign { target, value } => {
                self.resolve_expr(value);
                self.resolve_assign_target(target);
            }

            StmtKind::AugAssign { target, value, .. } => {
                self.resolve_expr(value);
                self.resolve_assign_target(target);
            }

            StmtKind::Fn {
                name: _,
                type_params,
                params,
                body,
                decorators,
                ..
            } => {
                self.check_decorators(decorators);
                self.table.push(ScopeKind::Function);
                for tp in type_params {
                    self.define_sym(tp, SymbolKind::Variable, stmt.span);
                    self.table.mark_used_in_current(tp);
                }
                self.resolve_params(params);
                self.hoist_fns_and_structs(body);
                for s in body {
                    self.resolve_stmt(s);
                }
                self.pop_scope();
            }

            StmtKind::Struct {
                type_params, body, ..
            } => {
                self.table.push(ScopeKind::Struct);
                for tp in type_params {
                    self.define_sym(tp, SymbolKind::Variable, stmt.span);
                }
                for s in body {
                    self.resolve_stmt(s);
                }
                self.pop_scope();
            }

            StmtKind::Impl {
                type_params, body, ..
            } => {
                self.table.push(ScopeKind::Struct);
                for tp in type_params {
                    self.define_sym(tp, SymbolKind::Variable, stmt.span);
                }
                self.hoist_fns_and_structs(body);
                for s in body {
                    self.resolve_stmt(s);
                }
                self.pop_scope();
            }

            StmtKind::Trait { .. } => {}

            StmtKind::TypeAlias { .. } => {}

            StmtKind::If {
                condition,
                then_body,
                elif_clauses,
                else_body,
            } => {
                self.resolve_expr(condition);
                self.resolve_block(then_body);
                for (cond, body) in elif_clauses {
                    self.resolve_expr(cond);
                    self.resolve_block(body);
                }
                if let Some(body) = else_body {
                    self.resolve_block(body);
                }
            }

            StmtKind::While {
                condition,
                body,
                else_body,
            } => {
                self.resolve_expr(condition);
                self.resolve_block(body);
                if let Some(body) = else_body {
                    self.resolve_block(body);
                }
            }

            StmtKind::For {
                target,
                iter,
                body,
                else_body,
            } => {
                self.resolve_expr(iter);
                self.table.push(ScopeKind::Block);
                self.define_for_target(target);
                self.hoist_fns_and_structs(body);
                for s in body {
                    self.resolve_stmt(s);
                }
                self.pop_scope();
                if let Some(body) = else_body {
                    self.resolve_block(body);
                }
            }

            StmtKind::With { items, body } => {
                self.table.push(ScopeKind::Block);
                for item in items {
                    self.resolve_expr(&item.context_expr);
                    if let Some(alias_expr) = &item.alias
                        && let ExprKind::Identifier(name) = &alias_expr.kind
                    {
                        self.define_sym(name, SymbolKind::Variable, alias_expr.span);
                    }
                }
                self.hoist_fns_and_structs(body);
                for s in body {
                    self.resolve_stmt(s);
                }
                self.pop_scope();
            }

            StmtKind::Return(expr) => {
                if let Some(e) = expr {
                    self.resolve_expr(e);
                }
            }

            StmtKind::Assert { test, msg } => {
                self.resolve_expr(test);
                if let Some(m) = msg {
                    self.resolve_expr(m);
                }
            }

            StmtKind::Import { module, alias } => {
                let name = alias
                    .as_deref()
                    .unwrap_or_else(|| module.last().unwrap().as_str());
                self.define_sym(name, SymbolKind::Import, stmt.span);
            }

            StmtKind::NativeImport {
                alias,
                functions,
                structs,
                ..
            } => {
                self.define_sym(alias, SymbolKind::NativeImport, stmt.span);
                for sig in functions {
                    let mangled = format!("{}::{}", alias, sig.name);
                    self.define_sym(&mangled, SymbolKind::Function, stmt.span);
                }
                for s in structs {
                    let mangled = format!("{}::{}", alias, s.name);
                    self.define_sym(&mangled, SymbolKind::Struct, stmt.span);
                }
            }

            StmtKind::FromImport { names, .. } => {
                for (name, alias) in names {
                    if name.starts_with('_') {
                        self.errors.push(SemanticError::PrivateAccess {
                            name: name.clone(),
                            span: stmt.span,
                        });
                    } else {
                        let bound = alias.as_deref().unwrap_or(name.as_str());
                        self.define_sym(bound, SymbolKind::Import, stmt.span);
                    }
                }
            }

            StmtKind::PyImport { alias, .. } => {
                self.define_sym(alias, SymbolKind::PyImport, stmt.span);
            }

            StmtKind::ExprStmt(expr) => self.resolve_expr(expr),

            StmtKind::UnsafeBlock(body) => {
                for s in body {
                    self.resolve_stmt(s);
                }
            }

            StmtKind::Pass | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Enum { .. } => {}
            StmtKind::Defer(expr) => {
                self.resolve_expr(expr);
            }
        }
    }

    fn resolve_block(&mut self, stmts: &[Stmt]) {
        self.table.push(ScopeKind::Block);
        self.hoist_fns_and_structs(stmts);
        for s in stmts {
            self.resolve_stmt(s);
        }
        self.pop_scope();
    }

    fn resolve_params(&mut self, params: &[Param]) {
        let mut seen: std::collections::HashMap<String, Span> = std::collections::HashMap::new();
        for param in params {
            if let Some(&first) = seen.get(&param.name) {
                self.errors.push(SemanticError::DuplicateParam {
                    name: param.name.clone(),
                    span: param.span,
                    first,
                });
            } else {
                seen.insert(param.name.clone(), param.span);
                let sym = Symbol {
                    name: param.name.clone(),
                    kind: SymbolKind::Parameter,
                    span: param.span,
                    is_private: param.name.starts_with('_'),
                    used: false,
                };
                self.table.define(sym);
            }
            if let Some(default) = &param.default {
                self.resolve_expr(default);
            }
        }
    }

    fn define_for_target(&mut self, target: &ForTarget) {
        match target {
            ForTarget::Name(name, span) => {
                self.define_sym(name, SymbolKind::LoopVar, *span);
            }
            ForTarget::Tuple(names) => {
                for (name, span) in names {
                    self.define_sym(name, SymbolKind::LoopVar, *span);
                }
            }
        }
    }

    /// Nearest visible names to an unresolved identifier, ordered nearest first,
    /// plus whether the best one is an unambiguous autofix winner.
    fn suggest(&self, name: &str) -> (Vec<String>, bool) {
        super::suggest::ranked(name, self.table.visible_names(), 3)
    }

    fn resolve_assign_target(&mut self, target: &Expr) {
        match &target.kind {
            ExprKind::Identifier(name) => {
                if self.table.lookup(name).is_none() {
                    let (suggestions, can_autofix) = self.suggest(name);
                    self.errors.push(SemanticError::AssignToUndefined {
                        name: name.clone(),
                        span: target.span,
                        suggestions,
                        can_autofix,
                    });
                } else {
                    self.table.mark_used(name);
                }
            }
            ExprKind::Index { obj, index } => {
                self.resolve_expr(obj);
                self.resolve_expr(index);
            }
            ExprKind::Attr { obj, .. } => {
                self.resolve_expr(obj);
            }
            ExprKind::Tuple(elems) => {
                for e in elems {
                    self.resolve_assign_target(e);
                }
            }
            _ => self.resolve_expr(target),
        }
    }

    fn resolve_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Identifier(name) => {
                if name.starts_with("__olive_") {
                    return;
                }
                if let Some(sym) = self.table.lookup(name) {
                    if sym.is_private && sym.span.file_id != expr.span.file_id {
                        self.errors.push(SemanticError::PrivateAccess {
                            name: name.clone(),
                            span: expr.span,
                        });
                    }
                    // Check for ambiguous bare enum variant names.
                    if let Some(origins) = self.enum_variant_origins.get(name)
                        && origins.len() > 1
                    {
                        let qualifiers: Vec<String> =
                            origins.iter().map(|e| format!("`{e}::{name}`")).collect();
                        self.errors.push(SemanticError::rich(
                            crate::compile::errors::Diagnostic::error(
                                "E0427",
                                format!("ambiguous variant `{name}`"),
                                expr.span,
                            )
                            .label("variant name is defined by multiple enums")
                            .help(format!("qualify the variant: {}", qualifiers.join(" or "))),
                        ));
                        return;
                    }
                    self.table.mark_used(name);
                } else {
                    let (suggestions, can_autofix) = self.suggest(name);
                    self.errors.push(SemanticError::UndefinedName {
                        name: name.clone(),
                        span: expr.span,
                        suggestions,
                        can_autofix,
                    });
                }
            }

            ExprKind::BinOp { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }

            ExprKind::UnaryOp { operand, .. } => self.resolve_expr(operand),

            ExprKind::Call { callee, args } => {
                self.resolve_expr(callee);
                for arg in args {
                    match arg {
                        CallArg::Positional(e)
                        | CallArg::Keyword(_, e)
                        | CallArg::Splat(e)
                        | CallArg::KwSplat(e) => self.resolve_expr(e),
                    }
                }
            }

            ExprKind::Index { obj, index } => {
                self.resolve_expr(obj);
                self.resolve_expr(index);
            }

            ExprKind::Attr { obj, attr } => {
                if let ExprKind::Identifier(name) = &obj.kind
                    && let Some(sym) = self.table.lookup(name)
                {
                    if sym.kind == SymbolKind::NativeImport {
                        return;
                    }
                    if sym.kind == SymbolKind::PyImport {
                        return;
                    }
                    if sym.kind == SymbolKind::Import {
                        let mangled = format!("{}::{}", name, attr);
                        if self.table.lookup(&mangled).is_none() {
                            let (suggestions, can_autofix) = self.suggest(&mangled);
                            self.errors.push(SemanticError::UndefinedName {
                                name: mangled,
                                span: expr.span,
                                suggestions,
                                can_autofix,
                            });
                        }
                        return;
                    }
                }
                self.resolve_expr(obj);
            }

            ExprKind::OptAttr { obj, .. } => {
                self.resolve_expr(obj);
            }

            ExprKind::List(elems) | ExprKind::Tuple(elems) | ExprKind::Set(elems) => {
                for e in elems {
                    self.resolve_expr(e);
                }
            }

            ExprKind::Dict(pairs) => {
                for (k, v) in pairs {
                    self.resolve_expr(k);
                    self.resolve_expr(v);
                }
            }

            ExprKind::ListComp { elt, clauses } | ExprKind::SetComp { elt, clauses } => {
                self.resolve_comp_clauses(clauses);
                self.resolve_expr(elt);
                self.pop_scope();
            }

            ExprKind::DictComp {
                key,
                value,
                clauses,
            } => {
                self.resolve_comp_clauses(clauses);
                self.resolve_expr(key);
                self.resolve_expr(value);
                self.pop_scope();
            }

            ExprKind::Borrow(inner) | ExprKind::MutBorrow(inner) | ExprKind::Deref(inner) => {
                self.resolve_expr(inner);
            }

            ExprKind::Slice { start, stop, step } => {
                if let Some(e) = start {
                    self.resolve_expr(e);
                }
                if let Some(e) = stop {
                    self.resolve_expr(e);
                }
                if let Some(e) = step {
                    self.resolve_expr(e);
                }
            }

            ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::FStr(_)
            | ExprKind::Bool(_)
            | ExprKind::Null => {
                if let ExprKind::FStr(parts) = &expr.kind {
                    for p in parts {
                        self.resolve_expr(&p.expr);
                    }
                }
            }
            ExprKind::Ternary {
                cond,
                then,
                otherwise,
            } => {
                self.resolve_expr(cond);
                self.resolve_expr(then);
                self.resolve_expr(otherwise);
            }
            ExprKind::Match { expr, cases } => {
                self.resolve_expr(expr);
                for case in cases {
                    self.table.push(ScopeKind::Block);
                    self.resolve_pattern(&case.pattern);
                    if let Some(g) = &case.guard {
                        self.resolve_expr(g);
                    }
                    for stmt in &case.body {
                        self.resolve_stmt(stmt);
                    }
                    self.pop_scope();
                }
            }

            ExprKind::Try(inner) => {
                self.resolve_expr(inner);
            }
            ExprKind::Await(inner) => {
                self.resolve_expr(inner);
            }
            ExprKind::Range { start, end, .. } => {
                self.resolve_expr(start);
                self.resolve_expr(end);
            }
            ExprKind::Cast(operand, _) => {
                self.resolve_expr(operand);
            }
            ExprKind::AsyncBlock(body) => {
                self.table.push(ScopeKind::Block);
                for s in body {
                    self.resolve_stmt(s);
                }
                self.pop_scope();
            }
            ExprKind::Lambda { params, body } => {
                self.table.push(ScopeKind::Block);
                for p in params {
                    if p.name != "_" {
                        self.define_sym(&p.name, SymbolKind::Variable, p.span);
                    }
                }
                self.resolve_expr(body);
                self.pop_scope();
            }
        }
    }

    fn resolve_pattern(&mut self, pattern: &MatchPattern) {
        match pattern {
            MatchPattern::Wildcard => {}
            MatchPattern::Identifier(name, name_span) => {
                self.define_sym(name, SymbolKind::Variable, *name_span);
            }
            MatchPattern::Variant(_, inner_patterns) => {
                for p in inner_patterns {
                    self.resolve_pattern(p);
                }
            }
            MatchPattern::Literal(expr) => {
                self.resolve_expr(expr);
            }
        }
    }

    fn resolve_comp_clauses(&mut self, clauses: &[CompClause]) {
        self.table.push(ScopeKind::Comprehension);
        for clause in clauses {
            self.resolve_expr(&clause.iter);
            self.define_for_target(&clause.target);
            if let Some(cond) = &clause.condition {
                self.resolve_expr(cond);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn resolve(src: &str) -> Resolver {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        r
    }

    fn has_unused(r: &Resolver, name: &str) -> bool {
        r.warnings.iter().any(|w| {
            let d = w.to_diagnostic();
            d.code() == Some("W0640") && d.headline().contains(&format!("`{name}`"))
        })
    }

    #[test]
    fn unused_let_reported() {
        let r = resolve("fn f():\n    let x = 1\n    pass\n");
        assert!(has_unused(&r, "x"));
    }

    #[test]
    fn used_let_not_reported() {
        let r = resolve("fn f():\n    let x = 1\n    print(x)\n");
        assert!(!has_unused(&r, "x"));
    }

    #[test]
    fn underscore_binding_not_reported() {
        let r = resolve("fn f():\n    let _x = 1\n    pass\n");
        assert!(!has_unused(&r, "_x"));
    }

    #[test]
    fn unused_param_reported() {
        let r = resolve("fn f(unusedp: i64):\n    pass\n");
        assert!(has_unused(&r, "unusedp"));
    }

    #[test]
    fn self_param_not_reported() {
        let r = resolve("impl Foo:\n    fn m(self):\n        pass\n");
        assert!(!has_unused(&r, "self"));
    }

    #[test]
    fn assignment_counts_as_use() {
        let r = resolve("fn f():\n    let mut x = 1\n    x = 2\n");
        assert!(!has_unused(&r, "x"));
    }

    #[test]
    fn capture_in_nested_fn_counts_as_use() {
        let r = resolve("fn outer():\n    let v = 1\n    fn inner() -> i64:\n        return v\n");
        assert!(!has_unused(&r, "v"));
    }

    #[test]
    fn top_level_let_not_reported() {
        let r = resolve("let g = 1\n");
        assert!(!has_unused(&r, "g"));
    }

    #[test]
    fn unused_loop_var_reported() {
        let r = resolve("fn f():\n    for item in [1, 2]:\n        pass\n");
        assert!(has_unused(&r, "item"));
    }

    #[test]
    fn builtin_print_resolves() {
        let r = resolve("print(1)\n");
        assert!(r.errors.is_empty());
    }

    #[test]
    fn undefined_name_reported() {
        let r = resolve("no_such_name\n");
        assert!(r.errors.iter().any(|e| matches!(
            e,
            SemanticError::UndefinedName { name, .. } if name == "no_such_name"
        )));
    }

    #[test]
    fn undefined_name_suggests_nearest_binding() {
        let r = resolve("let total = 1\nprint(totl)\n");
        let suggestions = r.errors.iter().find_map(|e| match e {
            SemanticError::UndefinedName {
                name, suggestions, ..
            } if name == "totl" => Some(suggestions.clone()),
            _ => None,
        });
        assert_eq!(suggestions, Some(vec!["total".to_string()]));
    }

    #[test]
    fn unrelated_undefined_name_has_no_suggestion() {
        let r = resolve("let total = 1\nprint(xyzzy)\n");
        let suggestions = r.errors.iter().find_map(|e| match e {
            SemanticError::UndefinedName {
                name, suggestions, ..
            } if name == "xyzzy" => Some(suggestions.clone()),
            _ => None,
        });
        assert_eq!(suggestions, Some(vec![]));
    }

    #[test]
    fn let_binding_visible_after_definition() {
        let r = resolve("let x = 42\nprint(x)\n");
        assert!(r.errors.is_empty());
    }

    #[test]
    fn variable_not_visible_before_let() {
        let r = resolve("print(x)\nlet x = 1\n");
        assert!(
            r.errors
                .iter()
                .any(|e| matches!(e, SemanticError::UndefinedName { .. }))
        );
    }

    #[test]
    fn function_hoisting_allows_forward_call() {
        let r = resolve(
            "fn main() -> i64:\n    return helper()\n\nfn helper() -> i64:\n    return 0\n",
        );
        assert!(r.errors.is_empty());
    }

    #[test]
    fn duplicate_param_reported() {
        let r = resolve("fn bad(x: i64, x: i64):\n    pass\n");
        assert!(
            r.errors
                .iter()
                .any(|e| matches!(e, SemanticError::DuplicateParam { .. }))
        );
    }

    #[test]
    fn assign_to_undefined_reported() {
        let r = resolve("x = 99\n");
        assert!(
            r.errors
                .iter()
                .any(|e| matches!(e, SemanticError::AssignToUndefined { .. }))
        );
    }

    #[test]
    fn struct_hoisted_before_use() {
        let r = resolve(
            "fn make() -> i64:\n    let p = Point(1, 2)\n    return 0\n\nstruct Point:\n    x: i64\n    y: i64\n",
        );
        assert!(r.errors.is_empty());
    }

    #[test]
    fn for_loop_binds_target() {
        let r = resolve("for i in [1, 2, 3]:\n    print(i)\n");
        assert!(r.errors.is_empty());
    }

    #[test]
    fn if_branches_scoped() {
        let r = resolve("if 1 == 1:\n    let x = 10\n");
        assert!(r.errors.is_empty());
    }

    #[test]
    fn unsafe_block_resolves_body() {
        let r = resolve("unsafe:\n    let x = 1\n    print(x)\n");
        assert!(r.errors.is_empty());
    }

    #[test]
    fn native_import_alias_defined() {
        let r = resolve("import \"/usr/lib/libc.so.6\" as libc:\n    fn puts(s: str) -> i64\n");
        assert!(r.errors.is_empty());
    }
}
