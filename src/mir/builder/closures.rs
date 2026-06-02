use super::{MirBuilder, NestedFnInfo};
use crate::mir::ir::{Constant, Local, Operand, Rvalue, StatementKind, TerminatorKind};
use crate::parser::{Expr, ExprKind, Stmt, StmtKind};
use crate::semantic::free_vars::{free_variables, free_variables_expr};
use crate::semantic::type_descriptor::type_descriptor;
use crate::semantic::types::Type;
use crate::span::Span;
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

    /// Builds an escaping closure value: a heap record `[thunk_ptr, desc_ptr,
    /// cap0, cap1, ...]` allocated through the ordinary struct allocator,
    /// paired with a lazily-generated calling thunk, cast to `info`'s own
    /// `Type::Fn`. A non-capturing function gets the same record shape with
    /// zero captures -- see the E5.2/E5.3 design note in roadmap.md for why:
    /// one uniform representation means every indirect call site (E5.3) and
    /// every generic drop/copy site (below) has exactly one shape to handle.
    pub(super) fn build_closure_value(
        &mut self,
        info: &NestedFnInfo,
        expr_id: usize,
        span: Span,
    ) -> Operand {
        let fn_ty = self.get_type(expr_id);
        let (param_tys, ret_ty) = match &fn_ty {
            Type::Fn(p, r, _) => (p.clone(), (**r).clone()),
            _ => (info.param_tys.clone(), Type::Any),
        };
        let captures = self.resolve_captures(&info.raw_captures);
        let struct_name = format!("__closure${}", info.mangled);
        let thunk_name = format!("{}$thunk", info.mangled);

        if !self.struct_fields.contains_key(&struct_name) {
            let mut field_names = vec!["__thunk".to_string(), "__desc".to_string()];
            self.struct_field_types
                .insert((struct_name.clone(), "__thunk".to_string()), Type::Int);
            self.struct_field_types
                .insert((struct_name.clone(), "__desc".to_string()), Type::Int);
            for cap in &captures {
                field_names.push(cap.name.clone());
                self.struct_field_types
                    .insert((struct_name.clone(), cap.name.clone()), cap.ty.clone());
            }
            self.struct_fields.insert(struct_name.clone(), field_names);
        }

        if !self.closure_thunks.contains(&thunk_name) {
            self.closure_thunks.insert(thunk_name.clone());
            self.build_closure_thunk(
                &thunk_name,
                &info.mangled,
                &struct_name,
                &captures,
                &param_tys,
                ret_ty,
            );
        }

        // A per-instance descriptor, not a per-type one: two closures sharing
        // one `Type::Fn` signature can capture entirely different variables,
        // so the layout can't be looked up from the static type the way an
        // ordinary struct's can. Embedding it in the record lets the generic
        // drop/copy sites (`translate.rs`, `translate_call.rs`) load it back
        // at runtime and hand it to the unmodified struct free/copy path.
        let desc = type_descriptor(
            &Type::Struct(struct_name.clone(), Vec::new(), false),
            &self.struct_fields,
            &self.struct_field_types,
            &self.enum_defs,
        );

        let struct_ty = Type::Struct(struct_name, Vec::new(), false);
        let record = self.new_unscoped_local(struct_ty.clone());
        let n_fields = Operand::Constant(Constant::Int(2 + captures.len() as i64));
        self.push_statement(
            StatementKind::Assign(
                record,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_struct_alloc".to_string())),
                    args: vec![n_fields],
                },
            ),
            span,
        );
        self.push_statement(
            StatementKind::SetAttr(
                Operand::Copy(record),
                "__thunk".to_string(),
                Operand::Constant(Constant::Function(thunk_name)),
            ),
            span,
        );
        self.push_statement(
            StatementKind::SetAttr(
                Operand::Copy(record),
                "__desc".to_string(),
                Operand::Constant(Constant::Str(desc)),
            ),
            span,
        );
        for cap in &captures {
            self.push_statement(
                StatementKind::SetAttr(
                    Operand::Copy(record),
                    cap.name.clone(),
                    Operand::Copy(cap.local),
                ),
                span,
            );
        }

        // Same bits, final flowing type: a fresh allocation reinterpreted as
        // the closure's own `Type::Fn`, exactly like `lower_identifier_expr`
        // retags a narrowed union -- not a second alias of a shared value.
        let fn_view = self.new_local(fn_ty.clone(), None, true);
        self.push_statement(
            StatementKind::Assign(fn_view, Rvalue::Cast(Operand::Copy(record), fn_ty)),
            span,
        );
        self.operand_for_local(fn_view)
    }

    /// Emits the uniform-ABI forwarding thunk `<mangled>$thunk(params.., env)
    /// -> ret` for an escaping closure: unpacks captures off the env record
    /// and calls the original (fast-path, direct-call-only) function
    /// unchanged. One thunk per escaping definition, called only through
    /// `call_indirect` at a value-call site (E5.3); never on the direct-call
    /// path, which keeps calling `target_mangled` straight through.
    fn build_closure_thunk(
        &mut self,
        thunk_name: &str,
        target_mangled: &str,
        struct_name: &str,
        captures: &[ResolvedCapture],
        param_tys: &[Type],
        ret_ty: Type,
    ) {
        let saved_name = std::mem::take(&mut self.current_name);
        let saved_locals = std::mem::take(&mut self.current_locals);
        let saved_blocks = std::mem::take(&mut self.current_blocks);
        let saved_block = self.current_block.take();
        let saved_var_map = std::mem::take(&mut self.var_map);
        let saved_loop_stack = std::mem::take(&mut self.loop_stack);
        let saved_scope_locals = std::mem::take(&mut self.scope_locals);
        let saved_arg_count = self.current_arg_count;
        let saved_is_async = self.current_is_async;
        self.current_is_async = false;

        self.start_function(thunk_name.to_string(), param_tys.len() + 1, ret_ty);

        let mut fwd_args = Vec::with_capacity(param_tys.len() + captures.len());
        for (i, ty) in param_tys.iter().enumerate() {
            let local = self.declare_var(format!("__p{i}"), ty.clone(), false);
            self.current_locals[local.0].is_owning = false;
            fwd_args.push(Operand::Copy(local));
        }
        let env_local = self.declare_var(
            "__env".to_string(),
            Type::Struct(struct_name.to_string(), Vec::new(), false),
            false,
        );
        self.current_locals[env_local.0].is_owning = false;

        for cap in captures {
            let field_val = self.new_local_with_owning(cap.ty.clone(), None, true, false);
            self.push_statement(
                StatementKind::Assign(
                    field_val,
                    Rvalue::GetAttr(Operand::Copy(env_local), cap.name.clone()),
                ),
                Span::default(),
            );
            fwd_args.push(Operand::Copy(field_val));
        }

        self.push_statement(
            StatementKind::Assign(
                Local(0),
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(target_mangled.to_string())),
                    args: fwd_args,
                },
            ),
            Span::default(),
        );
        if let Some(bb) = self.current_block {
            // Mirrors ordinary `return expr` lowering (lower_stmt/mod.rs):
            // drop the unpacked-capture temps still owning a value, then
            // terminate and hand `finish_function`'s trailing `leave_scope`
            // a fresh, predecessor-less block. Without this, that
            // `leave_scope` drops `_return` (Local(0)) in this SAME live
            // block, freeing and zeroing the value being returned before
            // the `Return` terminator reads it.
            self.emit_open_scope_drops(0, None);
            self.terminate_block(bb, TerminatorKind::Return, Span::default());
        }
        self.current_block = Some(self.new_block());
        self.finish_function();

        self.current_name = saved_name;
        self.current_locals = saved_locals;
        self.current_blocks = saved_blocks;
        self.current_block = saved_block;
        self.var_map = saved_var_map;
        self.loop_stack = saved_loop_stack;
        self.scope_locals = saved_scope_locals;
        self.current_arg_count = saved_arg_count;
        self.current_is_async = saved_is_async;
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
