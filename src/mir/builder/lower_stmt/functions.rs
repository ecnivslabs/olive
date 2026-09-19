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
                        Type::Struct(struct_name.to_string(), Vec::new(), false)
                    } else {
                        ty
                    }
                } else {
                    ty
                };
                let local = self.declare_var(param.name.clone(), ty, param.is_mut);
                self.current_locals[local.0].is_owning = *is_async
                    && !matches!(
                        crate::semantic::type_descriptor::concrete_ty(
                            &self.current_locals[local.0].ty
                        ),
                        Type::Struct(_, _, true)
                    );
                param_locals.push(local);
            }

            // Async wrappers own runtime-managed params and captures. Foreign
            // struct layouts have no descriptor-driven copy and remain borrowed.
            for cap in &captures {
                let local = self.declare_var(cap.name.clone(), cap.ty.clone(), false);
                self.current_locals[local.0].is_owning = *is_async
                    && !matches!(
                        crate::semantic::type_descriptor::concrete_ty(&cap.ty),
                        Type::Struct(_, _, true)
                    );
            }
            self.current_arg_count += captures.len();

            // A struct's own `__drop__` runs once per logical owner, but an
            // implicit compiler copy of a has-drop struct (async task-arg
            // marshalling, escape-copies) shares the same allocation instead
            // of duplicating it -- see `struct_share.rs`. So every drop call
            // must check in with the shared refcount first and only run the
            // real body (and reclaim `self`'s own memory, at this function's
            // ordinary end) when it is actually the last reference.
            if mangled.ends_with("::__drop__") && !param_locals.is_empty() {
                let self_local = param_locals[0];
                let gate = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        gate,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_struct_gate".to_string(),
                            )),
                            args: vec![Operand::Copy(self_local)],
                        },
                    ),
                    stmt.span,
                );
                let body_bb = self.new_block();
                let skip_bb = self.new_block();
                let cur_bb = self.current_block.unwrap();
                self.terminate_block(
                    cur_bb,
                    TerminatorKind::SwitchInt {
                        discr: Operand::Copy(gate),
                        targets: vec![(1, body_bb)],
                        otherwise: skip_bb,
                    },
                    stmt.span,
                );
                self.terminate_block(skip_bb, TerminatorKind::Return, stmt.span);
                self.current_block = Some(body_bb);
                // From here to the end of this function every exit path must
                // reclaim `self` (see `emit_drop_self_reclaim`): the hook
                // consumes it but no other party releases its storage. The
                // skip block above returns directly and stays untouched.
                if let Some(suffix) = mangled.strip_suffix("::__drop__") {
                    self.drop_self_reclaim =
                        Some((mangled.clone(), self_local, suffix.to_string()));
                }
            }

            self.nested_fns
                .push(self.collect_nested_fns(body, &mangled));
            self.bound_lambdas
                .push(self.collect_bound_lambdas(body, &mangled));

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
                let dummy = self.new_unscoped_local_with_owning(Type::Any, false);
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
                    // Defers may still read `self`; reclaim only after they run.
                    self.emit_drop_self_reclaim(Span::default());
                    self.terminate_block(bb, TerminatorKind::Return, Span::default());
                }
            }

            self.drop_self_reclaim = None;
            self.nested_fns.pop();
            self.bound_lambdas.pop();
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
    pub(crate) fn inferred_return_type(&self, name: &str, is_async: bool) -> Type {
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

    /// Reclaim a `__drop__`'s `self` storage at a function exit (early return
    /// or fall-through end), if currently lowering that hook's own body. The
    /// hook consumes `self` but nothing else releases it: fields go through
    /// typed free (nested resource structs recurse through their own hooks,
    /// everything else matches what the silent container-free path would do),
    /// then the record slot itself. A no-op anywhere else, including nested
    /// functions and closures (which run under their own names).
    pub(super) fn emit_drop_self_reclaim(&mut self, span: Span) {
        let Some((fn_name, self_local, struct_name)) = self.drop_self_reclaim.clone() else {
            return;
        };
        if fn_name != self.current_name {
            return;
        }
        let fields = match self.struct_fields.get(&struct_name).cloned() {
            Some(fields) => fields,
            None => return,
        };
        if self.current_block.is_none() {
            return;
        }
        // Fields the user already moved or dropped out of `self` (a
        // `GetAttr(self)` temp later used as `Move`, or named in a `Drop`)
        // have an owner that frees them; reclaiming here as well would free
        // twice (benign today only through the generation guard), so those
        // fields are skipped below.
        let moved_fields = self.moved_self_fields(self_local);
        // A zeroed `self` reaches here when a scope drops an already-moved
        // local a second time (codegen zeroes vars after Drop): every other
        // free path no-ops on null, and GetAttr below would fault reading
        // field words from address 0, so skip the whole reclaim on null.
        let reclaim_bb = self.new_block();
        let done_bb = self.new_block();
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(self_local),
                targets: vec![(0, done_bb)],
                otherwise: reclaim_bb,
            },
            span,
        );
        self.current_block = Some(reclaim_bb);
        for field in &fields {
            if moved_fields.contains(field) {
                continue;
            }
            let field_ty = self
                .struct_field_types
                .get(&(struct_name.clone(), field.clone()))
                .cloned()
                .unwrap_or(Type::Int);
            let desc = crate::semantic::type_descriptor::type_descriptor(
                &field_ty,
                &self.struct_fields,
                &self.struct_field_types,
                &self.enum_defs,
            );
            let field_tmp = self.new_unscoped_local_with_owning(Type::Any, false);
            self.push_statement(
                StatementKind::Assign(
                    field_tmp,
                    Rvalue::GetAttr(Operand::Copy(self_local), field.clone()),
                ),
                span,
            );
            {
                let sink = self.new_unscoped_local_with_owning(Type::Any, false);
                let rval = match &field_ty {
                    // A nested resource struct recurses through its own hook
                    // (which reclaims it), mirroring what `lower_drop_hooks`
                    // does for a direct local of that type; anything else goes
                    // through typed free like the silent container path.
                    Type::Struct(field_struct, field_args, _) => {
                        let mono = crate::mir::optimizations::drop_hooks::monomorphized_name(
                            field_struct,
                            field_args,
                        );
                        let stripped = field_struct.rsplit("::").next().unwrap_or(field_struct);
                        if self.has_drop_structs.contains(field_struct)
                            || self.has_drop_structs.contains(&mono)
                            || self.has_drop_structs.contains(stripped)
                        {
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(format!(
                                    "{}::__drop__",
                                    mono
                                ))),
                                args: vec![Operand::Move(field_tmp)],
                            }
                        } else {
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_free_typed".to_string(),
                                )),
                                args: vec![
                                    Operand::Move(field_tmp),
                                    Operand::Constant(Constant::Str(desc)),
                                ],
                            }
                        }
                    }
                    _ => Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_free_typed".to_string(),
                        )),
                        args: vec![
                            Operand::Move(field_tmp),
                            Operand::Constant(Constant::Str(desc)),
                        ],
                    },
                };
                self.push_statement(StatementKind::Assign(sink, rval), span);
            }
        }
        let sink = self.new_unscoped_local_with_owning(Type::Any, false);
        self.push_statement(
            StatementKind::Assign(
                sink,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_free_struct".to_string())),
                    args: vec![Operand::Copy(self_local)],
                },
            ),
            span,
        );
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: done_bb },
            span,
        );
        self.current_block = Some(done_bb);
    }

    /// Fields of `self_local` the `__drop__` body already moved or dropped:
    /// a temp assigned from `GetAttr(self)` that is later used as `Move` (its
    /// new owner frees it) or named in a `Drop` (freed at that point).
    fn moved_self_fields(&self, self_local: Local) -> std::collections::HashSet<String> {
        use std::collections::{HashMap, HashSet};
        fn is_self(op: &Operand, owner: Local) -> bool {
            matches!(op, Operand::Copy(l) | Operand::Move(l) if *l == owner)
        }
        fn moves(rval: &Rvalue, tmp: Local) -> bool {
            let op = |o: &Operand| matches!(o, Operand::Move(l) if *l == tmp);
            match rval {
                Rvalue::Use(o)
                | Rvalue::UnaryOp(_, o)
                | Rvalue::Cast(o, _)
                | Rvalue::GetAttr(o, _)
                | Rvalue::GetTag(o)
                | Rvalue::GetTypeId(o)
                | Rvalue::VectorSplat(o, _)
                | Rvalue::VectorReduce(_, o, _)
                | Rvalue::PtrLoad(o)
                | Rvalue::FatPtrData(o)
                | Rvalue::GenOf(o) => op(o),
                Rvalue::BinaryOp(_, a, b) | Rvalue::GetIndex(a, b, _) => op(a) || op(b),
                Rvalue::VectorLoad(a, b, _) => op(a) || op(b),
                Rvalue::VectorFMA(a, b, c) => op(a) || op(b) || op(c),
                Rvalue::Call { func, args } => op(func) || args.iter().any(op),
                Rvalue::Aggregate(_, ops) => ops.iter().any(op),
                Rvalue::VTableLoad { vtable, .. } => op(vtable),
                Rvalue::Ref(_) | Rvalue::MutRef(_) => false,
            }
        }
        let mut field_of_tmp: HashMap<Local, String> = HashMap::new();
        for bb in &self.current_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(tmp, Rvalue::GetAttr(obj, field)) = &stmt.kind
                    && is_self(obj, self_local)
                {
                    field_of_tmp.insert(*tmp, field.clone());
                }
            }
        }
        let mut moved = HashSet::new();
        if field_of_tmp.is_empty() {
            return moved;
        }
        for bb in &self.current_blocks {
            for stmt in &bb.statements {
                match &stmt.kind {
                    StatementKind::Assign(_, rval) => {
                        for (tmp, field) in &field_of_tmp {
                            if moves(rval, *tmp) {
                                moved.insert(field.clone());
                            }
                        }
                    }
                    StatementKind::SetAttr(a, _, v) | StatementKind::PtrStore(a, v) => {
                        for (tmp, field) in &field_of_tmp {
                            if matches!(a, Operand::Move(l) if *l == *tmp)
                                || matches!(v, Operand::Move(l) if *l == *tmp)
                            {
                                moved.insert(field.clone());
                            }
                        }
                    }
                    StatementKind::SetIndex(a, i, v, _) => {
                        for (tmp, field) in &field_of_tmp {
                            if [a, i, v]
                                .iter()
                                .any(|o| matches!(o, Operand::Move(l) if *l == *tmp))
                            {
                                moved.insert(field.clone());
                            }
                        }
                    }
                    StatementKind::Drop(l) => {
                        if let Some(field) = field_of_tmp.get(l) {
                            moved.insert(field.clone());
                        }
                    }
                    _ => {}
                }
            }
            if let Some(term) = &bb.terminator
                && let TerminatorKind::SwitchInt { discr, .. } = &term.kind
            {
                for (tmp, field) in &field_of_tmp {
                    if matches!(discr, Operand::Move(l) if *l == *tmp) {
                        moved.insert(field.clone());
                    }
                }
            }
        }
        moved
    }
}
