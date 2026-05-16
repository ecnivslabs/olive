use super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{ExprKind, Stmt, StmtKind};
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::FxHashMap as HashMap;

impl<'a> MirBuilder<'a> {
    pub(super) fn lower_stmt(&mut self, stmt: &Stmt) {
        if self.is_terminated() {
            return;
        }

        match &stmt.kind {
            StmtKind::Let {
                name,
                value,
                is_mut,
                ..
            } => {
                let rval = self.lower_expr(value);
                let ty = self.get_type(value.id);
                let local = self.declare_var(name.clone(), ty, *is_mut);
                self.push_statement(StatementKind::Assign(local, Rvalue::Use(rval)), stmt.span);
            }

            StmtKind::Const { name, value, .. } => {
                let rval = self.lower_expr(value);
                if let Operand::Constant(_) = &rval {
                    self.globals.insert(name.clone(), rval);
                } else {
                    let ty = self.get_type(value.id);
                    let local = self.declare_var(name.clone(), ty, false);
                    self.push_statement(StatementKind::Assign(local, Rvalue::Use(rval)), stmt.span);
                }
            }

            StmtKind::ExprStmt(expr) => {
                let rval = self.lower_expr(expr);
                let tmp = self.new_local(Type::Any, None, true);
                self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(rval)), expr.span);
            }

            StmtKind::Assign { target, value } => {
                self.lower_assign(target, value);
            }

            StmtKind::AugAssign { target, op, value } => {
                let bin_op = match op {
                    crate::parser::AugOp::Add => crate::parser::BinOp::Add,
                    crate::parser::AugOp::Sub => crate::parser::BinOp::Sub,
                    crate::parser::AugOp::Mul => crate::parser::BinOp::Mul,
                    crate::parser::AugOp::Div => crate::parser::BinOp::Div,
                    crate::parser::AugOp::Mod => crate::parser::BinOp::Mod,
                    crate::parser::AugOp::Pow => crate::parser::BinOp::Pow,
                    crate::parser::AugOp::Shl => crate::parser::BinOp::Shl,
                    crate::parser::AugOp::Shr => crate::parser::BinOp::Shr,
                };
                let lhs_op = self.lower_expr(target);
                let rhs_op = self.lower_expr(value);
                let tmp = self.new_local(Type::Any, None, true);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::BinaryOp(bin_op, lhs_op, rhs_op)),
                    stmt.span,
                );

                if let ExprKind::Identifier(name) = &target.kind
                    && let Some(local) = self.lookup_var(name)
                {
                    self.push_statement(
                        StatementKind::Assign(local, Rvalue::Use(Operand::Copy(tmp))),
                        stmt.span,
                    );
                }
            }

            StmtKind::Return(Some(expr)) => {
                let rval = self.lower_expr(expr);
                self.push_statement(
                    StatementKind::Assign(Local(0), Rvalue::Use(rval)),
                    stmt.span,
                );
                if let Some(bb) = self.current_block {
                    if let Some((_, _, exit_bb)) = self.memo_context {
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: exit_bb },
                            stmt.span,
                        );
                    } else {
                        self.terminate_block(bb, TerminatorKind::Return, stmt.span);
                    }
                }
                self.current_block = Some(self.new_block());
            }

            StmtKind::Return(None) => {
                if let Some(bb) = self.current_block {
                    if let Some((_, _, exit_bb)) = self.memo_context {
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: exit_bb },
                            stmt.span,
                        );
                    } else {
                        self.terminate_block(bb, TerminatorKind::Return, stmt.span);
                    }
                }
                self.current_block = Some(self.new_block());
            }

            StmtKind::If {
                condition,
                then_body,
                elif_clauses,
                else_body,
            } => {
                self.lower_if(condition, then_body, elif_clauses, else_body);
            }

            StmtKind::While {
                condition,
                body,
                else_body,
            } => {
                self.lower_while(condition, body, else_body);
            }

            StmtKind::For {
                target,
                iter,
                body,
                else_body,
            } => {
                self.lower_for(target, iter, body, else_body);
            }

            StmtKind::Break => {
                if let Some(ctx) = self.loop_stack.last() {
                    let exit = ctx.exit;
                    if let Some(bb) = self.current_block {
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: exit },
                            Span::default(),
                        );
                    }
                    self.current_block = Some(self.new_block());
                }
            }

            StmtKind::Continue => {
                if let Some(ctx) = self.loop_stack.last() {
                    let header = ctx.header;
                    if let Some(bb) = self.current_block {
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: header },
                            Span::default(),
                        );
                    }
                    self.current_block = Some(self.new_block());
                }
            }

            StmtKind::Fn { type_params, .. } => {
                if type_params.is_empty() {
                    self.lower_fn_def(stmt);
                }
            }

            StmtKind::Trait { .. } => {}

            StmtKind::Impl {
                type_params,
                type_name,
                body,
                ..
            } => {
                if !type_params.is_empty() {
                    for s in body {
                        if let StmtKind::Fn {
                            name: fn_name,
                            type_params: fn_type_params,
                            params,
                            return_type,
                            body: fn_body,
                            decorators,
                            is_async,
                        } = &s.kind
                        {
                            let mangled_name = format!("{}::{}", type_name, fn_name);
                            let mut merged_type_params = type_params.clone();
                            for tp in fn_type_params {
                                if !merged_type_params.contains(tp) {
                                    merged_type_params.push(tp.clone());
                                }
                            }
                            let generic_fn = crate::parser::Stmt {
                                kind: StmtKind::Fn {
                                    name: mangled_name.clone(),
                                    type_params: merged_type_params,
                                    params: params.clone(),
                                    return_type: return_type.clone(),
                                    body: fn_body.clone(),
                                    decorators: decorators.clone(),
                                    is_async: *is_async,
                                },
                                span: s.span,
                            };
                            self.generic_fns.insert(mangled_name, generic_fn);
                        }
                    }
                    return;
                }
                for s in body {
                    if let StmtKind::Fn {
                        name: fn_name,
                        type_params,
                        ..
                    } = &s.kind
                    {
                        if !type_params.is_empty() {
                            continue;
                        }
                        let mangled_name = format!("{}::{}", type_name, fn_name);
                        let mut impl_stmt = s.clone();
                        if let StmtKind::Fn {
                            name: ref mut n, ..
                        } = impl_stmt.kind
                        {
                            *n = mangled_name;
                        }
                        self.lower_fn_def(&impl_stmt);
                    }
                }
            }

            StmtKind::Assert { test, msg } => {
                let test_op = self.lower_expr(test);
                if let Some(m) = msg {
                    self.lower_expr(m);
                }
                let pass_bb = self.new_block();
                let fail_bb = self.new_block();
                if let Some(bb) = self.current_block {
                    self.terminate_block(
                        bb,
                        TerminatorKind::SwitchInt {
                            discr: test_op,
                            targets: vec![(1, pass_bb)],
                            otherwise: fail_bb,
                        },
                        test.span,
                    );
                }
                self.terminate_block(fail_bb, TerminatorKind::Unreachable, Span::default());
                self.current_block = Some(pass_bb);
            }

            StmtKind::Struct {
                name,
                fields,
                type_params,
                ..
            } => {
                if !type_params.is_empty() {
                    let init_name = format!("{}::__init__", name);
                    let mut params = vec![crate::parser::Param {
                        name: "self".to_string(),
                        type_ann: None,
                        is_mut: false,
                        default: None,
                        kind: crate::parser::ParamKind::Regular,
                        span: Span::default(),
                    }];
                    for f in fields {
                        params.push(crate::parser::Param {
                            name: f.name.clone(),
                            type_ann: f.type_ann.clone(),
                            is_mut: false,
                            default: None,
                            kind: crate::parser::ParamKind::Regular,
                            span: Span::default(),
                        });
                    }

                    self.generic_fns.insert(init_name, stmt.clone());
                    return;
                }
                if !fields.is_empty() {
                    let init_name = format!("{}::__init__", name);
                    let n_params = fields.len() + 1;

                    let saved_name = std::mem::take(&mut self.current_name);
                    let saved_locals = std::mem::take(&mut self.current_locals);
                    let saved_blocks = std::mem::take(&mut self.current_blocks);
                    let saved_block = self.current_block.take();
                    let saved_var_map = std::mem::take(&mut self.var_map);
                    let saved_loop_stack = std::mem::take(&mut self.loop_stack);
                    let saved_arg_count = self.current_arg_count;

                    self.start_function(init_name, n_params, Type::Null);

                    let self_local = self.new_local(
                        Type::Struct(name.clone(), Vec::new()),
                        Some("self".to_string()),
                        false,
                    );
                    let mut field_locals = Vec::new();
                    for field in fields {
                        let field_ty = field
                            .type_ann
                            .as_ref()
                            .map(|ann| self.resolve_type_expr(ann))
                            .unwrap_or(Type::Any);
                        let fl = self.new_local(field_ty, Some(field.name.clone()), false);
                        field_locals.push((field.name.clone(), fl));
                    }

                    for (field_name, fl) in &field_locals {
                        self.push_statement(
                            StatementKind::SetAttr(
                                Operand::Copy(self_local),
                                field_name.clone(),
                                Operand::Copy(*fl),
                            ),
                            Span::default(),
                        );
                    }

                    if let Some(bb) = self.current_block {
                        self.terminate_block(bb, TerminatorKind::Return, Span::default());
                    }

                    self.finish_function();

                    self.current_name = saved_name;
                    self.current_locals = saved_locals;
                    self.current_blocks = saved_blocks;
                    self.current_block = saved_block;
                    self.var_map = saved_var_map;
                    self.loop_stack = saved_loop_stack;
                    self.current_arg_count = saved_arg_count;
                }
            }

            StmtKind::Pass
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. }
            | StmtKind::NativeImport { .. } => {}

            StmtKind::UnsafeBlock(body) => {
                for s in body {
                    self.lower_stmt(s);
                }
            }

            StmtKind::Enum { name, variants, .. } => {
                for (i, variant) in variants.iter().enumerate() {
                    let mangled = format!("{}::{}", name, variant.name);
                    self.enum_variants.insert(mangled, (name.clone(), i));
                }
            }
        }
    }

    pub(super) fn lower_assign(
        &mut self,
        target: &crate::parser::Expr,
        value: &crate::parser::Expr,
    ) {
        let rval = self.lower_expr(value);
        match &target.kind {
            ExprKind::Identifier(name) => {
                if let Some(local) = self.lookup_var(name) {
                    self.push_statement(
                        StatementKind::Assign(local, Rvalue::Use(rval)),
                        target.span,
                    );
                }
            }
            ExprKind::Attr { obj, attr } => {
                let obj_op = self.lower_expr_as_copy(obj);
                self.push_statement(
                    StatementKind::SetAttr(obj_op, attr.clone(), rval),
                    target.span,
                );
            }
            ExprKind::Index { obj, index } => {
                let obj_op = self.lower_expr_as_copy(obj);
                let idx_op = self.lower_expr(index);
                self.push_statement(StatementKind::SetIndex(obj_op, idx_op, rval), target.span);
            }
            ExprKind::Deref(ptr_expr) => {
                let ptr_op = self.lower_expr(ptr_expr);
                self.push_statement(StatementKind::PtrStore(ptr_op, rval), target.span);
            }
            ExprKind::Tuple(elems) => {
                let rhs_local = self.new_tmp_for_expr(value);
                self.push_statement(
                    StatementKind::Assign(rhs_local, Rvalue::Use(rval)),
                    value.span,
                );
                for (i, elem) in elems.iter().enumerate() {
                    let idx_op = Operand::Constant(Constant::Int(i as i64));
                    let elem_tmp = self.new_tmp_for_expr(elem);
                    self.push_statement(
                        StatementKind::Assign(
                            elem_tmp,
                            Rvalue::GetIndex(Operand::Copy(rhs_local), idx_op),
                        ),
                        elem.span,
                    );
                    if let ExprKind::Identifier(name) = &elem.kind
                        && let Some(local) = self.lookup_var(name)
                    {
                        self.push_statement(
                            StatementKind::Assign(local, Rvalue::Use(Operand::Copy(elem_tmp))),
                            elem.span,
                        );
                    }
                }
            }
            _ => {
                let tmp = self.new_tmp_for_expr(target);
                self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(rval)), target.span);
            }
        }
    }

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
            if !self.fn_meta.contains_key(name) {
                self.register_fn_meta(name, params);
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
            let saved_arg_count = self.current_arg_count;
            let saved_is_async = self.current_is_async;
            self.current_is_async = *is_async;

            let ret_ty = return_type
                .as_ref()
                .map(|ann| self.resolve_type_expr(ann))
                .unwrap_or(Type::Any);

            self.start_function(name.clone(), params.len(), ret_ty);

            let mut param_locals = Vec::new();
            for param in params {
                let ty = param
                    .type_ann
                    .as_ref()
                    .map(|ann| self.resolve_type_expr(ann))
                    .unwrap_or(Type::Any);
                let ty = if param.name == "self" && name.contains("::") {
                    let struct_name = name.split("::").next().unwrap_or("");
                    if self.struct_fields.contains_key(struct_name) {
                        Type::Struct(struct_name.to_string(), Vec::new())
                    } else {
                        ty
                    }
                } else {
                    ty
                };
                let local = self.declare_var(param.name.clone(), ty, param.is_mut);
                param_locals.push(local);
            }

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
                    if i == body.len() - 1
                        && let StmtKind::ExprStmt(e) = &s.kind
                    {
                        let rval = self.lower_expr(e);
                        self.push_statement(
                            StatementKind::Assign(Local(0), Rvalue::Use(rval)),
                            e.span,
                        );
                        if let Some(bb) = self.current_block {
                            self.terminate_block(bb, TerminatorKind::Return, e.span);
                        }
                        self.current_block = Some(self.new_block());
                        continue;
                    }
                    self.lower_stmt(s);
                }

                if let Some(bb) = self.current_block {
                    self.terminate_block(bb, TerminatorKind::Return, Span::default());
                }
            }

            self.finish_function();

            self.current_name = saved_name;
            self.current_locals = saved_locals;
            self.current_blocks = saved_blocks;
            self.current_block = saved_block;
            self.var_map = saved_var_map;
            self.loop_stack = saved_loop_stack;
            self.current_arg_count = saved_arg_count;
            self.current_is_async = saved_is_async;
        }
    }

    pub(super) fn lower_fn_def_or_impl(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Fn { .. } => self.lower_fn_def(stmt),
            StmtKind::Impl {
                type_name, body, ..
            } => {
                let type_name = type_name.clone();
                let body = body.clone();
                for s in &body {
                    if let StmtKind::Fn { name: fn_name, .. } = &s.kind {
                        let mangled = format!("{}::{}", type_name, fn_name);
                        let mut impl_stmt = s.clone();
                        if let StmtKind::Fn {
                            name: ref mut n, ..
                        } = impl_stmt.kind
                        {
                            *n = mangled;
                        }
                        self.lower_fn_def(&impl_stmt);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn resolve_type_expr(&self, expr: &crate::parser::TypeExpr) -> Type {
        use crate::parser::TypeExprKind;
        match &expr.kind {
            TypeExprKind::Name(name) => match name.as_str() {
                "int" | "i64" => Type::Int,
                "i32" => Type::I32,
                "i16" => Type::I16,
                "i8" => Type::I8,
                "u64" => Type::U64,
                "u32" => Type::U32,
                "u16" => Type::U16,
                "u8" => Type::U8,
                "float" | "f64" => Type::Float,
                "f32" => Type::F32,
                "str" => Type::Str,
                "bool" => Type::Bool,
                "None" => Type::Null,
                "Any" => Type::Any,
                "Never" => Type::Never,
                _ => {
                    if let Some(Type::Enum(e, args)) = self.global_types.get(name) {
                        Type::Enum(e.clone(), args.clone())
                    } else {
                        Type::Struct(name.clone(), Vec::new())
                    }
                }
            },
            TypeExprKind::Generic(name, args) => match (name.as_str(), args.len()) {
                ("list", 1) => Type::List(Box::new(self.resolve_type_expr(&args[0]))),
                ("set", 1) => Type::Set(Box::new(self.resolve_type_expr(&args[0]))),
                ("dict", 2) => Type::Dict(
                    Box::new(self.resolve_type_expr(&args[0])),
                    Box::new(self.resolve_type_expr(&args[1])),
                ),
                _ => Type::Struct(name.clone(), Vec::new()),
            },
            TypeExprKind::List(inner) => Type::List(Box::new(self.resolve_type_expr(inner))),
            TypeExprKind::Dict(k, v) => Type::Dict(
                Box::new(self.resolve_type_expr(k)),
                Box::new(self.resolve_type_expr(v)),
            ),
            TypeExprKind::Tuple(types) => {
                Type::Tuple(types.iter().map(|t| self.resolve_type_expr(t)).collect())
            }
            TypeExprKind::Fn { params, ret } => Type::Fn(
                params.iter().map(|t| self.resolve_type_expr(t)).collect(),
                Box::new(self.resolve_type_expr(ret)),
                Vec::new(),
            ),
            TypeExprKind::Ref(inner) => Type::Ref(Box::new(self.resolve_type_expr(inner))),
            TypeExprKind::MutRef(inner) => Type::MutRef(Box::new(self.resolve_type_expr(inner))),
            TypeExprKind::Union(a, b) => {
                let ta = self.resolve_type_expr(a);
                let tb = self.resolve_type_expr(b);
                let mut vars = Vec::new();
                if let Type::Union(mut va) = ta {
                    vars.append(&mut va);
                } else {
                    vars.push(ta);
                }
                if let Type::Union(mut vb) = tb {
                    vars.append(&mut vb);
                } else {
                    vars.push(tb);
                }
                Type::Union(vars)
            }
            // Opaque in MIR: treat as Int
            TypeExprKind::Ptr(_) => Type::Int,
            TypeExprKind::FixedArray(_, _) => Type::List(Box::new(Type::Int)),
        }
    }

    pub(super) fn monomorphize(&mut self, name: &str, type_args: &[Type]) -> String {
        let generic_stmt = match self.generic_fns.get(name).cloned() {
            Some(s) => s,
            None => return name.to_string(),
        };

        let arg_str = type_args
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join("_")
            .replace("[", "_")
            .replace("]", "_")
            .replace(",", "_")
            .replace(" ", "")
            .replace("->", "_to_")
            .replace("(", "_")
            .replace(")", "_")
            .replace("&", "ref_")
            .replace("*", "ptr_")
            .replace("|", "_or_")
            .replace(":", "_");

        let mut specialized_name = format!("{}_{}", name, arg_str);
        if name.contains("::__init__") {
            let parts: Vec<&str> = name.split("::__init__").collect();
            specialized_name = format!("{}_{}::__init__", parts[0], arg_str);
        }

        if self.functions.iter().any(|f| f.name == specialized_name) {
            return specialized_name;
        }

        let mut specialized_stmt = generic_stmt.clone();
        match &mut specialized_stmt.kind {
            StmtKind::Fn {
                name: n,
                type_params: tp,
                params: p,
                return_type: rt,
                body: b,
                ..
            } => {
                let tp_clone = tp.clone();
                *n = specialized_name.clone();
                *tp = Vec::new();

                let mut type_map = HashMap::default();
                for (param_name, arg_ty) in tp_clone.iter().zip(type_args.iter()) {
                    type_map.insert(param_name.clone(), arg_ty.clone());
                }

                self.replace_types_in_fn(p, rt, b, &type_map);
            }
            StmtKind::Struct {
                name: n,
                type_params: tp,
                fields: f,
                ..
            } => {
                let tp_clone = tp.clone();
                // Monomorphize constructor and specialize fields
                let mut type_map = HashMap::default();
                for (param_name, arg_ty) in tp_clone.iter().zip(type_args.iter()) {
                    type_map.insert(param_name.clone(), arg_ty.clone());
                }

                for field in f {
                    if let Some(ann) = &mut field.type_ann {
                        self.replace_type_expr(ann, &type_map);
                    }
                }

                *n = specialized_name.clone().replace("::__init__", "");
                *tp = Vec::new();
            }
            _ => {}
        }

        self.lower_stmt(&specialized_stmt);
        specialized_name
    }

    fn replace_types_in_fn(
        &self,
        params: &mut [crate::parser::Param],
        ret: &mut Option<crate::parser::TypeExpr>,
        body: &mut [crate::parser::Stmt],
        type_map: &HashMap<String, Type>,
    ) {
        for p in params {
            if let Some(ann) = &mut p.type_ann {
                self.replace_type_expr(ann, type_map);
            }
        }
        if let Some(ann) = ret {
            self.replace_type_expr(ann, type_map);
        }
        for s in body {
            self.replace_types_in_stmt(s, type_map);
        }
    }

    fn replace_types_in_stmt(
        &self,
        stmt: &mut crate::parser::Stmt,
        type_map: &HashMap<String, Type>,
    ) {
        match &mut stmt.kind {
            StmtKind::Let {
                type_ann, value, ..
            } => {
                if let Some(ann) = type_ann {
                    self.replace_type_expr(ann, type_map);
                }
                self.replace_types_in_expr(value, type_map);
            }
            StmtKind::Const {
                type_ann, value, ..
            } => {
                if let Some(ann) = type_ann {
                    self.replace_type_expr(ann, type_map);
                }
                self.replace_types_in_expr(value, type_map);
            }
            StmtKind::ExprStmt(e) | StmtKind::Return(Some(e)) => {
                self.replace_types_in_expr(e, type_map)
            }
            StmtKind::Assign { target, value } => {
                self.replace_types_in_expr(target, type_map);
                self.replace_types_in_expr(value, type_map);
            }
            StmtKind::AugAssign { target, value, .. } => {
                self.replace_types_in_expr(target, type_map);
                self.replace_types_in_expr(value, type_map);
            }
            StmtKind::If {
                condition,
                then_body,
                elif_clauses,
                else_body,
            } => {
                self.replace_types_in_expr(condition, type_map);
                for s in then_body {
                    self.replace_types_in_stmt(s, type_map);
                }
                for (c, b) in elif_clauses {
                    self.replace_types_in_expr(c, type_map);
                    for s in b {
                        self.replace_types_in_stmt(s, type_map);
                    }
                }
                if let Some(eb) = else_body {
                    for s in eb {
                        self.replace_types_in_stmt(s, type_map);
                    }
                }
            }
            StmtKind::While {
                condition,
                body,
                else_body,
            } => {
                self.replace_types_in_expr(condition, type_map);
                for s in body {
                    self.replace_types_in_stmt(s, type_map);
                }
                if let Some(eb) = else_body {
                    for s in eb {
                        self.replace_types_in_stmt(s, type_map);
                    }
                }
            }
            StmtKind::For {
                iter,
                body,
                else_body,
                ..
            } => {
                self.replace_types_in_expr(iter, type_map);
                for s in body {
                    self.replace_types_in_stmt(s, type_map);
                }
                if let Some(eb) = else_body {
                    for s in eb {
                        self.replace_types_in_stmt(s, type_map);
                    }
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    fn replace_types_in_expr(
        &self,
        expr: &mut crate::parser::Expr,
        type_map: &HashMap<String, Type>,
    ) {
        match &mut expr.kind {
            crate::parser::ExprKind::BinOp { left, right, .. } => {
                self.replace_types_in_expr(left, type_map);
                self.replace_types_in_expr(right, type_map);
            }
            crate::parser::ExprKind::UnaryOp { operand, .. } => {
                self.replace_types_in_expr(operand, type_map)
            }
            crate::parser::ExprKind::Call { callee, args } => {
                self.replace_types_in_expr(callee, type_map);
                for arg in args {
                    match arg {
                        crate::parser::CallArg::Positional(e)
                        | crate::parser::CallArg::Keyword(_, e)
                        | crate::parser::CallArg::Splat(e)
                        | crate::parser::CallArg::KwSplat(e) => {
                            self.replace_types_in_expr(e, type_map)
                        }
                    }
                }
            }
            crate::parser::ExprKind::Index { obj, index } => {
                self.replace_types_in_expr(obj, type_map);
                self.replace_types_in_expr(index, type_map);
            }
            crate::parser::ExprKind::Attr { obj, .. } => self.replace_types_in_expr(obj, type_map),
            crate::parser::ExprKind::List(elems)
            | crate::parser::ExprKind::Tuple(elems)
            | crate::parser::ExprKind::Set(elems) => {
                for e in elems {
                    self.replace_types_in_expr(e, type_map);
                }
            }
            crate::parser::ExprKind::Dict(pairs) => {
                for (k, v) in pairs {
                    self.replace_types_in_expr(k, type_map);
                    self.replace_types_in_expr(v, type_map);
                }
            }
            _ => {}
        }
    }

    fn replace_type_expr(
        &self,
        ann: &mut crate::parser::TypeExpr,
        type_map: &HashMap<String, Type>,
    ) {
        use crate::parser::TypeExprKind;
        match &mut ann.kind {
            TypeExprKind::Name(name) => {
                if let Some(ty) = type_map.get(name) {
                    ann.kind = self.type_to_type_expr_kind(ty);
                }
            }
            TypeExprKind::Generic(_, args) => {
                for arg in args {
                    self.replace_type_expr(arg, type_map);
                }
            }
            TypeExprKind::List(inner) | TypeExprKind::Ref(inner) | TypeExprKind::MutRef(inner) => {
                self.replace_type_expr(inner, type_map)
            }
            TypeExprKind::Tuple(elems) => {
                for e in elems {
                    self.replace_type_expr(e, type_map);
                }
            }
            TypeExprKind::Fn { params, ret } => {
                for p in params {
                    self.replace_type_expr(p, type_map);
                }
                self.replace_type_expr(ret, type_map);
            }
            _ => {}
        }
    }

    fn type_to_type_expr_kind(&self, ty: &Type) -> crate::parser::TypeExprKind {
        use crate::parser::TypeExprKind;
        match ty {
            Type::Int => TypeExprKind::Name("int".to_string()),
            Type::Float => TypeExprKind::Name("float".to_string()),
            Type::Str => TypeExprKind::Name("str".to_string()),
            Type::Bool => TypeExprKind::Name("bool".to_string()),
            Type::Null => TypeExprKind::Name("None".to_string()),
            Type::Any => TypeExprKind::Name("Any".to_string()),
            Type::Never => TypeExprKind::Name("Never".to_string()),
            Type::List(inner) => TypeExprKind::List(Box::new(crate::parser::TypeExpr::new(
                self.type_to_type_expr_kind(inner),
                Span::default(),
            ))),
            Type::Struct(name, args) => {
                let type_args = args
                    .iter()
                    .map(|a| {
                        crate::parser::TypeExpr::new(
                            self.type_to_type_expr_kind(a),
                            Span::default(),
                        )
                    })
                    .collect();
                TypeExprKind::Generic(name.clone(), type_args)
            }
            _ => TypeExprKind::Name("Any".to_string()),
        }
    }
}
