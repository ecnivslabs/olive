//! Names visible at a position, for identifier-prefix completion. AST walk,
//! not `Resolver::SymbolTable` -- its scopes are popped after resolution.

use crate::parser::ast::{ForTarget, Program, Stmt, StmtKind};
use crate::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    Function,
    Struct,
    Enum,
    Variable,
    Parameter,
    Module,
}

#[derive(Debug, Clone)]
pub struct Binding {
    pub name: String,
    pub kind: BindingKind,
}

fn contains(span: Span, file_id: usize, offset: usize) -> bool {
    span.file_id == file_id && span.start <= offset && offset <= span.end
}

fn push(out: &mut Vec<Binding>, name: &str, kind: BindingKind) {
    out.push(Binding {
        name: name.to_string(),
        kind,
    });
}

fn collect_module_level(stmts: &[Stmt], out: &mut Vec<Binding>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Fn { name, .. } => push(out, name, BindingKind::Function),
            StmtKind::Struct { name, .. } => push(out, name, BindingKind::Struct),
            StmtKind::TypeAlias { name, .. } => push(out, name, BindingKind::Struct),
            StmtKind::Enum { name, variants, .. } => {
                push(out, name, BindingKind::Enum);
                for v in variants {
                    push(out, &v.name, BindingKind::Function);
                }
            }
            StmtKind::Let { name, .. } | StmtKind::Const { name, .. } => {
                push(out, name, BindingKind::Variable)
            }
            StmtKind::MultiLet { names, .. } | StmtKind::MultiConst { names, .. } => {
                for n in names {
                    push(out, n, BindingKind::Variable);
                }
            }
            StmtKind::Import { module, alias } => {
                let name = alias
                    .clone()
                    .unwrap_or_else(|| module.last().cloned().unwrap_or_default());
                push(out, &name, BindingKind::Module);
            }
            StmtKind::PyImport { alias, .. } | StmtKind::NativeImport { alias, .. } => {
                push(out, alias, BindingKind::Module)
            }
            _ => {}
        }
    }
}

fn collect_in_block(stmts: &[Stmt], file_id: usize, offset: usize, out: &mut Vec<Binding>) {
    for stmt in stmts {
        let declared_before_cursor = stmt.span.file_id == file_id && stmt.span.start <= offset;
        match &stmt.kind {
            StmtKind::Let { name, .. } | StmtKind::Const { name, .. } => {
                if declared_before_cursor {
                    push(out, name, BindingKind::Variable);
                }
            }
            StmtKind::MultiLet { names, .. } | StmtKind::MultiConst { names, .. } => {
                if declared_before_cursor {
                    for n in names {
                        push(out, n, BindingKind::Variable);
                    }
                }
            }
            StmtKind::Fn { params, body, .. } => {
                if contains(stmt.span, file_id, offset) {
                    for p in params {
                        push(out, &p.name, BindingKind::Parameter);
                    }
                    collect_in_block(body, file_id, offset, out);
                }
            }
            StmtKind::If {
                then_body,
                elif_clauses,
                else_body,
                ..
            } => {
                if contains(stmt.span, file_id, offset) {
                    collect_in_block(then_body, file_id, offset, out);
                    for (_, body) in elif_clauses {
                        collect_in_block(body, file_id, offset, out);
                    }
                    if let Some(b) = else_body {
                        collect_in_block(b, file_id, offset, out);
                    }
                }
            }
            StmtKind::While {
                body, else_body, ..
            } => {
                if contains(stmt.span, file_id, offset) {
                    collect_in_block(body, file_id, offset, out);
                    if let Some(b) = else_body {
                        collect_in_block(b, file_id, offset, out);
                    }
                }
            }
            StmtKind::For {
                target,
                body,
                else_body,
                ..
            } => {
                if contains(stmt.span, file_id, offset) {
                    match target {
                        ForTarget::Name(n, _) => push(out, n, BindingKind::Variable),
                        ForTarget::Tuple(names) => {
                            for (n, _) in names {
                                push(out, n, BindingKind::Variable);
                            }
                        }
                    }
                    collect_in_block(body, file_id, offset, out);
                    if let Some(b) = else_body {
                        collect_in_block(b, file_id, offset, out);
                    }
                }
            }
            StmtKind::With { items, body } => {
                if contains(stmt.span, file_id, offset) {
                    for item in items {
                        if let Some(alias) = &item.alias
                            && let crate::parser::ast::ExprKind::Identifier(n) = &alias.kind
                        {
                            push(out, n, BindingKind::Variable);
                        }
                    }
                    collect_in_block(body, file_id, offset, out);
                }
            }
            StmtKind::UnsafeBlock(body) => {
                if contains(stmt.span, file_id, offset) {
                    collect_in_block(body, file_id, offset, out);
                }
            }
            StmtKind::Impl { body, .. } => {
                if contains(stmt.span, file_id, offset) {
                    collect_in_block(body, file_id, offset, out);
                }
            }
            _ => {}
        }
    }
}

/// Module-level items (hoisted, visible everywhere) plus locals/params/loop
/// vars from enclosing blocks containing `offset`.
pub fn visible_bindings_at(program: &Program, file_id: usize, offset: usize) -> Vec<Binding> {
    let mut out = Vec::new();
    collect_module_level(&program.stmts, &mut out);
    collect_in_block(&program.stmts, file_id, offset, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn parse(src: &str) -> Program {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        Parser::new(tokens).parse_program().unwrap()
    }

    #[test]
    fn sees_module_level_fn_from_anywhere() {
        let src = "fn helper() -> int:\n    return 1\n\nfn main():\n    let y = 1\n";
        let program = parse(src);
        let offset = src.len(); // char count == byte count here (ascii)
        let bindings = visible_bindings_at(&program, 0, offset);
        let names: Vec<&str> = bindings.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"helper"));
    }

    #[test]
    fn sees_param_and_local_inside_function_body() {
        let src = "fn add(a: int, b: int) -> int:\n    let c = a + b\n    return c\n";
        let program = parse(src);
        let offset = src.find("return c").unwrap();
        let bindings = visible_bindings_at(&program, 0, offset);
        let names: Vec<&str> = bindings.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
    }

    #[test]
    fn does_not_see_local_from_unrelated_sibling_function() {
        let src = "fn one():\n    let secret = 1\n\nfn two():\n    let x = 2\n";
        let program = parse(src);
        let offset = src.find("let x").unwrap();
        let bindings = visible_bindings_at(&program, 0, offset);
        assert!(!bindings.iter().any(|b| b.name == "secret"));
    }

    #[test]
    fn sees_for_loop_target() {
        let src = "fn main():\n    for i in 0..10:\n        let y = i\n";
        let program = parse(src);
        let offset = src.find("let y").unwrap();
        let bindings = visible_bindings_at(&program, 0, offset);
        assert!(bindings.iter().any(|b| b.name == "i"));
    }
}
