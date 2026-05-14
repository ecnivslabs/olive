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
            ExprKind::Integer(_) => Type::Int,
            ExprKind::Float(_) => Type::Float,
            ExprKind::Str(_) => Type::Str,
            ExprKind::FStr(exprs) => {
                for e in exprs {
                    self.check_expr(e);
                }
                Type::Str
            }
            ExprKind::Bool(_) => Type::Bool,

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
            ExprKind::Identifier(name) => self.lookup_type(name).unwrap_or_else(Type::new_var),

            ExprKind::BinOp { left, op, right } => {
                let l_ty = self.check_expr(left);
                let r_ty = self.check_expr(right);
                self.check_binop(op, &l_ty, &r_ty, expr.span)
            }

            ExprKind::UnaryOp { op, operand } => {
                let o_ty = self.check_expr(operand);
                match op {
                    UnaryOp::Not => Type::Bool,
                    UnaryOp::Neg | UnaryOp::Pos => o_ty,
                }
            }

            ExprKind::List(elems) => {
                let elem_ty = Type::new_var();
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
                let elem_ty = Type::new_var();
                for e in elems {
                    let e_ty = self.check_expr(e);
                    self.unify(&elem_ty, &e_ty, expr.span);
                }
                Type::Set(Box::new(self.apply_subst(elem_ty)))
            }

            ExprKind::Dict(pairs) => {
                let k_ty = Type::new_var();
                let v_ty = Type::new_var();
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
                let resolved_callee = self.instantiate(applied);
                self.expr_types.insert(callee.id, resolved_callee.clone());

                if let Type::Struct(name, type_args) = resolved_callee {
                    let mut arg_types = Vec::new();
                    for arg in args {
                        arg_types.push(self.check_expr(match arg {
                            CallArg::Positional(e)
                            | CallArg::Keyword(_, e)
                            | CallArg::Splat(e)
                            | CallArg::KwSplat(e) => e,
                        }));
                    }

                    let init_name = format!("{}::__init__", name);
                    if let Some(init_ty) = self.lookup_type(&init_name) {
                        let instantiated_init = self.instantiate(init_ty);
                        if let Type::Fn(params, _, _) = instantiated_init {
                            // Unify the self parameter with the struct type
                            if !params.is_empty() {
                                self.unify(&params[0], &Type::Struct(name.clone(), type_args.clone()), expr.span);
                            }

                            if params.len() != arg_types.len() + 1 {
                                self.errors
                                    .push(super::super::error::SemanticError::Custom {
                                        msg: format!(
                                            "constructor arity mismatch: expected {}, found {}",
                                            params.len() - 1,
                                            arg_types.len()
                                        ),
                                        span: expr.span,
                                    });
                            } else {
                                for (p, a) in params.iter().skip(1).zip(arg_types) {
                                    self.unify(p, &a, expr.span);
                                }
                            }
                        }
                    } else {
                        // Implicit constructor: unify arguments with fields
                        if let Some(fields) = self.struct_fields.get(&name).cloned() {
                            for (i, arg_ty) in arg_types.iter().enumerate() {
                                if i < fields.len() {
                                    let field_name = &fields[i];
                                    if let Some(field_ty) = self.field_types.get(&(name.clone(), field_name.clone())).cloned() {
                                        let subst = self.get_struct_subst(&name, &type_args);
                                        let instantiated_field = self.replace_params_with_vars(field_ty, &subst);
                                        self.unify(&instantiated_field, arg_ty, expr.span);
                                    }
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

                let is_vararg = if let ExprKind::Identifier(name) = &callee.kind {
                    self.vararg_fns.contains(name.as_str())
                } else {
                    false
                };
                if is_vararg {
                    let ret_ty = Type::new_var();
                    if let Type::Fn(_, fn_ret, _) = self.apply_subst(final_callee_ty) {
                        self.unify(&ret_ty, &fn_ret, expr.span);
                    }
                    self.apply_subst(ret_ty)
                } else {
                    let ret_ty = Type::new_var();
                    let expected_args = if let Type::Fn(_, _, callee_args) = &resolved_callee {
                        callee_args.iter().map(|_| Type::new_var()).collect()
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
                    _ => Type::new_var(),
                }
            }

            ExprKind::Attr { obj, attr } => {
                let obj_ty = self.check_expr(obj);
                let resolved_obj = self.apply_subst(obj_ty);

                if let ExprKind::Identifier(name) = &obj.kind {
                    let mangled = format!("{}::{}", name, attr);
                    if let Some(ty) = self.lookup_type(&mangled) {
                        let instantiated = self.instantiate(ty);
                        if let Type::Fn(params, _, _) = &instantiated {
                            if !params.is_empty() {
                                self.unify(&params[0], &resolved_obj, expr.span);
                            }
                        }
                        return instantiated;
                    }
                }

                if let Type::Struct(ref struct_name, ref type_args) = resolved_obj {
                    let mangled = format!("{}::{}", struct_name, attr);
                    if let Some(ty) = self.lookup_type(&mangled) {
                        let instantiated = self.instantiate(ty);
                        if let Type::Fn(params, _, _) = &instantiated {
                            if !params.is_empty() {
                                self.unify(&params[0], &resolved_obj, expr.span);
                            }
                        }
                        return instantiated;
                    }

                    if let Some(ty) = self.field_types.get(&(struct_name.clone(), attr.clone())) {
                        let subst = self.get_struct_subst(struct_name, type_args);
                        return self.replace_params_with_vars(ty.clone(), &subst);
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

                Type::new_var()
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
                let mut return_ty = Type::new_var();

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
                    if case.body.is_empty() {
                        case_ty = Type::Null;
                    } else {
                        for stmt in &case.body {
                            self.check_stmt(stmt);
                            if let crate::parser::StmtKind::ExprStmt(e) = &stmt.kind {
                                case_ty = self.infer_expr(e);
                            }
                        }
                    }
                    self.unify(&return_ty, &case_ty, expr.span);
                    return_ty = case_ty;

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
                if let Type::Union(variants) = &inner_ty
                    && !variants.is_empty()
                {
                    if let Type::Enum(_, _) = &variants[0] {
                         return variants[0].clone();
                    }
                }
                inner_ty
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
        match op {
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Mod
            | BinOp::Pow
            | BinOp::Shl
            | BinOp::Shr => {
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
        self.unify(target, val, span);
        self.apply_subst(target.clone())
    }

    pub(super) fn expect_truthy(&mut self, _ty: &Type, _span: Span) {}
}
