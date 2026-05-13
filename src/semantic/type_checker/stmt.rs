use super::super::error::SemanticError;
use super::super::types::Type;
use super::TypeChecker;
use crate::parser::{ParamKind, Stmt, StmtKind};

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
                    if let Type::Struct(struct_name) = resolved_obj {
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
                params,
                return_type,
                body,
                is_async,
                ..
            } => {
                let inner_ret_ty = return_type
                    .as_ref()
                    .map(|ann| self.resolve_type_expr(ann))
                    .unwrap_or_else(Type::new_var);
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
                        .unwrap_or_else(Type::new_var);
                    param_types.push(p_ty);
                }

                let final_name = if let Some(struct_name) = &self.current_struct {
                    format!("{}::{}", struct_name, name)
                } else {
                    name.clone()
                };

                let fn_ty = Type::Fn(param_types.clone(), Box::new(ret_ty.clone()));
                if self.current_struct.is_some() && self.type_env.len() >= 2 {
                    let outer_idx = self.type_env.len() - 2;
                    self.type_env[outer_idx].insert(final_name.clone(), fn_ty.clone());
                } else {
                    self.define_type(&final_name, fn_ty.clone(), false);
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
                        p_ty = Type::Struct(self.current_struct.clone().unwrap());
                    }
                    self.define_type(&param.name, p_ty, param.is_mut);
                }

                for (i, s) in body.iter().enumerate() {
                    self.check_stmt(s);
                    if i == body.len() - 1
                        && let StmtKind::ExprStmt(e) = &s.kind
                        && let Some(last_ty) = self.expr_types.get(&e.id).cloned()
                        && let Some(expected) = self.current_return_type.clone()
                    {
                        self.unify(&expected, &last_ty, s.span);
                    }
                }

                if *is_async {
                    self.async_depth -= 1;
                }
                self.current_return_type = prev_ret;
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

            StmtKind::Struct {
                name, fields, body, ..
            } => {
                self.define_type(name, Type::Struct(name.clone()), false);

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
            }

            StmtKind::Impl {
                trait_name,
                type_name,
                body,
            } => {
                if let Some(tr) = trait_name {
                    if let Some(required) = self.traits.get(tr).cloned() {
                        let provided: rustc_hash::FxHashSet<String> = body
                            .iter()
                            .filter_map(|s| {
                                if let StmtKind::Fn { name, .. } = &s.kind {
                                    Some(name.clone())
                                } else {
                                    None
                                }
                            })
                            .collect();
                        for method in &required {
                            if !provided.contains(method) {
                                self.errors.push(SemanticError::Custom {
                                    msg: format!(
                                        "`{}` does not implement `{}::{}` required by trait `{}`",
                                        type_name, type_name, method, tr
                                    ),
                                    span: stmt.span,
                                });
                            }
                        }
                        self.type_traits.insert((type_name.clone(), tr.clone()));
                    } else {
                        self.errors.push(SemanticError::Custom {
                            msg: format!("undefined trait `{}`", tr),
                            span: stmt.span,
                        });
                    }
                }
                let prev_struct = self.current_struct.take();
                self.current_struct = Some(type_name.clone());
                self.enter_scope();
                for s in body {
                    self.check_stmt(s);
                }
                self.leave_scope();
                self.current_struct = prev_struct;
            }

            StmtKind::Trait { name, methods } => {
                let method_names: Vec<String> = methods
                    .iter()
                    .filter_map(|s| {
                        if let StmtKind::Fn { name, .. } = &s.kind {
                            Some(name.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                self.traits.insert(name.clone(), method_names);
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

            StmtKind::Pass
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. } => {}

            StmtKind::Enum { name, variants, .. } => {
                self.define_type(name, Type::Enum(name.clone()), false);
                let mut variant_names = Vec::new();
                for variant in variants {
                    variant_names.push(variant.name.clone());
                    let mut param_types = Vec::new();
                    for ty_expr in &variant.types {
                        param_types.push(self.resolve_type_expr(ty_expr));
                    }
                    let fn_ty = Type::Fn(param_types, Box::new(Type::Enum(name.clone())));
                    let variant_mangled = format!("{}::{}", name, variant.name);
                    self.define_type(&variant_mangled, fn_ty, false);
                }
                self.enum_variants.insert(name.clone(), variant_names);
            }
        }
    }
}
