use super::error::SemanticError;
use super::symbol_table::{ScopeKind, Symbol, SymbolKind, SymbolTable};
use crate::parser::ast::{
    CallArg, CompClause, Expr, ExprKind, ForTarget, MatchPattern, Param, Program, Stmt, StmtKind,
};
use crate::span::Span;

pub struct Resolver {
    pub table: SymbolTable,
    pub errors: Vec<SemanticError>,
    #[allow(dead_code)]
    pub current_file_id: usize,
}

impl Resolver {
    pub fn new() -> Self {
        let mut table = SymbolTable::new();
        table.define(Symbol {
            name: "print".to_string(),
            kind: SymbolKind::Function,
            span: Span::default(),
            is_private: false,
        });
        table.define(Symbol {
            name: "str".to_string(),
            kind: SymbolKind::Function,
            span: Span::default(),
            is_private: false,
        });
        table.define(Symbol {
            name: "int".to_string(),
            kind: SymbolKind::Function,
            span: Span::default(),
            is_private: false,
        });
        for ty_name in [
            "i64", "i32", "i16", "i8", "u64", "u32", "u16", "u8", "float", "f64", "f32", "bool",
        ] {
            table.define(Symbol {
                name: ty_name.to_string(),
                kind: SymbolKind::Function,
                span: Span::default(),
                is_private: false,
            });
        }
        table.define(Symbol {
            name: "type".to_string(),
            kind: SymbolKind::Function,
            span: Span::default(),
            is_private: false,
        });
        table.define(Symbol {
            name: "len".to_string(),
            kind: SymbolKind::Function,
            span: Span::default(),
            is_private: false,
        });
        table.define(Symbol {
            name: "list_new".to_string(),
            kind: SymbolKind::Function,
            span: Span::default(),
            is_private: false,
        });
        table.define(Symbol {
            name: "slice".to_string(),
            kind: SymbolKind::Function,
            span: Span::default(),
            is_private: false,
        });
        table.define(Symbol {
            name: "None".to_string(),
            kind: SymbolKind::Variable,
            span: Span::default(),
            is_private: false,
        });
        Self {
            table,
            errors: Vec::new(),
            current_file_id: 0,
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
                StmtKind::Impl {
                    type_name, body, ..
                } => {
                    for s in body {
                        if let StmtKind::Fn { name: fn_name, .. } = &s.kind {
                            let mangled = format!("{}::{}", type_name, fn_name);
                            self.define_sym(&mangled, SymbolKind::Function, s.span);
                        }
                    }
                }
                StmtKind::Trait { .. } => {}
                StmtKind::Enum { name, variants, .. } => {
                    self.define_sym(name, SymbolKind::Enum, stmt.span);
                    for variant in variants {
                        let mangled = format!("{}::{}", name, variant.name);
                        self.define_sym(&mangled, SymbolKind::Function, stmt.span);
                    }
                }
                _ => {}
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
        };
        self.table.define(sym);
    }

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let { name, value, .. } => {
                self.resolve_expr(value);
                self.define_sym(name, SymbolKind::Variable, stmt.span);
            }

            StmtKind::Const { name, value, .. } => {
                self.resolve_expr(value);
                self.define_sym(name, SymbolKind::Variable, stmt.span);
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
                ..
            } => {
                self.table.push(ScopeKind::Function);
                for tp in type_params {
                    self.define_sym(tp, SymbolKind::Variable, stmt.span);
                }
                self.resolve_params(params);
                self.hoist_fns_and_structs(body);
                for s in body {
                    self.resolve_stmt(s);
                }
                self.table.pop();
            }

            StmtKind::Struct {
                type_params,
                body,
                ..
            } => {
                self.table.push(ScopeKind::Block);
                for tp in type_params {
                    self.define_sym(tp, SymbolKind::Variable, stmt.span);
                }
                // struct field declarations have no resolvable expressions by default
                // any consts/nested types in body are resolved here
                for s in body {
                    self.resolve_stmt(s);
                }
                self.table.pop();
            }

            StmtKind::Impl {
                type_params,
                body,
                ..
            } => {
                self.table.push(ScopeKind::Struct);
                for tp in type_params {
                    self.define_sym(tp, SymbolKind::Variable, stmt.span);
                }
                self.hoist_fns_and_structs(body);
                for s in body {
                    self.resolve_stmt(s);
                }
                self.table.pop();
            }

            StmtKind::Trait { .. } => {}

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
                self.table.pop();
                if let Some(body) = else_body {
                    self.resolve_block(body);
                }
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

            StmtKind::NativeImport { alias, functions, structs, .. } => {
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
                        // bind as alias if provided, else original name
                        let bound = alias.as_deref().unwrap_or(name.as_str());
                        self.define_sym(bound, SymbolKind::Import, stmt.span);
                    }
                }
            }

            StmtKind::ExprStmt(expr) => self.resolve_expr(expr),

            StmtKind::Pass | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Enum { type_params: _, .. } => {
                // Enum definition has been hoisted
                // But we might have type params in variants (resolved during hoist?)
                // Actually variants are hoisted in hoist_fns_and_structs.
                // For now, if variants have type params, we need to handle them.
            }
        }
    }

    fn resolve_block(&mut self, stmts: &[Stmt]) {
        self.table.push(ScopeKind::Block);
        self.hoist_fns_and_structs(stmts);
        for s in stmts {
            self.resolve_stmt(s);
        }
        self.table.pop();
    }

    fn resolve_params(&mut self, params: &[Param]) {
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for param in params {
            if let Some(&_prev_line) = seen.get(&param.name) {
                self.errors.push(SemanticError::DuplicateParam {
                    name: param.name.clone(),
                    span: param.span,
                });
            } else {
                seen.insert(param.name.clone(), param.span.line);
                let sym = Symbol {
                    name: param.name.clone(),
                    kind: SymbolKind::Parameter,
                    span: param.span,
                    is_private: param.name.starts_with('_'),
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

    fn resolve_assign_target(&mut self, target: &Expr) {
        match &target.kind {
            ExprKind::Identifier(name) => {
                if self.table.lookup(name).is_none() {
                    self.errors.push(SemanticError::AssignToUndefined {
                        name: name.clone(),
                        span: target.span,
                    });
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
                } else {
                    self.errors.push(SemanticError::UndefinedName {
                        name: name.clone(),
                        span: expr.span,
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
                    if sym.kind == SymbolKind::Import {
                        let mangled = format!("{}::{}", name, attr);
                        if self.table.lookup(&mangled).is_none() {
                            self.errors.push(SemanticError::UndefinedName {
                                name: mangled,
                                span: expr.span,
                            });
                        }
                        return;
                    }
                }
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
                self.table.pop();
            }

            ExprKind::DictComp {
                key,
                value,
                clauses,
            } => {
                self.resolve_comp_clauses(clauses);
                self.resolve_expr(key);
                self.resolve_expr(value);
                self.table.pop();
            }

            ExprKind::Borrow(inner) | ExprKind::MutBorrow(inner) => {
                self.resolve_expr(inner);
            }

            ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::FStr(_)
            | ExprKind::Bool(_) => {
                if let ExprKind::FStr(exprs) = &expr.kind {
                    for e in exprs {
                        self.resolve_expr(e);
                    }
                }
            }
            ExprKind::Match { expr, cases } => {
                self.resolve_expr(expr);
                for case in cases {
                    self.table.push(ScopeKind::Block);
                    self.resolve_pattern(&case.pattern, expr.span);
                    for stmt in &case.body {
                        self.resolve_stmt(stmt);
                    }
                    self.table.pop();
                }
            }

            ExprKind::Try(inner) => {
                self.resolve_expr(inner);
            }
            ExprKind::Await(inner) => {
                self.resolve_expr(inner);
            }
            ExprKind::AsyncBlock(body) => {
                self.table.push(ScopeKind::Block);
                for s in body {
                    self.resolve_stmt(s);
                }
                self.table.pop();
            }
        }
    }

    fn resolve_pattern(&mut self, pattern: &MatchPattern, span: Span) {
        match pattern {
            MatchPattern::Wildcard => {}
            MatchPattern::Identifier(name) => {
                self.define_sym(name, SymbolKind::Variable, span);
            }
            MatchPattern::Variant(_, inner_patterns) => {
                for p in inner_patterns {
                    self.resolve_pattern(p, span);
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
            // iter resolves in outer scope (before the target is bound)
            self.resolve_expr(&clause.iter);
            self.define_for_target(&clause.target);
            if let Some(cond) = &clause.condition {
                self.resolve_expr(cond);
            }
        }
        // caller must pop after resolving the element expression
    }
}
