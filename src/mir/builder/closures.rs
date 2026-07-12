use super::{MirBuilder, NestedFnInfo};
use crate::mir::ir::Local;
use crate::parser::{Expr, ExprKind, Stmt, StmtKind};
use crate::semantic::free_vars::{free_variables, free_variables_expr};
use crate::semantic::types::Type;
use rustc_hash::FxHashMap as HashMap;

/// A resolved capture: source name, the enclosing local it reads, its type.
pub(super) struct ResolvedCapture {
    pub(super) name: String,
    pub(super) local: Local,
    pub(super) ty: Type,
}

impl<'a> MirBuilder<'a> {
    /// Builds the lifted-name + free-var table for fns defined directly in this body.
    pub(super) fn collect_nested_fns(
        &self,
        body: &[Stmt],
        parent: &str,
    ) -> HashMap<String, NestedFnInfo> {
        let mut defs = Vec::new();
        collect_fn_defs(body, &mut defs);
        let mut out: HashMap<String, NestedFnInfo> = HashMap::default();
        for stmt in defs {
            if let StmtKind::Fn {
                name,
                params,
                body,
                type_params,
                ..
            } = &stmt.kind
            {
                if !type_params.is_empty() {
                    continue;
                }
                let param_tys = params
                    .iter()
                    .map(|p| {
                        p.type_ann
                            .as_ref()
                            .map(|ann| self.resolve_type_expr(ann))
                            .unwrap_or(Type::Any)
                    })
                    .collect();
                out.insert(
                    name.clone(),
                    NestedFnInfo {
                        mangled: format!("{parent}${name}"),
                        raw_captures: free_variables(params, body),
                        param_tys,
                    },
                );
            }
        }
        out
    }

    /// Resolves `name` to the nearest lexically-enclosing nested fn.
    pub(super) fn lookup_nested_fn(&self, name: &str) -> Option<NestedFnInfo> {
        self.nested_fns
            .iter()
            .rev()
            .find_map(|m| m.get(name).cloned())
    }

    /// Builds the name -> lifted-lambda table for `let`/assign bindings of a
    /// lambda directly in this body (`let g = lambda ...: ...`). The mangled
    /// name is derived from the lambda expr's node id, computed identically
    /// here and in `lower_lambda_expr`, so this table can be built up front
    /// without lowering the lambda first.
    pub(super) fn collect_bound_lambdas(
        &self,
        body: &[Stmt],
        parent: &str,
    ) -> HashMap<String, NestedFnInfo> {
        let mut defs = Vec::new();
        collect_lambda_bindings(body, &mut defs);
        let mut out: HashMap<String, NestedFnInfo> = HashMap::default();
        for (name, lambda_expr) in defs {
            let ExprKind::Lambda {
                params,
                body: lbody,
            } = &lambda_expr.kind
            else {
                unreachable!("collect_lambda_bindings only collects Lambda exprs");
            };
            // An unannotated param's real type is the checker's inferred
            // one (see `lower_lambda_expr`); defaulting to `Any` here would
            // desync the call site's arg coercion from the callee's actual
            // param type.
            let checked_param_tys = match self.get_type(lambda_expr.id) {
                Type::Fn(p, _, _) => p,
                _ => Vec::new(),
            };
            let param_tys = params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    p.type_ann
                        .as_ref()
                        .map(|ann| self.resolve_type_expr(ann))
                        .or_else(|| checked_param_tys.get(i).cloned())
                        .unwrap_or(Type::Any)
                })
                .collect();
            out.insert(
                name.to_string(),
                NestedFnInfo {
                    mangled: format!("{parent}$lambda_{}", lambda_expr.id),
                    raw_captures: free_variables_expr(params, lbody),
                    param_tys,
                },
            );
        }
        out
    }

    /// Resolves `name` to a lambda bound directly to it in an enclosing
    /// scope. Checked independently of `lookup_nested_fn`, whose lookup
    /// deliberately defers to a same-named local; here the bound name IS
    /// the local.
    pub(super) fn lookup_bound_lambda(&self, name: &str) -> Option<NestedFnInfo> {
        self.bound_lambdas
            .iter()
            .rev()
            .find_map(|m| m.get(name).cloned())
    }

    /// Lowers a call to a lifted nested fn; captured locals appended as trailing args.
    pub(super) fn lower_nested_fn_call(
        &mut self,
        info: &NestedFnInfo,
        arg_ops: &[crate::mir::ir::Operand],
        arg_tys: &[Type],
        arg_kw_names: &[Option<String>],
        span: crate::span::Span,
        expr_id: usize,
    ) -> crate::mir::ir::Operand {
        use crate::mir::ir::{Constant, Operand, Rvalue, StatementKind};
        let mut args = self.pack_fn_call_args(
            &info.mangled,
            arg_ops,
            arg_tys,
            &info.param_tys,
            arg_kw_names,
            span,
        );
        for cap in self.resolve_captures(&info.raw_captures) {
            args.push(Operand::Copy(cap.local));
        }
        let result = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(
                result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(info.mangled.clone())),
                    args,
                },
            ),
            span,
        );
        self.operand_for_local(result)
    }

    /// Filters raw free-vars to currently-live enclosing locals, preserving order.
    pub(super) fn resolve_captures(&self, raw: &[String]) -> Vec<ResolvedCapture> {
        let mut out = Vec::new();
        for name in raw {
            if let Some(local) = self.lookup_var(name) {
                out.push(ResolvedCapture {
                    name: name.clone(),
                    local,
                    ty: self.current_locals[local.0].ty.clone(),
                });
            }
        }
        out
    }
}

/// Functions defined in `stmts`, descending control-flow but not fn bodies.
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
            StmtKind::With { body, .. } | StmtKind::UnsafeBlock(body) => {
                collect_fn_defs(body, out);
            }
            _ => {}
        }
    }
}

/// `(name, lambda_expr)` pairs for every `let`/assign binding of a lambda
/// directly to a name in `stmts`, descending control-flow but not fn or
/// lambda bodies.
fn collect_lambda_bindings<'s>(stmts: &'s [Stmt], out: &mut Vec<(&'s str, &'s Expr)>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Let { name, value, .. } | StmtKind::Const { name, value, .. } => {
                if matches!(value.kind, ExprKind::Lambda { .. }) {
                    out.push((name, value));
                }
            }
            StmtKind::Assign { target, value } => {
                if let ExprKind::Identifier(name) = &target.kind
                    && matches!(value.kind, ExprKind::Lambda { .. })
                {
                    out.push((name, value));
                }
            }
            StmtKind::If {
                then_body,
                elif_clauses,
                else_body,
                ..
            } => {
                collect_lambda_bindings(then_body, out);
                for (_, b) in elif_clauses {
                    collect_lambda_bindings(b, out);
                }
                if let Some(b) = else_body {
                    collect_lambda_bindings(b, out);
                }
            }
            StmtKind::While {
                body, else_body, ..
            }
            | StmtKind::For {
                body, else_body, ..
            } => {
                collect_lambda_bindings(body, out);
                if let Some(b) = else_body {
                    collect_lambda_bindings(b, out);
                }
            }
            StmtKind::With { body, .. } | StmtKind::UnsafeBlock(body) => {
                collect_lambda_bindings(body, out);
            }
            _ => {}
        }
    }
}
