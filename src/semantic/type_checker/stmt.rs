use super::super::error::SemanticError;
use super::super::pyi;
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
    /// Records an E0421: a type used at the C FFI boundary has no C layout.
    fn push_ffi_unsafe(&mut self, message: String, span: crate::span::Span, reason: &str) {
        self.errors.push(SemanticError::rich(
            crate::compile::errors::Diagnostic::error("E0421", message, span)
                .label("not representable in C")
                .note(reason)
                .help("use a scalar, a `str`, a C struct, or a raw `ptr`"),
        ));
    }

    pub(super) fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let {
                name,
                type_ann,
                value,
                is_mut,
                ..
            } => {
                let declared_ty = type_ann.as_ref().map(|ann| self.resolve_type_expr(ann));
                let var_ty = if let Some(decl) = declared_ty {
                    let val_ty = self.check_expr_expecting(value, &decl);
                    self.unify(&decl, &val_ty, value.span);
                    decl
                } else {
                    self.check_expr(value)
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
                let val_ty_raw = self.check_expr(value);
                let val_ty = self.apply_subst(val_ty_raw);
                if let Type::Tuple(elem_tys) = val_ty.clone() {
                    if elem_tys.len() == names.len() {
                        for (name, ty) in names.iter().zip(elem_tys) {
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
                            .push(crate::semantic::error::SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0417",
                                    "tuple unpacking length mismatch",
                                    stmt.span,
                                )
                                .label(format!("{} name(s) bound here", names.len()))
                                .help("bind exactly as many names as the tuple has elements"),
                            ));
                        for name in names {
                            self.define_type(name, Type::Any, *is_mut);
                        }
                    }
                } else {
                    if !matches!(val_ty, Type::PyObject | Type::Any) {
                        self.errors
                            .push(crate::semantic::error::SemanticError::rich(
                            crate::compile::errors::Diagnostic::error(
                                "E0417",
                                "cannot destructure a non-tuple value",
                                stmt.span,
                            )
                            .label(format!("this value is `{val_ty}`, not a tuple"))
                            .help(
                                "multiple-variable bindings require a tuple on the right-hand side",
                            ),
                        ));
                    }
                    for name in names {
                        self.define_type(name, Type::Any, *is_mut);
                    }
                }
            }

            StmtKind::Const {
                name,
                type_ann,
                value,
                ..
            } => {
                let declared_ty = type_ann.as_ref().map(|ann| self.resolve_type_expr(ann));
                let var_ty = if let Some(decl) = declared_ty {
                    let val_ty = self.check_expr_expecting(value, &decl);
                    self.unify(&decl, &val_ty, value.span);
                    decl
                } else {
                    self.check_expr(value)
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
                        for (name, ty) in names.iter().zip(elem_tys) {
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
                            .push(crate::semantic::error::SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0417",
                                    "tuple unpacking length mismatch",
                                    stmt.span,
                                )
                                .label(format!("{} name(s) bound here", names.len()))
                                .help("bind exactly as many names as the tuple has elements"),
                            ));
                        for name in names {
                            self.define_type(name, Type::Any, false);
                        }
                    }
                } else {
                    self.errors
                        .push(crate::semantic::error::SemanticError::rich(
                            crate::compile::errors::Diagnostic::error(
                                "E0417",
                                "cannot destructure a non-tuple value",
                                stmt.span,
                            )
                            .label("this value is not a tuple")
                            .help(
                                "multiple-constant bindings require a tuple on the right-hand side",
                            ),
                        ));
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
                        let existing = self
                            .field_types
                            .get(&(struct_name.clone(), attr.clone()))
                            .cloned();
                        if existing.is_none() || existing == Some(Type::Any) {
                            self.field_types.insert((struct_name, attr.clone()), val_ty);
                        }
                    }
                }

                if let crate::parser::ExprKind::Identifier(name) = &target.kind
                    && !self.is_mutable(name)
                {
                    self.errors.push(SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0410",
                            format!("cannot assign twice to immutable variable `{name}`"),
                            stmt.span,
                        )
                        .label("cannot reassign")
                        .help(format!(
                            "make it mutable at its declaration: `let mut {name} = ...`"
                        )),
                    ));
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
                    let mut p_ty = param
                        .type_ann
                        .as_ref()
                        .map(|ann| self.resolve_type_expr(ann))
                        .unwrap_or_else(|| self.fresh_var());

                    if param.name == "self"
                        && param.type_ann.is_none()
                        && let Some(struct_name) = &self.current_struct
                        && let Some(s_ty) = self.lookup_type(struct_name)
                    {
                        p_ty = s_ty;
                    }

                    // `*args: T` collects positionals into a `[T]` and
                    // `**kwargs: T` collects keywords into a `dict[str, T]`, so
                    // each answers the right collection operations.
                    match param.kind {
                        ParamKind::VarArg => p_ty = Type::List(Box::new(p_ty)),
                        ParamKind::KwArg => p_ty = Type::Dict(Box::new(Type::Str), Box::new(p_ty)),
                        ParamKind::Regular => {}
                    }

                    if let Some(default_expr) = &param.default {
                        let default_ty = self.check_expr(default_expr);
                        self.unify(&p_ty, &default_ty, param.span);
                    }

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
                    if i == 0
                        && self.current_struct.is_some()
                        && param.name == "self"
                        && param.type_ann.is_none()
                    {
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
                        } else if return_type.is_none() {
                            self.unify(&expected, &Type::Null, stmt.span);
                        } else if resolved_expected != Type::Null && resolved_expected != Type::Any
                        {
                            self.errors.push(SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0407",
                                    "missing return value",
                                    stmt.span,
                                )
                                .label(format!("this function must return `{resolved_expected}`"))
                                .note("control reaches the end of the function without a `return`")
                                .help(format!("add `return <{resolved_expected}>` on every path")),
                            ));
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
                            self.errors.push(crate::semantic::error::SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0407",
                                    format!("`{name}` is not a context manager"),
                                    item.context_expr.span,
                                )
                                .label("missing `__enter__` method")
                                .help(format!("implement `fn __enter__(self)` on `{name}` to use it in a `with`")),
                            ));
                            Type::Any
                        };

                        if self.lookup_type(&exit_mangled).is_none() {
                            self.errors.push(crate::semantic::error::SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0407",
                                    format!("`{name}` is not a context manager"),
                                    item.context_expr.span,
                                )
                                .label("missing `__exit__` method")
                                .help(format!("implement `fn __exit__(self)` on `{name}` to use it in a `with`")),
                            ));
                        }

                        if let Some(alias_expr) = &item.alias
                            && let crate::parser::ExprKind::Identifier(alias_name) =
                                &alias_expr.kind
                        {
                            self.define_type(alias_name, enter_ret_ty.clone(), false);
                            self.expr_types.insert(alias_expr.id, enter_ret_ty);
                        }
                    } else if resolved_ctx != Type::Any
                        && resolved_ctx != Type::Null
                        && resolved_ctx != Type::PyObject
                    {
                        self.errors
                            .push(crate::semantic::error::SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0407",
                                    format!("`{resolved_ctx}` cannot be used as a context manager"),
                                    item.context_expr.span,
                                )
                                .label("not usable in a `with` statement")
                                .note(
                                    "a context manager must define both `__enter__` and `__exit__`",
                                ),
                            ));
                        if let Some(alias_expr) = &item.alias
                            && let crate::parser::ExprKind::Identifier(alias_name) =
                                &alias_expr.kind
                        {
                            self.define_type(alias_name, Type::Any, false);
                            self.expr_types.insert(alias_expr.id, Type::Any);
                        }
                    } else if resolved_ctx == Type::PyObject
                        && let Some(alias_expr) = &item.alias
                        && let crate::parser::ExprKind::Identifier(alias_name) = &alias_expr.kind
                    {
                        self.define_type(alias_name, Type::PyObject, false);
                        self.expr_types.insert(alias_expr.id, Type::PyObject);
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
                let required = fields.iter().filter(|f| f.default.is_none()).count();
                self.struct_required_fields.insert(name.clone(), required);
                if fields
                    .iter()
                    .enumerate()
                    .any(|(i, f)| f.default.is_some() && i < required)
                {
                    self.errors.push(super::super::error::SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0413",
                            format!("`{name}` has a field with a default before one without"),
                            stmt.span,
                        )
                        .label("fields with defaults must come last")
                        .help("move every field that has a default value to the end of the struct"),
                    ));
                }

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
                // Use the bare type name (e.g. `Box`, not `Box[T]`) so methods
                // can look up the struct's type parameters and carry them as
                // their own generic arguments.
                let base_type_name = match &type_name.kind {
                    crate::parser::TypeExprKind::Generic(n, _) => n.clone(),
                    crate::parser::TypeExprKind::Name(n) => n.clone(),
                    _ => type_name.to_string(),
                };
                self.current_struct = Some(base_type_name);
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
                                let mut expected_ty = required_ty.clone();
                                if let Type::Fn(params, ret, args) = expected_ty {
                                    let mut new_params = params.clone();
                                    if !new_params.is_empty() {
                                        let first_name = match &new_params[0] {
                                            Type::TraitObject(n, _) => Some(n.clone()),
                                            Type::Struct(n, _) => Some(n.clone()),
                                            _ => None,
                                        };
                                        if first_name == Some(tr.to_string())
                                            && let Some(impl_ty) =
                                                self.lookup_type(&type_name.to_string())
                                        {
                                            new_params[0] = impl_ty;
                                        }
                                    }
                                    expected_ty = Type::Fn(new_params, ret, args);
                                }
                                self.unify(&expected_ty, provided_ty, stmt.span);
                            } else {
                                self.errors.push(SemanticError::rich(
                                    crate::compile::errors::Diagnostic::error(
                                        "E0415",
                                        format!("`{type_name}` does not satisfy trait `{tr}`"),
                                        stmt.span,
                                    )
                                    .label(format!("missing method `{method_name}`"))
                                    .help(format!(
                                        "implement `fn {method_name}(...)` in `impl {tr} for {type_name}`"
                                    )),
                                ));
                            }
                        }
                        self.type_traits
                            .insert((type_name.to_string(), tr.to_string()));
                    } else {
                        let tr_name = tr.to_string();
                        let suggestions = super::super::suggest::closest_n(
                            &tr_name,
                            self.traits.keys().map(String::as_str),
                            3,
                        );
                        self.errors.push(SemanticError::rich(
                            crate::compile::errors::Diagnostic::error(
                                "E0416",
                                format!("cannot find trait `{tr_name}` in this scope"),
                                stmt.span,
                            )
                            .label("not a trait")
                            .suggest_names(&suggestions),
                        ));
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

                // Within its own default methods a trait satisfies itself, so a
                // default may call a sibling method through `self`.
                self.type_traits.insert((name.clone(), name.clone()));

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
                let ret_ty = match self.current_return_type.clone() {
                    Some(expected) => {
                        let ret_ty = self.check_expr_expecting(expr, &expected);
                        self.unify(&expected, &ret_ty, stmt.span);
                        ret_ty
                    }
                    None => self.check_expr(expr),
                };
                let _ = ret_ty;
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
                    for p in &sig.params {
                        let resolved = self.resolve_type_expr(&p.ty);
                        if let Some(reason) = super::super::abi::ffi_unsafe_reason(&resolved) {
                            self.push_ffi_unsafe(
                                format!(
                                    "parameter of `{}` has type `{resolved}`, which cannot cross the FFI boundary",
                                    sig.name
                                ),
                                sig.span,
                                reason,
                            );
                        }
                    }
                    let param_types: Vec<Type> = sig
                        .params
                        .iter()
                        .map(|p| ffi_type(self.resolve_type_expr(&p.ty)))
                        .collect();
                    let resolved_ret = sig
                        .ret
                        .as_ref()
                        .map(|t| self.resolve_type_expr(t))
                        .unwrap_or(Type::Null);
                    if let Some(reason) = super::super::abi::ffi_unsafe_reason(&resolved_ret) {
                        self.push_ffi_unsafe(
                            format!(
                                "`{}` returns `{resolved_ret}`, which cannot cross the FFI boundary",
                                sig.name
                            ),
                            sig.span,
                            reason,
                        );
                    }
                    let ret_type = ffi_type(resolved_ret);
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
                        let resolved = self.resolve_type_expr(&field.ty);
                        if let Some(reason) = super::super::abi::ffi_unsafe_reason(&resolved) {
                            self.push_ffi_unsafe(
                                format!(
                                    "field `{}` of C struct `{}` has type `{resolved}`, which has no C layout",
                                    field.name, s.name
                                ),
                                stmt.span,
                                reason,
                            );
                        }
                        let field_ty = ffi_type(resolved);
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

            StmtKind::PyImport {
                alias,
                typed_types,
                typed_fns,
                module,
                ..
            } => {
                self.define_type(alias, Type::PyObject, false);
                self.py_aliases.insert(alias.clone());
                self.py_alias_module.insert(alias.clone(), module.clone());

                // Prefer explicit stub block; fall back to .pyi introspection.
                if !typed_types.is_empty() || !typed_fns.is_empty() {
                    self.py_explicit_modules.insert(alias.clone());
                    for type_name in typed_types {
                        let named_ty = Type::PyNamed(alias.clone(), type_name.clone());
                        self.py_module_types
                            .entry(alias.clone())
                            .or_default()
                            .insert(type_name.clone(), named_ty.clone());
                        self.define_type(type_name, named_ty, false);
                    }
                    for sig in typed_fns {
                        let param_tys: Vec<Type> = sig
                            .params
                            .iter()
                            .map(|p| self.resolve_type_expr(p))
                            .collect();
                        let ret_ty = sig
                            .ret
                            .as_ref()
                            .map(|r| self.resolve_type_expr(r))
                            .unwrap_or(Type::PyObject);
                        self.py_module_fns
                            .entry(alias.clone())
                            .or_default()
                            .entry(sig.name.clone())
                            .or_default()
                            .push((param_tys.clone(), ret_ty.clone()));
                        let arity = param_tys.len();
                        self.py_fn_arity
                            .entry(alias.clone())
                            .or_default()
                            .entry(sig.name.clone())
                            .or_default()
                            .push((arity, Some(arity)));
                        let mangled = format!("{}::{}", alias, sig.name);
                        if self.lookup_type(&mangled).is_none() {
                            self.define_type(
                                &mangled,
                                Type::Fn(param_tys, Box::new(ret_ty), vec![]),
                                false,
                            );
                        }
                    }
                } else {
                    match pyi::query(module) {
                        pyi::PyiOutcome::Found(info) => self.register_pyi(alias, info),
                        pyi::PyiOutcome::NoStub => {}
                        pyi::PyiOutcome::ModuleNotFound => {
                            self.errors.push(SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0600",
                                    format!("Python module `{module}` cannot be imported"),
                                    stmt.span,
                                )
                                .label("module not found by the active interpreter")
                                .note("Olive introspects Python modules at compile time to type-check their use")
                                .help(format!(
                                    "install it (e.g. `pip install {module}`), or add an explicit `type`/`fn` stub block"
                                )),
                            ));
                        }
                        pyi::PyiOutcome::Python3Missing => {
                            self.warnings.push(SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "W0601",
                                    format!("`{module}` could not be introspected"),
                                    stmt.span,
                                )
                                .label("`python3` was not found on PATH")
                                .note("calls into this module fall back to dynamic typing")
                                .help("add an explicit `type`/`fn` stub block to recover static checks"),
                            ));
                        }
                        pyi::PyiOutcome::InspectorError(detail) => {
                            self.warnings.push(SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "W0602",
                                    format!("could not introspect Python module `{module}`"),
                                    stmt.span,
                                )
                                .label("introspection failed")
                                .note(detail.clone())
                                .note("calls into this module fall back to dynamic typing")
                                .help("add an explicit `type`/`fn` stub block to recover static checks"),
                            ));
                        }
                    }
                }
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
                // Only an `if` with an `else` can yield a value on every path, so
                // only then is it an implicit return. A bare `if` (no `else`) is
                // control flow and must not be checked as a returned value.
                let Some(else_last) = else_body.as_ref().and_then(|b| b.last()) else {
                    return;
                };
                if let Some(last) = then_body.last() {
                    self.check_tail_return(last, expected);
                }
                for (_, body) in elif_clauses {
                    if let Some(last) = body.last() {
                        self.check_tail_return(last, expected);
                    }
                }
                self.check_tail_return(else_last, expected);
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

#[cfg(test)]
mod tests {
    use super::super::super::TypeChecker;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::semantic::Resolver;

    fn typeck(src: &str) -> TypeChecker {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        let mut tc = TypeChecker::new();
        tc.check_program(&prog);
        tc
    }

    #[test]
    fn const_declaration() {
        let tc = typeck("const PI = 3\nlet r = PI * 2\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn multi_let_tuple_destructuring() {
        let tc = typeck("let a, b = 1, 2\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn multi_let_length_mismatch_reported() {
        let tc = typeck("let a, b = 1\n");
        assert!(!tc.errors.is_empty());
    }

    #[test]
    fn multi_const_ok() {
        let tc = typeck("const X, Y = 1, 2\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn assert_with_truthy_expr() {
        let tc = typeck("assert True\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn assert_with_msg() {
        let tc = typeck("assert True, \"should pass\"\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn for_loop_with_tuple_target() {
        let tc = typeck("for (k, v) in [(1, 2)]:\n    pass\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn defer_expr() {
        let tc = typeck("defer print(42)\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn unsafe_block_ok() {
        let tc = typeck("unsafe:\n    let x = 42\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn pass_stmt() {
        let tc = typeck("pass\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn enum_with_explicit_values() {
        let tc = typeck("enum Color:\n    Red = 0\n    Green = 1\n    Blue = 2\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn while_loop_ok() {
        let tc = typeck("let mut i = 0\nwhile i < 10:\n    i = i + 1\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn break_continue_in_loop() {
        let tc =
            typeck("let mut i = 0\nwhile i < 10:\n    if i == 5:\n        break\n    i = i + 1\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn nested_if_else_ok() {
        let tc = typeck(
            "let x = 1\nif x > 0:\n    if x > 5:\n        let y = 1\n    else:\n        let y = 2\n",
        );
        assert!(tc.errors.is_empty());
    }
}
