use super::super::types::Type;
use super::TypeChecker;
use crate::parser::{AugOp, BinOp, CallArg, Expr, ExprKind, UnaryOp};
use crate::span::Span;

impl TypeChecker {
    pub(super) fn check_expr(&mut self, expr: &Expr) -> Type {
        let ty = self.infer_expr(expr);
        let final_ty = self.apply_subst(ty);
        self.expr_types.insert(expr.id, final_ty.clone());
        final_ty
    }

    pub(super) fn infer_expr(&mut self, expr: &Expr) -> Type {
        match &expr.kind {
            ExprKind::Integer(_) => Type::IntegerLiteral(self.fresh_var_id()),
            ExprKind::Float(_) => Type::FloatLiteral(self.fresh_var_id()),
            ExprKind::Str(_) => Type::Str,
            ExprKind::FStr(exprs) => {
                for e in exprs {
                    self.check_expr(e);
                }
                Type::Str
            }
            ExprKind::Bool(_) => Type::Bool,

            ExprKind::Deref(inner) => {
                let inner_ty = self.check_expr(inner);
                let resolved = self.apply_subst(inner_ty.clone());
                match resolved {
                    Type::Ptr(_) => {
                        if self.unsafe_depth == 0 {
                            self.errors
                                .push(super::super::error::SemanticError::Custom {
                                    msg: "pointer dereference requires unsafe block".into(),
                                    span: expr.span,
                                });
                        }
                        Type::Int
                    }
                    Type::List(elem_ty) => *elem_ty,
                    Type::PyObject | Type::Any | Type::Tuple(_) => Type::Any,
                    _ => {
                        if self.unsafe_depth == 0 {
                            self.errors
                                .push(super::super::error::SemanticError::Custom {
                                    msg: "pointer dereference requires unsafe block".into(),
                                    span: expr.span,
                                });
                        }
                        Type::Int
                    }
                }
            }
            ExprKind::Borrow(inner) => {
                let inner_ty = self.check_expr(inner);
                Type::Ref(Box::new(inner_ty))
            }
            ExprKind::MutBorrow(inner) => {
                let inner_ty = self.check_expr(inner);
                if let ExprKind::Identifier(name) = &inner.kind
                    && !self.is_mutable(name)
                {
                    self.errors
                        .push(super::super::error::SemanticError::Custom {
                            msg: format!("cannot mutably borrow immutable variable `{}`", name),
                            span: expr.span,
                        });
                }
                Type::MutRef(Box::new(inner_ty))
            }
            ExprKind::Identifier(name) => match self.lookup_type(name) {
                Some(t) => t,
                None => self.fresh_var(),
            },

            ExprKind::Cast(operand, type_expr) => {
                self.check_expr(operand);
                self.resolve_type_expr(type_expr)
            }

            ExprKind::BinOp { left, op, right } => {
                let l_ty = self.check_expr(left);
                let r_ty = self.check_expr(right);
                self.check_binop(op, &l_ty, &r_ty, expr.span)
            }

            ExprKind::UnaryOp { op, operand } => {
                let o_ty = self.check_expr(operand);
                match op {
                    UnaryOp::Not => Type::Bool,
                    UnaryOp::Neg | UnaryOp::Pos | UnaryOp::BitNot => o_ty,
                }
            }

            ExprKind::List(elems) => {
                let elem_ty = self.fresh_var();
                for e in elems {
                    let e_ty = self.check_expr(e);
                    self.unify(&elem_ty, &e_ty, expr.span);
                }
                Type::List(Box::new(self.apply_subst(elem_ty)))
            }

            ExprKind::Tuple(elems) => {
                let types: Vec<Type> = elems.iter().map(|e| self.check_expr(e)).collect();
                Type::Tuple(types)
            }

            ExprKind::Set(elems) => {
                let elem_ty = self.fresh_var();
                for e in elems {
                    let e_ty = self.check_expr(e);
                    self.unify(&elem_ty, &e_ty, expr.span);
                }
                Type::Set(Box::new(self.apply_subst(elem_ty)))
            }

            ExprKind::Dict(pairs) => {
                let k_ty = self.fresh_var();
                let v_ty = self.fresh_var();
                for (k, v) in pairs {
                    let kt = self.check_expr(k);
                    let vt = self.check_expr(v);
                    self.unify(&k_ty, &kt, expr.span);
                    self.unify(&v_ty, &vt, expr.span);
                }
                Type::Dict(
                    Box::new(self.apply_subst(k_ty)),
                    Box::new(self.apply_subst(v_ty)),
                )
            }

            ExprKind::Call { callee, args } => {
                let callee_ty = self.check_expr(callee);
                let applied = self.apply_subst(callee_ty.clone());

                if applied == Type::PyObject {
                    for arg in args {
                        self.check_expr(match arg {
                            CallArg::Positional(e)
                            | CallArg::Keyword(_, e)
                            | CallArg::Splat(e)
                            | CallArg::KwSplat(e) => e,
                        });
                    }
                    return Type::PyObject;
                }

                let resolved_callee = self.instantiate(applied);
                self.expr_types.insert(callee.id, resolved_callee.clone());

                if let Type::Struct(name, type_args) = resolved_callee {
                    let init_name = format!("{}::__init__", name);
                    let has_init = self.lookup_type(&init_name).is_some();
                    let expected_fields = if has_init {
                        self.init_params
                            .get(&init_name)
                            .cloned()
                            .unwrap_or_default()
                    } else {
                        self.struct_fields.get(&name).cloned().unwrap_or_default()
                    };

                    let mut has_kwargs = false;
                    let mut pos_idx = 0;
                    let mut kwarg_map = vec![0; expected_fields.len()];
                    let mut final_args: Vec<Option<Type>> = vec![None; expected_fields.len()];
                    let mut raw_count = 0;

                    for (i, arg) in args.iter().enumerate() {
                        raw_count += 1;
                        match arg {
                            CallArg::Positional(e) | CallArg::Splat(e) | CallArg::KwSplat(e) => {
                                let t = self.check_expr(e);
                                if pos_idx < expected_fields.len() {
                                    final_args[pos_idx] = Some(t);
                                    kwarg_map[pos_idx] = i;
                                    pos_idx += 1;
                                }
                            }
                            CallArg::Keyword(kw_name, e) => {
                                has_kwargs = true;
                                let t = self.check_expr(e);
                                if let Some(idx) = expected_fields.iter().position(|f| f == kw_name)
                                {
                                    if final_args[idx].is_some() {
                                        self.errors.push(
                                            super::super::error::SemanticError::Custom {
                                                msg: format!("duplicate argument `{}`", kw_name),
                                                span: expr.span,
                                            },
                                        );
                                    }
                                    final_args[idx] = Some(t);
                                    kwarg_map[idx] = i;
                                } else {
                                    self.errors
                                        .push(super::super::error::SemanticError::Custom {
                                            msg: format!("no field `{}` on `{}`", kw_name, name),
                                            span: expr.span,
                                        });
                                }
                            }
                        }
                    }

                    if has_kwargs {
                        self.expr_kwarg_maps.insert(expr.id, kwarg_map);
                    }

                    let mut packed_args = Vec::new();
                    for t in final_args {
                        if let Some(t) = t {
                            packed_args.push(t);
                        } else {
                            packed_args.push(Type::Any);
                        }
                    }

                    if has_init {
                        if let Some(init_ty) = self.lookup_type(&init_name) {
                            let instantiated_init = self.instantiate(init_ty);
                            if let Type::Fn(params, _, _) = instantiated_init {
                                if !params.is_empty() {
                                    self.unify(
                                        &params[0],
                                        &Type::Struct(name.clone(), type_args.clone()),
                                        expr.span,
                                    );
                                }
                                if params.len() != packed_args.len() + 1
                                    || raw_count != packed_args.len()
                                {
                                    self.errors
                                        .push(super::super::error::SemanticError::Custom {
                                            msg: format!(
                                                "constructor arity mismatch: expected {}, found {}",
                                                params.len() - 1,
                                                raw_count
                                            ),
                                            span: expr.span,
                                        });
                                } else {
                                    for (p, a) in params.iter().skip(1).zip(packed_args) {
                                        self.unify(p, &a, expr.span);
                                    }
                                }
                            }
                        }
                    } else {
                        if raw_count != expected_fields.len() {
                            self.errors
                                .push(super::super::error::SemanticError::Custom {
                                    msg: format!(
                                        "constructor arity mismatch: expected {}, found {}",
                                        expected_fields.len(),
                                        raw_count
                                    ),
                                    span: expr.span,
                                });
                        }
                        for (i, arg_ty) in packed_args.iter().enumerate() {
                            if i < expected_fields.len() {
                                let field_name = &expected_fields[i];
                                if let Some(field_ty) = self
                                    .field_types
                                    .get(&(name.clone(), field_name.clone()))
                                    .cloned()
                                {
                                    let subst = self.get_struct_subst(&name, &type_args);
                                    let instantiated_field =
                                        self.replace_params_with_vars(field_ty, &subst);
                                    self.unify(&instantiated_field, arg_ty, expr.span);
                                }
                            }
                        }
                    }

                    return Type::Struct(name, type_args);
                }

                let mut arg_types = Vec::with_capacity(args.len());
                for arg in args {
                    match arg {
                        CallArg::Positional(e)
                        | CallArg::Keyword(_, e)
                        | CallArg::Splat(e)
                        | CallArg::KwSplat(e) => {
                            arg_types.push(self.check_expr(e));
                        }
                    }
                }

                let mut final_callee_ty = resolved_callee.clone();
                if let ExprKind::Identifier(name) = &callee.kind {
                    if self.ffi_fns.contains(name) && self.unsafe_depth == 0 {
                        self.errors
                            .push(super::super::error::SemanticError::Custom {
                                msg: format!(
                                    "call to unsafe FFI function `{}` requires unsafe block",
                                    name
                                ),
                                span: expr.span,
                            });
                    }
                } else if let ExprKind::Attr { obj, attr } = &callee.kind
                    && let ExprKind::Identifier(alias) = &obj.kind
                {
                    let mangled = format!("{}::{}", alias, attr);
                    if self.ffi_fns.contains(&mangled) && self.unsafe_depth == 0 {
                        self.errors
                            .push(super::super::error::SemanticError::Custom {
                                msg: format!(
                                    "call to unsafe FFI function `{}` requires unsafe block",
                                    mangled
                                ),
                                span: expr.span,
                            });
                    }
                }

                if let ExprKind::Attr { .. } = &callee.kind
                    && let Type::Fn(params, ret, args) = &resolved_callee
                    && !params.is_empty()
                    && params.len() == arg_types.len() + 1
                {
                    final_callee_ty = Type::Fn(
                        params.iter().skip(1).cloned().collect(),
                        ret.clone(),
                        args.clone(),
                    );
                }

                let is_vararg = match &callee.kind {
                    ExprKind::Identifier(name) => self.vararg_fns.contains(name.as_str()),
                    ExprKind::Attr { obj, attr } => {
                        if let ExprKind::Identifier(alias) = &obj.kind {
                            let mangled = format!("{}::{}", alias, attr);
                            self.vararg_fns.contains(mangled.as_str())
                        } else {
                            false
                        }
                    }
                    _ => false,
                };
                if is_vararg {
                    let ret_ty = self.fresh_var();
                    if let Type::Fn(_, fn_ret, _) = self.apply_subst(final_callee_ty) {
                        self.unify(&ret_ty, &fn_ret, expr.span);
                    }
                    self.apply_subst(ret_ty)
                } else {
                    let ret_ty = self.fresh_var();
                    let expected_args = if let Type::Fn(_, _, callee_args) = &resolved_callee {
                        callee_args.iter().map(|_| self.fresh_var()).collect()
                    } else {
                        Vec::new()
                    };
                    let expected_fn = Type::Fn(arg_types, Box::new(ret_ty.clone()), expected_args);
                    self.unify(&final_callee_ty, &expected_fn, expr.span);
                    self.apply_subst(ret_ty)
                }
            }

            ExprKind::Index { obj, index } => {
                let obj_ty = self.check_expr(obj);
                let idx_ty = self.check_expr(index);
                let mut current_obj_ty = self.apply_subst(obj_ty);
                while let Type::Ref(inner) | Type::MutRef(inner) = current_obj_ty {
                    current_obj_ty = *inner;
                }
                match current_obj_ty {
                    Type::List(inner) => {
                        self.unify(&Type::Int, &idx_ty, expr.span);
                        *inner
                    }
                    Type::Dict(k, v) => {
                        self.unify(&k, &idx_ty, expr.span);
                        *v
                    }
                    Type::Tuple(_) => Type::Any,
                    Type::Str => {
                        self.unify(&Type::Int, &idx_ty, expr.span);
                        Type::Str
                    }
                    Type::PyObject => Type::PyObject,
                    _ => self.fresh_var(),
                }
            }

            ExprKind::Attr { obj, attr } => {
                let obj_ty = self.check_expr(obj);
                let resolved_obj = self.apply_subst(obj_ty);

                if resolved_obj == Type::PyObject {
                    return Type::PyObject;
                }

                if let ExprKind::Identifier(name) = &obj.kind {
                    let mangled = format!("{}::{}", name, attr);
                    if let Some(ty) = self.lookup_type(&mangled) {
                        let instantiated = self.instantiate(ty);
                        if let Type::Fn(params, _, _) = &instantiated
                            && !params.is_empty()
                        {
                            let mut auto_ref_obj = resolved_obj.clone();
                            if let Type::MutRef(inner) = &params[0] {
                                if auto_ref_obj == **inner {
                                    auto_ref_obj = Type::MutRef(Box::new(auto_ref_obj));
                                }
                            } else if let Type::Ref(inner) = &params[0] {
                                if auto_ref_obj == **inner {
                                    auto_ref_obj = Type::Ref(Box::new(auto_ref_obj));
                                }
                            }
                            self.unify(&params[0], &auto_ref_obj, expr.span);
                        }
                        return instantiated;
                    }
                }

                let mut inner_obj = resolved_obj.clone();
                while let Type::Ref(inner) | Type::MutRef(inner) = &inner_obj {
                    inner_obj = *inner.clone();
                }

                if let Type::Struct(ref struct_name, ref type_args) = inner_obj {
                    let mangled = format!("{}::{}", struct_name, attr);
                    if let Some(ty) = self.lookup_type(&mangled) {
                        let instantiated = self.instantiate(ty);
                        if let Type::Fn(params, _, _) = &instantiated
                            && !params.is_empty()
                        {
                            let mut auto_ref_obj = resolved_obj.clone();
                            if let Type::MutRef(inner) = &params[0] {
                                if auto_ref_obj == **inner {
                                    auto_ref_obj = Type::MutRef(Box::new(auto_ref_obj));
                                }
                            } else if let Type::Ref(inner) = &params[0] {
                                if auto_ref_obj == **inner {
                                    auto_ref_obj = Type::Ref(Box::new(auto_ref_obj));
                                }
                            }
                            self.unify(&params[0], &auto_ref_obj, expr.span);
                        }
                        return instantiated;
                    }

                    if let Some(ty) = self.field_types.get(&(struct_name.clone(), attr.clone())) {
                        let subst = self.get_struct_subst(struct_name, type_args);
                        return self.replace_params_with_vars(ty.clone(), &subst);
                    }
                }

                let mut current_obj = resolved_obj.clone();
                while let Type::Ref(inner) | Type::MutRef(inner) = current_obj {
                    current_obj = *inner;
                }

                if let Type::Dict(ref k, ref v) = current_obj {
                    if attr == "keys" {
                        return Type::Fn(vec![], Box::new(Type::List(k.clone())), Vec::new());
                    }
                    if attr == "values" {
                        return Type::Fn(vec![], Box::new(Type::List(v.clone())), Vec::new());
                    }
                    if attr == "remove" {
                        return Type::Fn(vec![*k.clone()], Box::new(Type::Null), Vec::new());
                    }
                }

                if attr == "copy" {
                    return Type::Fn(vec![], Box::new(resolved_obj), Vec::new());
                }

                if attr == "str" {
                    return Type::Fn(vec![], Box::new(Type::Str), Vec::new());
                }
                if attr == "int" || attr == "i64" {
                    return Type::Fn(vec![], Box::new(Type::Int), Vec::new());
                }
                match attr.as_str() {
                    "i32" => return Type::Fn(vec![], Box::new(Type::I32), Vec::new()),
                    "i16" => return Type::Fn(vec![], Box::new(Type::I16), Vec::new()),
                    "i8" => return Type::Fn(vec![], Box::new(Type::I8), Vec::new()),
                    "u64" => return Type::Fn(vec![], Box::new(Type::U64), Vec::new()),
                    "u32" => return Type::Fn(vec![], Box::new(Type::U32), Vec::new()),
                    "u16" => return Type::Fn(vec![], Box::new(Type::U16), Vec::new()),
                    "u8" => return Type::Fn(vec![], Box::new(Type::U8), Vec::new()),
                    "float" | "f64" => return Type::Fn(vec![], Box::new(Type::Float), Vec::new()),
                    "f32" => return Type::Fn(vec![], Box::new(Type::F32), Vec::new()),
                    _ => {}
                }
                if attr == "bool" {
                    return Type::Fn(vec![], Box::new(Type::Bool), Vec::new());
                }

                self.fresh_var()
            }

            ExprKind::ListComp { elt, clauses } => {
                self.enter_scope();
                self.check_comp_clauses(clauses, expr.span);
                let inner = self.check_expr(elt);
                self.leave_scope();
                Type::List(Box::new(inner))
            }

            ExprKind::SetComp { elt, clauses } => {
                self.enter_scope();
                self.check_comp_clauses(clauses, expr.span);
                let inner = self.check_expr(elt);
                self.leave_scope();
                Type::Set(Box::new(inner))
            }

            ExprKind::DictComp {
                key,
                value,
                clauses,
            } => {
                self.enter_scope();
                self.check_comp_clauses(clauses, expr.span);
                let k = self.check_expr(key);
                let v = self.check_expr(value);
                self.leave_scope();
                Type::Dict(Box::new(k), Box::new(v))
            }

            ExprKind::Match { expr, cases } => {
                let match_ty = self.check_expr(expr);
                let return_ty = self.fresh_var();

                let mut matched_variants = std::collections::HashSet::new();
                let mut has_wildcard = false;

                for case in cases {
                    self.enter_scope();

                    if let crate::parser::ast::MatchPattern::Variant(v_name, _) = &case.pattern {
                        matched_variants.insert(v_name.clone());
                    }
                    if let crate::parser::ast::MatchPattern::Wildcard = &case.pattern {
                        has_wildcard = true;
                    }

                    self.check_pattern(&case.pattern, &match_ty, expr.span);

                    let mut case_ty = Type::Null;
                    for (i, stmt) in case.body.iter().enumerate() {
                        self.check_stmt(stmt);
                        if i == case.body.len() - 1 {
                            if let crate::parser::StmtKind::ExprStmt(e) = &stmt.kind {
                                case_ty = self.check_expr(e);
                            }
                        }
                    }

                    let branch_returns = case
                        .body
                        .iter()
                        .any(|s| matches!(s.kind, crate::parser::StmtKind::Return(_)));
                    if !branch_returns {
                        self.unify(&return_ty, &case_ty, expr.span);
                    }

                    self.leave_scope();
                }

                if !has_wildcard {
                    match &match_ty {
                        Type::Enum(enum_name, _) => {
                            if let Some(all_variants) = self.enum_variants.get(enum_name) {
                                for v in all_variants {
                                    if !matched_variants.contains(v) {
                                        self.errors.push(super::super::error::SemanticError::Custom {
                                            msg: format!(
                                                "non-exhaustive patterns: variant {} not covered",
                                                v
                                            ),
                                            span: expr.span,
                                        });
                                    }
                                }
                            }
                        }
                        Type::Union(members) => {
                            for ty in members {
                                if let Type::Enum(en, _) = ty
                                    && let Some(all_variants) = self.enum_variants.get(en)
                                {
                                    for v in all_variants {
                                        if !matched_variants.contains(v) {
                                            self.errors.push(super::super::error::SemanticError::Custom {
                                                msg: format!(
                                                    "non-exhaustive patterns: variant {} of {} not covered",
                                                    v, en
                                                ),
                                                span: expr.span,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }

                return_ty
            }

            ExprKind::Try(inner) => {
                let inner_ty = self.check_expr(inner);
                let inner_ty = self.apply_subst(inner_ty);

                let is_error = |ty: &Type| -> bool {
                    match ty {
                        Type::Struct(name, _) | Type::Enum(name, _) => {
                            name == "Error"
                                || name.ends_with("Error")
                                || self
                                    .type_traits
                                    .contains(&(name.clone(), "Error".to_string()))
                        }
                        _ => false,
                    }
                };

                let (success_types, error_types): (Vec<Type>, Vec<Type>) = match &inner_ty {
                    Type::Union(variants) => variants.iter().cloned().partition(|ty| !is_error(ty)),
                    other => {
                        if is_error(other) {
                            (vec![], vec![other.clone()])
                        } else {
                            (vec![other.clone()], vec![])
                        }
                    }
                };

                if let Some(expected) = self.current_return_type.clone() {
                    let expected_resolved = self.apply_subst(expected);
                    let mut expected_variants = Vec::new();
                    match &expected_resolved {
                        Type::Union(v) => expected_variants.extend(v.clone()),
                        Type::Any => {}
                        other => expected_variants.push(other.clone()),
                    }

                    if expected_resolved != Type::Any {
                        for err_ty in &error_types {
                            if !expected_variants.contains(err_ty) {
                                self.errors
                                    .push(super::super::error::SemanticError::Custom {
                                        msg: format!(
                                            "cannot propagate error `{}`, function returns `{}`",
                                            err_ty, expected_resolved
                                        ),
                                        span: expr.span,
                                    });
                            }
                        }
                    }
                }

                if success_types.is_empty() {
                    Type::Never
                } else if success_types.len() == 1 {
                    success_types[0].clone()
                } else {
                    Type::Union(success_types)
                }
            }

            ExprKind::Await(inner) => {
                let inner_ty = self.check_expr(inner);
                match inner_ty {
                    Type::Future(t) => *t,
                    Type::Any | Type::Var(_) => Type::Any,
                    other => {
                        self.errors
                            .push(super::super::error::SemanticError::Custom {
                                msg: format!("'await' requires a Future[T], got {}", other),
                                span: expr.span,
                            });
                        Type::Any
                    }
                }
            }

            ExprKind::Slice { start, stop, step } => {
                if let Some(e) = start {
                    self.check_expr(e);
                }
                if let Some(e) = stop {
                    self.check_expr(e);
                }
                if let Some(e) = step {
                    self.check_expr(e);
                }
                Type::Any
            }

            ExprKind::AsyncBlock(body) => {
                self.async_depth += 1;
                self.enter_scope();
                let mut last_ty = Type::Null;
                for (i, s) in body.iter().enumerate() {
                    self.check_stmt(s);
                    if i == body.len() - 1
                        && let crate::parser::StmtKind::ExprStmt(e) = &s.kind
                        && let Some(t) = self.expr_types.get(&e.id).cloned()
                    {
                        last_ty = t;
                    }
                }
                self.leave_scope();
                self.async_depth -= 1;
                Type::Future(Box::new(last_ty))
            }
        }
    }

    pub(super) fn check_binop(&mut self, op: &BinOp, l: &Type, r: &Type, span: Span) -> Type {
        let l_resolved = self.apply_subst(l.clone());
        let r_resolved = self.apply_subst(r.clone());
        let is_py = matches!(l_resolved, Type::PyObject) || matches!(r_resolved, Type::PyObject);

        match op {
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Mod
            | BinOp::Pow
            | BinOp::Shl
            | BinOp::Shr
            | BinOp::BitOr
            | BinOp::BitAnd
            | BinOp::BitXor => {
                if is_py {
                    return Type::PyObject;
                }
                self.unify(l, r, span);
                self.apply_subst(l.clone())
            }
            BinOp::Eq
            | BinOp::NotEq
            | BinOp::Lt
            | BinOp::LtEq
            | BinOp::Gt
            | BinOp::GtEq
            | BinOp::In
            | BinOp::NotIn => Type::Bool,
            BinOp::And | BinOp::Or => {
                if is_py {
                    return Type::PyObject;
                }
                self.unify(l, r, span);
                self.apply_subst(l.clone())
            }
        }
    }

    pub(super) fn check_aug_op(
        &mut self,
        _op: &AugOp,
        target: &Type,
        val: &Type,
        span: Span,
    ) -> Type {
        let target_resolved = self.apply_subst(target.clone());
        let val_resolved = self.apply_subst(val.clone());
        if matches!(target_resolved, Type::PyObject) || matches!(val_resolved, Type::PyObject) {
            return Type::PyObject;
        }
        self.unify(target, val, span);
        self.apply_subst(target.clone())
    }

    pub(super) fn expect_truthy(&mut self, ty: &Type, span: Span) {
        let resolved = self.apply_subst(ty.clone());
        match resolved {
            Type::Null | Type::Never => {
                self.errors
                    .push(super::super::error::SemanticError::Custom {
                        msg: format!("type `{}` cannot be used as a condition", resolved),
                        span,
                    });
            }
            Type::Fn(..) => {
                self.errors
                    .push(super::super::error::SemanticError::Custom {
                        msg: "function value cannot be used as a condition".into(),
                        span,
                    });
            }
            Type::Future(_) => {
                self.errors
                    .push(super::super::error::SemanticError::Custom {
                        msg: "future cannot be used as a condition; did you mean to await it?"
                            .into(),
                        span,
                    });
            }
            _ => {}
        }
    }
}
