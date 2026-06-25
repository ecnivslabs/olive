use super::super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{Stmt, StmtKind};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    pub(super) fn lower_fn_def(&mut self, stmt: &Stmt) {
        if let StmtKind::Fn {
            name,
            params,
            body,
            decorators,
            return_type,
            is_async,
            type_params,
            ..
        } = &stmt.kind
        {
            if !type_params.is_empty() {
                self.generic_fns.insert(name.clone(), stmt.clone());
                return;
            }

            // Resolve captures against the still-live enclosing scope before
            // `start_function` clears it; nested fns read them as trailing params.
            let is_nested = self.current_name != "__main__" && !self.current_name.is_empty();
            let info = if is_nested {
                self.lookup_nested_fn(name)
            } else {
                None
            };
            let mangled = info
                .as_ref()
                .map(|i| i.mangled.clone())
                .unwrap_or_else(|| name.clone());
            let captures = match &info {
                Some(i) => self.resolve_captures(&i.raw_captures),
                None => Vec::new(),
            };

            if !self.fn_meta.contains_key(&mangled) {
                self.register_fn_meta(&mangled, params);
            }

            let is_memo = decorators
                .iter()
                .any(|d| d.name == "memo" && !d.is_directive);

            let saved_name = std::mem::take(&mut self.current_name);
            let saved_locals = std::mem::take(&mut self.current_locals);
            let saved_blocks = std::mem::take(&mut self.current_blocks);
            let saved_block = self.current_block.take();
            let saved_var_map = std::mem::take(&mut self.var_map);
            let saved_loop_stack = std::mem::take(&mut self.loop_stack);
            let saved_scope_locals = std::mem::take(&mut self.scope_locals);
            let saved_arg_count = self.current_arg_count;
            let saved_is_async = self.current_is_async;
            self.current_is_async = *is_async;

            // With no annotation the return type was inferred by the type
            // checker; read it back so the `_return` slot matches the type
            // callers see. Defaulting to `Any` here would box a concrete return
            // value that the caller then reads raw.
            let ret_ty = match return_type {
                Some(ann) => self.resolve_type_expr(ann),
                None => self.inferred_return_type(name, *is_async),
            };

            self.start_function(mangled.clone(), params.len(), ret_ty);

            let mut param_locals = Vec::new();
            for param in params {
                let ty = param
                    .type_ann
                    .as_ref()
                    .map(|ann| self.resolve_type_expr(ann))
                    .unwrap_or(Type::Any);
                let ty = if param.name == "self" && name.contains("::") {
                    let last_idx = name.rfind("::").unwrap();
                    let struct_name = &name[..last_idx];
                    if self.struct_fields.contains_key(struct_name) {
                        Type::Struct(struct_name.to_string(), Vec::new())
                    } else {
                        ty
                    }
                } else {
                    ty
                };
                let local = self.declare_var(param.name.clone(), ty, param.is_mut);
                self.current_locals[local.0].is_owning = false;
                param_locals.push(local);
            }

            // Captures are trailing params aliasing the caller's value (copied in);
            // the callee never owns or drops them.
            for cap in &captures {
                let local = self.declare_var(cap.name.clone(), cap.ty.clone(), false);
                self.current_locals[local.0].is_owning = false;
            }
            self.current_arg_count += captures.len();

            self.nested_fns
                .push(self.collect_nested_fns(body, &mangled));

            if is_memo {
                let cache_tmp = self.new_local(Type::Any, Some("cache".to_string()), false);
                let fn_name_const = Operand::Constant(Constant::Str(name.clone()));

                let is_tuple_val = if param_locals.len() > 1 { 1 } else { 0 };
                self.push_statement(
                    StatementKind::Assign(
                        cache_tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_memo_get".to_string(),
                            )),
                            args: vec![
                                fn_name_const,
                                Operand::Constant(Constant::Int(is_tuple_val)),
                            ],
                        },
                    ),
                    stmt.span,
                );

                let key = if param_locals.len() == 1 {
                    Operand::Copy(param_locals[0])
                } else {
                    let tuple_tmp = self.new_local(Type::Any, None, false);
                    let ops = param_locals.iter().map(|l| Operand::Copy(*l)).collect();
                    self.push_statement(
                        StatementKind::Assign(
                            tuple_tmp,
                            Rvalue::Aggregate(AggregateKind::Tuple, ops),
                        ),
                        stmt.span,
                    );
                    Operand::Copy(tuple_tmp)
                };

                let (has_fn, get_fn, set_fn) = if param_locals.len() == 1 {
                    (
                        "__olive_cache_has",
                        "__olive_cache_get",
                        "__olive_cache_set",
                    )
                } else {
                    (
                        "__olive_cache_has_tuple",
                        "__olive_cache_get_tuple",
                        "__olive_cache_set_tuple",
                    )
                };

                let cond_tmp = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        cond_tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(has_fn.to_string())),
                            args: vec![Operand::Copy(cache_tmp), key.clone()],
                        },
                    ),
                    stmt.span,
                );

                let body_bb = self.new_block();
                let return_bb = self.new_block();
                let exit_bb = self.new_block();

                self.memo_context = Some((Operand::Copy(cache_tmp), key.clone(), exit_bb));

                let cur_bb = self.current_block.unwrap();
                self.terminate_block(
                    cur_bb,
                    TerminatorKind::SwitchInt {
                        discr: Operand::Copy(cond_tmp),
                        targets: vec![(1, return_bb)],
                        otherwise: body_bb,
                    },
                    stmt.span,
                );

                self.current_block = Some(return_bb);
                let hit_tmp = self.new_local(Type::Any, Some("cache_hit".to_string()), false);
                self.push_statement(
                    StatementKind::Assign(
                        hit_tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(get_fn.to_string())),
                            args: vec![Operand::Copy(cache_tmp), key.clone()],
                        },
                    ),
                    stmt.span,
                );
                self.push_statement(
                    StatementKind::Assign(Local(0), Rvalue::Use(Operand::Copy(hit_tmp))),
                    stmt.span,
                );
                self.terminate_block(return_bb, TerminatorKind::Return, stmt.span);

                self.current_block = Some(body_bb);
                for s in body {
                    self.lower_stmt(s);
                }

                if let Some(bb) = self.current_block {
                    self.terminate_block(bb, TerminatorKind::Goto { target: exit_bb }, stmt.span);
                }

                self.current_block = Some(exit_bb);
                let (cache_val, key_val, _) = self.memo_context.as_ref().unwrap().clone();
                let res_local = Local(0);
                let dummy = self.new_local(Type::Any, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        dummy,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(set_fn.to_string())),
                            args: vec![cache_val, key_val, Operand::Copy(res_local)],
                        },
                    ),
                    stmt.span,
                );
                self.terminate_block(exit_bb, TerminatorKind::Return, stmt.span);

                self.memo_context = None;
            } else {
                for (i, s) in body.iter().enumerate() {
                    self.lower_stmt_with_tail(s, i == body.len() - 1);
                }

                if let Some(bb) = self.current_block {
                    self.emit_defers();
                    self.terminate_block(bb, TerminatorKind::Return, Span::default());
                }
            }

            self.nested_fns.pop();
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

    pub(crate) fn lower_fn_def_or_impl(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Fn { .. } => self.lower_fn_def(stmt),
            StmtKind::Impl {
                type_params,
                type_name,
                body,
                ..
            } => {
                // Generic impls (impl[T] Struct[T]) are handled by lower_stmt which stores
                // their methods in generic_fns for later monomorphization.
                if !type_params.is_empty() {
                    self.lower_stmt(stmt);
                    return;
                }
                let type_base_name = Self::type_expr_base_name(type_name);
                let body = body.clone();
                for s in &body {
                    if let StmtKind::Fn { name: fn_name, .. } = &s.kind {
                        let mangled = format!("{}::{}", type_base_name, fn_name);
                        let mut impl_stmt = s.clone();
                        if let StmtKind::Fn {
                            name: ref mut n, ..
                        } = impl_stmt.kind
                        {
                            *n = mangled;
                        }
                        self.lower_fn_def(&impl_stmt);
                    } else if let StmtKind::Const {
                        name: const_name,
                        value,
                        ..
                    } = &s.kind
                    {
                        let mangled = format!("{}::{}", type_base_name, const_name);
                        let rval = self.lower_expr(value);
                        if let Operand::Constant(_) = &rval {
                            self.globals.insert(mangled, rval);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The type checker's inferred return type for an un-annotated function,
    /// read from its resolved signature. An async function's signature carries
    /// a `Future[T]`, but the `_return` slot holds the inner `T`. A return type
    /// left unconstrained (a bare type variable) falls back to `Any`.
    fn inferred_return_type(&self, name: &str, is_async: bool) -> Type {
        let ret = match self.global_types.get(name) {
            Some(Type::Fn(_, ret, _)) => (**ret).clone(),
            _ => return Type::Any,
        };
        let ret = if is_async {
            match ret {
                Type::Future(inner) => *inner,
                other => other,
            }
        } else {
            ret
        };
        match ret {
            Type::Var(_) => Type::Any,
            other => other,
        }
    }

    /// Extract the base struct name from a TypeExpr.
    /// For `Box[T]` (TypeExprKind::Generic("Box", _)) returns "Box".
    /// For `Box` (TypeExprKind::Name("Box")) returns "Box".
    pub(crate) fn type_expr_base_name(type_name: &crate::parser::TypeExpr) -> String {
        use crate::parser::TypeExprKind;
        match &type_name.kind {
            TypeExprKind::Name(n) => n.clone(),
            TypeExprKind::Generic(n, _) => n.clone(),
            _ => type_name.to_string(),
        }
    }
}
