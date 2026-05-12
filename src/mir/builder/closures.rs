use super::{MirBuilder, NestedFnInfo};
use crate::mir::ir::Local;
use crate::parser::{Stmt, StmtKind};
use crate::semantic::free_vars::free_variables;
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
