use super::super::error::SemanticError;
use super::super::types::Type;
use super::TypeChecker;
use crate::parser::{ParamKind, Stmt, StmtKind};

fn ffi_type(t: Type) -> Type {
    match t {
        Type::I8 | Type::I16 | Type::I32 | Type::U8 | Type::U16 | Type::U32 => Type::Int,
        Type::Ref(inner) => Type::Ref(Box::new(ffi_type(*inner))),
        Type::MutRef(inner) => Type::MutRef(Box::new(ffi_type(*inner))),
        other => other,
    }
}

impl TypeChecker {
    pub(super) fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let {
                name,
                type_ann,
                value,
                is_mut,
            } => {
                let val_ty = self.check_expr(value);
                let declared_ty = type_ann.as_ref().map(|ann| self.resolve_type_expr(ann));
                let var_ty = if let Some(decl) = declared_ty {
                    self.unify(&decl, &val_ty, stmt.span);
                    decl
                } else {
                    val_ty
                };
                self.define_type(name, var_ty, *is_mut);
            }

            StmtKind::MultiLet {
                names,
                type_ann,
                value,
                is_mut,
                ..
            } => {
                let val_ty = self.check_expr(value);
                if let Type::Tuple(elem_tys) = val_ty {
                    if elem_tys.len() == names.len() {
                        for (name, ty) in names.iter().zip(elem_tys.into_iter()) {
                            if let Some(ann) = type_ann {
                                let expected_ty = self.resolve_type_expr(ann);
                                self.unify(&expected_ty, &ty, value.span);
                                self.define_type(name, expected_ty, *is_mut);
                            } else {
                                self.define_type(name, ty, *is_mut);
                            }
                        }
                    } else {
                        self.errors
                            .push(crate::semantic::error::SemanticError::Custom {
                                msg: "Tuple unpacking length mismatch".to_string(),
                                span: stmt.span,
                            });
                        for name in names {
                            self.define_type(name, Type::Any, *is_mut);
                        }
                    }
                } else {
                    self.errors
                        .push(crate::semantic::error::SemanticError::Custom {
                            msg: "Expected tuple for multiple variable declaration".to_string(),
                            span: stmt.span,
                        });
                    for name in names {
                        self.define_type(name, Type::Any, *is_mut);
                    }
                }
            }

            StmtKind::Const {
                name,
                type_ann,
                value,
            } => {
                let val_ty = self.check_expr(value);
                let declared_ty = type_ann.as_ref().map(|ann| self.resolve_type_expr(ann));
                let var_ty = if let Some(decl) = declared_ty {
                    self.unify(&decl, &val_ty, stmt.span);
                    decl
                } else {
                    val_ty
                };
                self.define_type(name, var_ty, false);
            }

            StmtKind::MultiConst {
                names,
                type_ann,
                value,
                ..
            } => {
                let val_ty = self.check_expr(value);
                if let Type::Tuple(elem_tys) = val_ty {
                    if elem_tys.len() == names.len() {
                        for (name, ty) in names.iter().zip(elem_tys.into_iter()) {
                            if let Some(ann) = type_ann {
                                let expected_ty = self.resolve_type_expr(ann);
                                self.unify(&expected_ty, &ty, value.span);
                                self.define_type(name, expected_ty, false);
                            } else {
                                self.define_type(name, ty, false);
                            }
                        }
                    } else {
                        self.errors
                            .push(crate::semantic::error::SemanticError::Custom {
                                msg: "Tuple unpacking length mismatch".to_string(),
                                span: stmt.span,
                            });
                        for name in names {
                            self.define_type(name, Type::Any, false);
                        }
                    }
                } else {
                    self.errors
                        .push(crate::semantic::error::SemanticError::Custom {
                            msg: "Expected tuple for multiple constant declaration".to_string(),
                            span: stmt.span,
                        });
                    for name in names {
                        self.define_type(name, Type::Any, false);
                    }
                }
            }

            StmtKind::ExprStmt(expr) => {
                self.check_expr(expr);
            }

            StmtKind::Assign { target, value } => {
                let val_ty = self.check_expr(value);
                let target_ty = self.check_expr(target);
                self.unify(&target_ty, &val_ty, stmt.span);

                if let crate::parser::ExprKind::Attr { obj, attr } = &target.kind {
                    let obj_ty = self.check_expr(obj);
                    let resolved_obj = self.apply_subst(obj_ty);
                    if let Type::Struct(struct_name, _) = resolved_obj {
                        self.field_types.insert((struct_name, attr.clone()), val_ty);
                    }
                }

                if let crate::parser::ExprKind::Identifier(name) = &target.kind
                    && !self.is_mutable(name)
                {
                    self.errors.push(SemanticError::Custom {
                        msg: format!(
                            "cannot reassign immutable variable `{}` (did you mean `let mut {}`?)",
                            name, name
                        ),
                        span: stmt.span,
                    });
                }
            }

            StmtKind::AugAssign { target, op, value } => {
                let val_ty = self.check_expr(value);
                let target_ty = self.check_expr(target);
                let result_ty = self.check_aug_op(op, &target_ty, &val_ty, stmt.span);
                self.unify(&target_ty, &result_ty, stmt.span);
            }

            StmtKind::If {
                condition,
                then_body,
                elif_clauses,
                else_body,
            } => {
                let cond_ty = self.check_expr(condition);
                self.expect_truthy(&cond_ty, stmt.span);
                self.check_block(then_body);
                for (cond, body) in elif_clauses {
                    let c_ty = self.check_expr(cond);
                    self.expect_truthy(&c_ty, cond.span);
                    self.check_block(body);
                }
                if let Some(body) = else_body {
                    self.check_block(body);
                }
            }

            StmtKind::Fn {
                name,
                type_params,
                params,
                return_type,
                body,
                is_async,
                ..
            } => {
                self.enter_scope();
                for tp in type_params {
                    self.define_type(tp, Type::Param(tp.clone()), false);
                }

                let inner_ret_ty = return_type
                    .as_ref()
                    .map(|ann| self.resolve_type_expr(ann))
                    .unwrap_or_else(|| self.fresh_var());
                let ret_ty = if *is_async {
                    Type::Future(Box::new(inner_ret_ty.clone()))
                } else {
                    inner_ret_ty.clone()
                };
                let mut param_types = Vec::with_capacity(params.len());
                for param in params {
                    let p_ty = param
                        .type_ann
                        .as_ref()
                        .map(|ann| self.resolve_type_expr(ann))
                        .unwrap_or_else(|| self.fresh_var());
                    param_types.push(p_ty);
                }

                let final_name = if let Some(struct_name) = &self.current_struct {
                    format!("{}::{}", struct_name, name)
                } else {
                    name.clone()
                };

                let mut all_type_params: Vec<Type> =
                    type_params.iter().map(|p| Type::Param(p.clone())).collect();

                if let Some(struct_name) = &self.current_struct
                    && let Some(Type::Struct(_, struct_args)) = self.lookup_type(struct_name)
                {
                    for arg in struct_args {
                        if !all_type_params.contains(&arg) {
                            all_type_params.push(arg);
                        }
                    }
                }

                let fn_ty = Type::Fn(
                    param_types.clone(),
                    Box::new(ret_ty.clone()),
                    all_type_params,
                );

                let outer_idx = 0;
                self.type_env[outer_idx].insert(final_name.clone(), fn_ty.clone());

                if final_name.ends_with("::__init__") {
                    let mut p_names = Vec::new();
                    for p in params.iter() {
                        if p.name != "self" {
                            p_names.push(p.name.clone());
                        }
                    }
                    self.init_params.insert(final_name.clone(), p_names);
                }

                if params
                    .iter()
                    .any(|p| matches!(p.kind, ParamKind::VarArg | ParamKind::KwArg))
                {
                    self.vararg_fns.insert(final_name.clone());
                }

                self.enter_scope();
                let prev_ret = self.current_return_type.take();
                self.current_return_type = Some(inner_ret_ty);
                if *is_async {
                    self.async_depth += 1;
                }

                for (i, (param, mut p_ty)) in params.iter().zip(param_types).enumerate() {
                    if i == 0 && self.current_struct.is_some() && param.name == "self" {
                        if param.type_ann.is_none() {
                            let struct_name = self.current_struct.clone().unwrap();
                            if let Some(struct_ty) = self.lookup_type(&struct_name) {
                                if let Type::Struct(_, args) = struct_ty {
                                    p_ty = Type::Struct(struct_name, args);
                                } else {
                                    p_ty = Type::Struct(struct_name, Vec::new());
                                }
                            } else {
                                p_ty = Type::Any;
                            }
                        }
                    }
                    self.define_type(&param.name, p_ty, param.is_mut);
                }

                for (i, s) in body.iter().enumerate() {
                    self.check_stmt(s);
                    if i == body.len() - 1
                        && let Some(expected) = self.current_return_type.clone()
                    {
                        self.check_tail_return(s, &expected);
                    }
                }

                if let Some(expected) = self.current_return_type.clone() {
                    let mut has_return = false;
                    for s in body {
                        if self.stmt_returns(s) {
                            has_return = true;
                            break;
                        }
                    }
                    if !has_return {
                        let resolved_expected = self.apply_subst(expected.clone());
                        if let Type::Var(_) = resolved_expected {
                            self.unify(&expected, &Type::Null, stmt.span);
                        } else if resolved_expected != Type::Null && resolved_expected != Type::Any
                        {
                            self.errors.push(SemanticError::Custom {
                                msg: format!(
                                    "type mismatch: expected `{}`, found `no return statement`",
                                    resolved_expected
                                ),
                                span: stmt.span,
                            });
                        }
                    }
                }

                if *is_async {
                    self.async_depth -= 1;
                }
                self.current_return_type = prev_ret;
                self.leave_scope();
                self.leave_scope();
            }

            StmtKind::While {
                condition,
                body,
                else_body,
            } => {
                let cond_ty = self.check_expr(condition);
                self.expect_truthy(&cond_ty, stmt.span);
                self.check_block(body);
                if let Some(body) = else_body {
                    self.check_block(body);
                }
            }

            StmtKind::For {
                target,
                iter,
                body,
                else_body,
            } => {
                let iter_ty = self.check_expr(iter);
                self.enter_scope();
                self.bind_for_target(target, &iter_ty, stmt.span);
                for s in body {
                    self.check_stmt(s);
                }
                self.leave_scope();
                if let Some(body) = else_body {
                    self.check_block(body);
                }
            }

            StmtKind::With { items, body } => {
                self.enter_scope();
                for item in items {
                    let ctx_ty = self.check_expr(&item.context_expr);
                    let resolved_ctx = self.apply_subst(ctx_ty.clone());

                    if let Type::Struct(name, _) = &resolved_ctx {
                        let enter_mangled = format!("{}::__enter__", name);
                        let exit_mangled = format!("{}::__exit__", name);

                        let enter_ret_ty = if let Some(enter_fn) = self.lookup_type(&enter_mangled)
                        {
                            if let Type::Fn(_, ret, _) = enter_fn {
                                *ret
                            } else {
                                Type::Any
                            }
                        } else {
                            self.errors
                                .push(crate::semantic::error::SemanticError::Custom {
                                    msg: format!("{} does not implement `__enter__`", name),
                                    span: item.context_expr.span,
                                });
                            Type::Any
                        };

                        if self.lookup_type(&exit_mangled).is_none() {
                            self.errors
                                .push(crate::semantic::error::SemanticError::Custom {
                                    msg: format!("{} does not implement `__exit__`", name),
                                    span: item.context_expr.span,
                                });
                        }

                        if let Some(alias_expr) = &item.alias {
                            if let crate::parser::ExprKind::Identifier(alias_name) =
                                &alias_expr.kind
                            {
                                self.define_type(alias_name, enter_ret_ty.clone(), false);
                                self.expr_types.insert(alias_expr.id, enter_ret_ty);
                            }
                        }
                    } else if resolved_ctx != Type::Any && resolved_ctx != Type::Null {
                        self.errors
                            .push(crate::semantic::error::SemanticError::Custom {
                                msg: format!(
                                    "type `{}` cannot be used as a context manager",
                                    resolved_ctx
                                ),
                                span: item.context_expr.span,
                            });
                        if let Some(alias_expr) = &item.alias {
                            if let crate::parser::ExprKind::Identifier(alias_name) =
                                &alias_expr.kind
                            {
                                self.define_type(alias_name, Type::Any, false);
                                self.expr_types.insert(alias_expr.id, Type::Any);
                            }
                        }
                    }
                }

                for s in body {
                    self.check_stmt(s);
                }
                self.leave_scope();
            }

            StmtKind::Struct {
                name,
                type_params,
                fields,
                body,
                ..
            } => {
                let abstract_args = type_params
                    .iter()
                    .map(|p| Type::Param(p.clone()))
                    .collect::<Vec<_>>();
                self.define_type(name, Type::Struct(name.clone(), abstract_args), false);
                self.struct_fields.insert(
                    name.clone(),
                    fields.iter().map(|f| f.name.clone()).collect(),
                );

                self.enter_scope();
                for tp in type_params {
                    self.define_type(tp, Type::Param(tp.clone()), false);
                }

                for field in fields {
                    let field_ty = field
                        .type_ann
                        .as_ref()
                        .map(|ann| self.resolve_type_expr(ann))
                        .unwrap_or(Type::Any);
                    self.field_types
                        .insert((name.clone(), field.name.clone()), field_ty);
                }

                let prev_struct = self.current_struct.take();
                self.current_struct = Some(name.clone());
                self.enter_scope();
                for s in body {
                    self.check_stmt(s);
                }
                self.leave_scope();
                self.current_struct = prev_struct;
                self.leave_scope();
            }

            StmtKind::Impl {
                type_params,
                trait_name,
                type_name,
                body,
            } => {
                self.enter_scope();
                for tp in type_params {
                    self.define_type(tp, Type::Param(tp.clone()), false);
                }

                let prev_struct = self.current_struct.take();
                self.current_struct = Some(type_name.to_string());
                self.enter_scope();
                for s in body {
                    self.check_stmt(s);
                }

                if let Some(tr) = trait_name {
                    if let Some(trait_def) = self.traits.get(&tr.to_string()).cloned() {
                        let mut provided = rustc_hash::FxHashMap::default();
                        for s in body {
                            if let StmtKind::Fn { name: fn_name, .. } = &s.kind {
                                let mangled = format!("{}::{}", type_name, fn_name);
                                if let Some(ty) = self.lookup_type(&mangled) {
                                    provided.insert(fn_name.clone(), ty);
                                }
                            }
                        }

                        for (method_name, required_ty) in &trait_def.methods {
                            if let Some(provided_ty) = provided.get(method_name) {
                                self.unify(required_ty, provided_ty, stmt.span);
                            } else {
                                self.errors.push(SemanticError::Custom {
                                    msg: format!(
                                        "`{}` does not implement `{}::{}` required by trait `{}`",
                                        type_name, type_name, method_name, tr
                                    ),
                                    span: stmt.span,
                                });
                            }
                        }
                        self.type_traits
                            .insert((type_name.to_string(), tr.to_string()));
                    } else {
                        self.errors.push(SemanticError::Custom {
                            msg: format!("undefined trait `{}`", tr),
                            span: stmt.span,
                        });
                    }
                }

                self.leave_scope();
                self.current_struct = prev_struct;
                self.leave_scope();
            }

            StmtKind::Trait {
                name,
                type_params,
                methods,
            } => {
                let prev_struct = self.current_struct.take();
                self.current_struct = Some(name.clone());

                self.enter_scope();
                for tp in type_params {
                    self.define_type(tp, Type::Param(tp.clone()), false);
                }

                let mut trait_methods = Vec::new();
                for s in methods {
                    self.check_stmt(s);
                    if let StmtKind::Fn { name: fn_name, .. } = &s.kind {
                        let mangled = format!("{}::{}", name, fn_name);
                        if let Some(ty) = self.lookup_type(&mangled) {
                            trait_methods.push((fn_name.clone(), ty));
                        }
                    }
                }

                self.leave_scope();
                self.current_struct = prev_struct;
                self.traits.insert(
                    name.clone(),
                    super::TraitDef {
                        type_params: type_params.clone(),
                        methods: trait_methods,
                    },
                );

                let abstract_args = type_params
                    .iter()
                    .map(|p| Type::Param(p.clone()))
                    .collect::<Vec<_>>();
                self.define_type(name, Type::TraitObject(name.clone(), abstract_args), false);
            }

            StmtKind::Return(Some(expr)) => {
                let ret_ty = self.check_expr(expr);
                if let Some(expected) = self.current_return_type.clone() {
                    self.unify(&expected, &ret_ty, stmt.span);
                }
            }

            StmtKind::Return(None) => {
                if let Some(expected) = self.current_return_type.clone() {
                    self.unify(&expected, &Type::Null, stmt.span);
                }
            }

            StmtKind::Assert { test, msg } => {
                let test_ty = self.check_expr(test);
                self.expect_truthy(&test_ty, stmt.span);
                if let Some(m) = msg {
                    self.check_expr(m);
                }
            }

            StmtKind::NativeImport {
                alias,
                functions,
                structs,
                vars,
                consts,
                block_safe,
                ..
            } => {
                self.define_type(alias, super::super::types::Type::Any, false);
                for sig in functions {
                    let param_types: Vec<Type> = sig
                        .params
                        .iter()
                        .map(|p| ffi_type(self.resolve_type_expr(&p.ty)))
                        .collect();
                    let ret_type = ffi_type(
                        sig.ret
                            .as_ref()
                            .map(|t| self.resolve_type_expr(t))
                            .unwrap_or(Type::Null),
                    );
                    let fn_type = Type::Fn(param_types, Box::new(ret_type), vec![]);
                    let mangled = format!("{}::{}", alias, sig.name);
                    self.define_type(&mangled, fn_type, false);
                    self.c_ffi_fns.insert(mangled.clone());
                    let is_safe = *block_safe || sig.decorators.iter().any(|d| d.name == "safe");
                    if !is_safe {
                        self.ffi_fns.insert(mangled.clone());
                    }
                    if sig.is_vararg {
                        self.vararg_fns.insert(mangled);
                    }
                }
                for s in structs {
                    let type_name = format!("{}::{}", alias, s.name);
                    self.define_type(&type_name, Type::Struct(type_name.clone(), vec![]), false);
                    self.c_ffi_structs.insert(type_name.clone());
                    for field in &s.fields {
                        let field_ty = ffi_type(self.resolve_type_expr(&field.ty));
                        self.field_types
                            .insert((type_name.clone(), field.name.clone()), field_ty);
                    }
                }
                for v in vars {
                    let mangled = format!("{}::{}", alias, v.name);
                    let fn_type = Type::Fn(vec![], Box::new(Type::Int), vec![]);
                    self.define_type(&mangled, fn_type, false);
                    if !block_safe {
                        self.ffi_fns.insert(mangled);
                    }
                }
                for c in consts {
                    let mangled = format!("{}::{}", alias, c.name);
                    self.define_type(&mangled, Type::Int, false);
                }
            }

            StmtKind::UnsafeBlock(body) => {
                self.unsafe_depth += 1;
                self.enter_scope();
                for s in body {
                    self.check_stmt(s);
                }
                self.leave_scope();
                self.unsafe_depth -= 1;
            }

            StmtKind::Pass
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. } => {}

            StmtKind::Defer(expr) => {
                self.check_expr(expr);
            }

            StmtKind::PyImport { alias, .. } => {
                self.define_type(alias, Type::PyObject, false);
            }

            StmtKind::Enum {
                name,
                type_params,
                variants,
                body,
                ..
            } => {
                let abstract_args = type_params
                    .iter()
                    .map(|p| Type::Param(p.clone()))
                    .collect::<Vec<_>>();
                self.define_type(name, Type::Enum(name.clone(), abstract_args.clone()), false);

                let prev_struct = self.current_struct.take();
                self.current_struct = Some(name.clone());

                self.enter_scope();
                for tp in type_params {
                    self.define_type(tp, Type::Param(tp.clone()), false);
                }

                let mut variant_data = Vec::new();
                for variant in variants {
                    if let Some(val) = &variant.value {
                        let val_ty = self.check_expr(val);
                        self.unify(&Type::Int, &val_ty, stmt.span);
                    }
                    let mut param_types = Vec::new();
                    for ty_expr in &variant.types {
                        param_types.push(self.resolve_type_expr(ty_expr));
                    }
                    variant_data.push((variant.name.clone(), param_types));
                }

                for s in body {
                    self.check_stmt(s);
                }

                self.leave_scope();
                self.current_struct = prev_struct;

                let mut variant_names = Vec::new();
                for (v_name, p_types) in variant_data {
                    variant_names.push(v_name.clone());
                    let fn_ty = Type::Fn(
                        p_types,
                        Box::new(Type::Enum(name.clone(), abstract_args.clone())),
                        abstract_args.clone(),
                    );
                    let variant_mangled = format!("{}::{}", name, v_name);
                    self.define_type(&variant_mangled, fn_ty.clone(), false);
                    self.define_type(&v_name, fn_ty, false);
                }
                self.enum_variants.insert(name.clone(), variant_names);
            }
        }
    }
}

impl TypeChecker {
    pub(super) fn check_tail_return(&mut self, stmt: &Stmt, expected: &Type) {
        match &stmt.kind {
            StmtKind::ExprStmt(expr) => {
                if let Some(last_ty) = self.expr_types.get(&expr.id).cloned() {
                    self.unify(expected, &last_ty, stmt.span);
                }
            }
            StmtKind::If {
                then_body,
                elif_clauses,
                else_body,
                ..
            } => {
                if let Some(last) = then_body.last() {
                    self.check_tail_return(last, expected);
                }
                for (_, body) in elif_clauses {
                    if let Some(last) = body.last() {
                        self.check_tail_return(last, expected);
                    }
                }
                if let Some(body) = else_body
                    && let Some(last) = body.last()
                {
                    self.check_tail_return(last, expected);
                }
            }
            StmtKind::UnsafeBlock(body) => {
                if let Some(last) = body.last() {
                    self.check_tail_return(last, expected);
                }
            }
            _ => {}
        }
    }

    pub(super) fn stmt_returns(&self, stmt: &Stmt) -> bool {
        match &stmt.kind {
            StmtKind::Return(_) => true,
            StmtKind::If {
                then_body,
                elif_clauses,
                else_body,
                ..
            } => {
                let then_ret = then_body.iter().any(|s| self.stmt_returns(s));
                let elifs_ret = elif_clauses
                    .iter()
                    .all(|(_, body)| body.iter().any(|s| self.stmt_returns(s)));
                let else_ret = else_body
                    .as_ref()
                    .is_some_and(|body| body.iter().any(|s| self.stmt_returns(s)));
                then_ret && elifs_ret && else_ret
            }
            StmtKind::UnsafeBlock(body) => body.iter().any(|s| self.stmt_returns(s)),
            StmtKind::ExprStmt(_) => true,
            _ => false,
        }
    }
}
