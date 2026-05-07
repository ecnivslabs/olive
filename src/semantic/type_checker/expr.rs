use super::super::types::Type;
use super::TypeChecker;
use crate::parser::{AugOp, BinOp, CallArg, Expr, ExprKind, UnaryOp};
use crate::span::Span;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Collection {
    List,
    Set,
}

impl TypeChecker {
    pub(super) fn check_expr(&mut self, expr: &Expr) -> Type {
        let ty = self.infer_expr(expr);
        let final_ty = self.apply_subst(ty);
        self.expr_types.insert(expr.id, final_ty.clone());
        final_ty
    }

    /// Checks `expr` against an expected type, so a collection literal can adopt
    /// it (e.g. a `[Any]` annotation accepts a mixed list). Applies to `expr`
    /// only.
    pub(super) fn check_expr_expecting(&mut self, expr: &Expr, expected: &Type) -> Type {
        let resolved = self.apply_subst(expected.clone());
        self.expected = Some(resolved);
        let ty = self.check_expr(expr);
        self.expected = None;
        ty
    }

    /// Tries to unify, rolling back both errors and substitutions on failure so
    /// the probe leaves no trace. Returns whether the types are compatible.
    pub(super) fn unify_silently(&mut self, a: &Type, b: &Type, span: Span) -> bool {
        let errs_before = self.errors.len();
        let subst_before = self.substitutions.clone();
        self.unify(a, b, span);
        if self.errors.len() > errs_before {
            self.errors.truncate(errs_before);
            self.substitutions = subst_before;
            false
        } else {
            true
        }
    }

    pub(super) fn infer_expr(&mut self, expr: &Expr) -> Type {
        let expected = self.expected.take();
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
            ExprKind::Null => Type::Null,

            ExprKind::Deref(inner) => {
                let inner_ty = self.check_expr(inner);
                let resolved = self.apply_subst(inner_ty.clone());
                match resolved {
                    Type::Ptr(_) => {
                        if self.unsafe_depth == 0 {
                            self.errors.push(super::super::error::SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0408",
                                    "dereferencing a raw pointer is unsafe",
                                    expr.span,
                                )
                                .label("this dereference may read invalid memory")
                                .help("wrap it in an `unsafe:` block to acknowledge the risk"),
                            ));
                        }
                        Type::Int
                    }
                    Type::List(elem_ty) => *elem_ty,
                    ref t if t.is_py_value() => Type::Any,
                    Type::Any | Type::Tuple(_) => Type::Any,
                    _ => {
                        if self.unsafe_depth == 0 {
                            self.errors.push(super::super::error::SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0408",
                                    "dereferencing a raw pointer is unsafe",
                                    expr.span,
                                )
                                .label("this dereference may read invalid memory")
                                .help("wrap it in an `unsafe:` block to acknowledge the risk"),
                            ));
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
                    self.errors.push(super::super::error::SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0411",
                            format!("cannot mutably borrow immutable variable `{name}`"),
                            expr.span,
                        )
                        .label("cannot borrow as mutable")
                        .help(format!("declare it mutable: `let mut {name} = ...`")),
                    ));
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

            ExprKind::List(elems) => self.infer_collection(elems, &expected, Collection::List),

            ExprKind::Tuple(elems) => {
                let elem_expected = match &expected {
                    Some(Type::Tuple(tys)) if tys.len() == elems.len() => Some(tys.clone()),
                    _ => None,
                };
                let types: Vec<Type> = elems
                    .iter()
                    .enumerate()
                    .map(|(i, e)| match &elem_expected {
                        Some(tys) => self.check_expr_expecting(e, &tys[i]),
                        None => self.check_expr(e),
                    })
                    .collect();
                Type::Tuple(types)
            }

            ExprKind::Set(elems) => self.infer_collection(elems, &expected, Collection::Set),

            ExprKind::Dict(pairs) => {
                let (exp_k, exp_v) = match &expected {
                    Some(Type::Dict(k, v)) => (Some((**k).clone()), Some((**v).clone())),
                    Some(Type::Any) => (Some(Type::Any), Some(Type::Any)),
                    _ => (None, None),
                };
                let k_ty = self.fresh_var();
                let v_ty = self.fresh_var();
                let mut hetero_k = false;
                let mut hetero_v = false;
                for (k, v) in pairs {
                    let kt = match &exp_k {
                        Some(e) => self.check_expr_expecting(k, e),
                        None => self.check_expr(k),
                    };
                    let vt = match &exp_v {
                        Some(e) => self.check_expr_expecting(v, e),
                        None => self.check_expr(v),
                    };
                    match &exp_k {
                        Some(e) => self.unify(e, &kt, k.span),
                        None => hetero_k |= !self.unify_silently(&k_ty, &kt, k.span),
                    }
                    match &exp_v {
                        Some(e) => self.unify(e, &vt, v.span),
                        None => hetero_v |= !self.unify_silently(&v_ty, &vt, v.span),
                    }
                }
                let key_ty = match exp_k {
                    Some(e) => e,
                    None if hetero_k => Type::Any,
                    None => self.apply_subst(k_ty),
                };
                let val_ty = match exp_v {
                    Some(e) => e,
                    None if hetero_v => Type::Any,
                    None => self.apply_subst(v_ty),
                };
                Type::Dict(Box::new(key_ty), Box::new(val_ty))
            }

            ExprKind::Call { callee, args } => {
                let callee_ty = self.check_expr(callee);
                let applied = self.apply_subst(callee_ty.clone());

                if applied.is_py_value() {
                    let arg_tys: Vec<Type> = args
                        .iter()
                        .map(|a| {
                            self.check_expr(match a {
                                CallArg::Positional(e)
                                | CallArg::Keyword(_, e)
                                | CallArg::Splat(e)
                                | CallArg::KwSplat(e) => e,
                            })
                        })
                        .collect();
                    if let ExprKind::Attr { obj, attr } = &callee.kind {
                        if let ExprKind::Identifier(module_name) = &obj.kind {
                            let all_positional =
                                args.iter().all(|a| matches!(a, CallArg::Positional(_)));
                            self.check_py_call(
                                module_name,
                                attr,
                                args.len(),
                                all_positional,
                                expr.span,
                            );
                            if let Some(ret_ty) =
                                self.resolve_py_fn_overload(module_name, attr, &arg_tys)
                            {
                                return ret_ty;
                            }
                        }
                        let obj_ty = self
                            .expr_types
                            .get(&obj.id)
                            .cloned()
                            .map(|t| self.apply_subst(t))
                            .unwrap_or(Type::PyObject);
                        if let Type::PyNamed(m, n) = &obj_ty
                            && let Some(ret_ty) =
                                self.resolve_py_method_overload(m, n, attr, &arg_tys)
                        {
                            return ret_ty;
                        }
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
                                        self.errors.push(super::super::error::SemanticError::rich(
                                            crate::compile::errors::Diagnostic::error(
                                                "E0412",
                                                format!("field `{kw_name}` is given twice"),
                                                expr.span,
                                            )
                                            .label("duplicate field in this initializer")
                                            .help("remove the redundant assignment"),
                                        ));
                                    }
                                    final_args[idx] = Some(t);
                                    kwarg_map[idx] = i;
                                } else {
                                    let suggestion = super::super::suggest::closest(
                                        kw_name,
                                        expected_fields.iter().map(String::as_str),
                                    );
                                    self.errors.push(super::super::error::SemanticError::rich(
                                        crate::compile::errors::Diagnostic::error(
                                            "E0413",
                                            format!("struct `{name}` has no field `{kw_name}`"),
                                            expr.span,
                                        )
                                        .label("unknown field")
                                        .suggest(&suggestion),
                                    ));
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
                                    self.errors.push(super::super::error::SemanticError::rich(
                                        crate::compile::errors::Diagnostic::error(
                                            "E0403",
                                            format!("wrong number of arguments to `{name}`"),
                                            expr.span,
                                        )
                                        .label(format!(
                                            "expected {} argument(s), found {}",
                                            params.len() - 1,
                                            raw_count
                                        )),
                                    ));
                                } else {
                                    for (p, a) in params.iter().skip(1).zip(packed_args) {
                                        self.unify(p, &a, expr.span);
                                    }
                                }
                            }
                        }
                    } else {
                        if raw_count != expected_fields.len() {
                            self.errors.push(super::super::error::SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0403",
                                    format!("wrong number of fields for `{name}`"),
                                    expr.span,
                                )
                                .label(format!(
                                    "expected {} field(s), found {}",
                                    expected_fields.len(),
                                    raw_count
                                ))
                                .help(format!(
                                    "`{name}` has fields: {}",
                                    expected_fields.join(", ")
                                )),
                            ));
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
                            .push(super::super::error::SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0409",
                                    format!("call to FFI function `{name}` is unsafe"),
                                    expr.span,
                                )
                                .label("foreign calls may violate memory safety")
                                .note("Olive cannot verify the safety of foreign code across the FFI boundary")
                                .help("wrap the call in an `unsafe:` block, or mark the declaration `@safe`"),
                            ));
                    }
                } else if let ExprKind::Attr { obj, attr } = &callee.kind
                    && let ExprKind::Identifier(alias) = &obj.kind
                {
                    let mangled = format!("{}::{}", alias, attr);
                    if self.ffi_fns.contains(&mangled) && self.unsafe_depth == 0 {
                        self.errors
                            .push(super::super::error::SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0409",
                                    format!("call to FFI function `{mangled}` is unsafe"),
                                    expr.span,
                                )
                                .label("foreign calls may violate memory safety")
                                .note("Olive cannot verify the safety of foreign code across the FFI boundary")
                                .help("wrap the call in an `unsafe:` block, or mark the declaration `@safe`"),
                            ));
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
                    Type::Bytes => {
                        self.unify(&Type::Int, &idx_ty, expr.span);
                        Type::Int
                    }
                    ref t if t.is_py_value() => Type::PyObject,
                    _ => self.fresh_var(),
                }
            }

            ExprKind::Attr { obj, attr } => {
                let obj_ty = self.check_expr(obj);
                let resolved_obj = self.apply_subst(obj_ty);

                if let Type::PyNamed(ref m, ref n) = resolved_obj {
                    if let Some(field_ty) = self.resolve_py_field(m, n, attr) {
                        return field_ty;
                    }
                    return Type::PyObject;
                }

                if resolved_obj.is_py_value() {
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
                            } else if let Type::Ref(inner) = &params[0]
                                && auto_ref_obj == **inner
                            {
                                auto_ref_obj = Type::Ref(Box::new(auto_ref_obj));
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
                            } else if let Type::Ref(inner) = &params[0]
                                && auto_ref_obj == **inner
                            {
                                auto_ref_obj = Type::Ref(Box::new(auto_ref_obj));
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

                // A dict erased to `Any` (e.g. from JSON) still answers the
                // dict methods at runtime.
                if current_obj == Type::Any {
                    match attr.as_str() {
                        "keys" | "values" => {
                            return Type::Fn(
                                vec![],
                                Box::new(Type::List(Box::new(Type::Any))),
                                Vec::new(),
                            );
                        }
                        "remove" => {
                            return Type::Fn(vec![Type::Any], Box::new(Type::Null), Vec::new());
                        }
                        _ => {}
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

                    self.check_pattern(&case.pattern, &match_ty, case.span);

                    let mut case_ty = Type::Null;
                    for (i, stmt) in case.body.iter().enumerate() {
                        self.check_stmt(stmt);
                        if i == case.body.len() - 1
                            && let crate::parser::StmtKind::ExprStmt(e) = &stmt.kind
                        {
                            case_ty = self.check_expr(e);
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
                                        self.errors.push(super::super::error::SemanticError::rich(
                                            crate::compile::errors::Diagnostic::error(
                                                "E0414",
                                                "non-exhaustive patterns",
                                                expr.span,
                                            )
                                            .label(format!(
                                                "variant `{enum_name}::{v}` is not covered"
                                            ))
                                            .note("a `match` must handle every possible value")
                                            .help(
                                                format!(
                                                    "add `case {v}:` or a wildcard `case _:` arm"
                                                ),
                                            ),
                                        ));
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
                                            self.errors.push(super::super::error::SemanticError::rich(
                                                crate::compile::errors::Diagnostic::error(
                                                    "E0414",
                                                    "non-exhaustive patterns",
                                                    expr.span,
                                                )
                                                .label(format!("variant `{en}::{v}` is not covered"))
                                                .note("a `match` over a union must handle every member's variants")
                                                .help(format!(
                                                    "add `case {v}:` or a wildcard `case _:` arm"
                                                )),
                                            ));
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
                                self.errors.push(
                                    super::super::error::SemanticError::rich(
                                        crate::compile::errors::Diagnostic::error(
                                            "E0406",
                                            format!("`?` cannot propagate `{err_ty}` here"),
                                            expr.span,
                                        )
                                        .label(format!(
                                            "this error is `{err_ty}`, but the function returns `{expected_resolved}`"
                                        ))
                                        .help(format!(
                                            "add `{err_ty}` to the function's return type, or convert the error before propagating"
                                        )),
                                    ),
                                );
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
                        self.errors.push(super::super::error::SemanticError::rich(
                            crate::compile::errors::Diagnostic::error(
                                "E0405",
                                "`await` requires a future",
                                expr.span,
                            )
                            .label(format!("this is `{other}`, not a `Future[T]`"))
                            .help("only the result of an `async` call can be awaited"),
                        ));
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

    fn binop_dunder(op: &BinOp) -> &'static str {
        match op {
            BinOp::Add => "__add__",
            BinOp::Sub => "__sub__",
            BinOp::Mul => "__mul__",
            BinOp::Div => "__truediv__",
            BinOp::Mod => "__mod__",
            BinOp::Pow => "__pow__",
            BinOp::BitOr => "__or__",
            BinOp::BitAnd => "__and__",
            BinOp::BitXor => "__xor__",
            BinOp::Shl => "__lshift__",
            BinOp::Shr => "__rshift__",
            _ => "",
        }
    }

    /// Infers a list or set literal's type. With an annotated element type, each
    /// element must match it (so `[str] = ["a", 5]` errors). Otherwise a uniform
    /// literal keeps its element type and a mixed one widens to `[Any]`.
    fn infer_collection(
        &mut self,
        elems: &[Expr],
        expected: &Option<Type>,
        kind: Collection,
    ) -> Type {
        let exp_elem = match expected {
            Some(Type::List(e)) if kind == Collection::List => Some((**e).clone()),
            Some(Type::Set(e)) if kind == Collection::Set => Some((**e).clone()),
            Some(Type::Any) => Some(Type::Any),
            _ => None,
        };
        let elem_ty = if let Some(exp) = exp_elem {
            for e in elems {
                let e_ty = self.check_expr_expecting(e, &exp);
                self.unify(&exp, &e_ty, e.span);
            }
            exp
        } else {
            let var = self.fresh_var();
            let mut heterogeneous = false;
            for e in elems {
                let e_ty = self.check_expr(e);
                heterogeneous |= !self.unify_silently(&var, &e_ty, e.span);
            }
            if heterogeneous {
                Type::Any
            } else {
                self.apply_subst(var)
            }
        };
        match kind {
            Collection::List => Type::List(Box::new(elem_ty)),
            Collection::Set => Type::Set(Box::new(elem_ty)),
        }
    }

    pub(super) fn check_binop(&mut self, op: &BinOp, l: &Type, r: &Type, span: Span) -> Type {
        let l_resolved = self.apply_subst(l.clone());
        let r_resolved = self.apply_subst(r.clone());
        let is_py = l_resolved.is_py_value() || r_resolved.is_py_value();

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
                    let dunder = Self::binop_dunder(op);
                    if !dunder.is_empty() {
                        if let Type::PyNamed(ref m, ref n) = l_resolved
                            && let Some(ret) = self.resolve_py_method_overload(
                                m,
                                n,
                                dunder,
                                std::slice::from_ref(&r_resolved),
                            )
                        {
                            return ret;
                        }
                        if let Type::PyNamed(ref m, ref n) = r_resolved {
                            let rdunder = format!("__r{}", &dunder[2..]);
                            if let Some(ret) = self.resolve_py_method_overload(
                                m,
                                n,
                                &rdunder,
                                std::slice::from_ref(&l_resolved),
                            ) {
                                return ret;
                            }
                        }
                    }
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
        if target_resolved.is_py_value() || val_resolved.is_py_value() {
            return Type::PyObject;
        }
        self.unify(target, val, span);
        self.apply_subst(target.clone())
    }

    pub(super) fn expect_truthy(&mut self, ty: &Type, span: Span) {
        let resolved = self.apply_subst(ty.clone());
        match resolved {
            Type::Null | Type::Never => {
                self.errors.push(super::super::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0404",
                        format!("`{resolved}` cannot be used as a condition"),
                        span,
                    )
                    .label("expected a `bool` here")
                    .help("compare the value explicitly, e.g. `value != None`"),
                ));
            }
            Type::Fn(..) => {
                self.errors.push(super::super::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0404",
                        "a function value cannot be used as a condition",
                        span,
                    )
                    .label("this is a function, not a `bool`")
                    .help("did you mean to call it? add `()`"),
                ));
            }
            Type::Future(_) => {
                self.errors.push(super::super::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0404",
                        "a future cannot be used as a condition",
                        span,
                    )
                    .label("this is a `Future[T]`, not a `bool`")
                    .help("await it first: `await value`"),
                ));
            }
            _ => {}
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
    fn list_literal_homogeneous() {
        let tc = typeck("let xs = [1, 2, 3]\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn set_literal() {
        let tc = typeck("let s = {1, 2, 3}\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn dict_literal() {
        let tc = typeck("let d = {\"a\": 1, \"b\": 2}\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn unary_neg() {
        let tc = typeck("let x = -42\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn unary_not_on_bool() {
        let tc = typeck("let b = not True\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn cast_to_float() {
        let tc = typeck("let x = 1 as float\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn immutable_borrow() {
        let tc = typeck("let x = 42\nlet r = &x\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn mutable_borrow_of_immutable() {
        let tc = typeck("let x = 42\nlet r = &mut x\n");
        assert!(!tc.errors.is_empty());
    }

    #[test]
    fn index_list() {
        let tc = typeck("let xs = [1, 2, 3]\nlet v = xs[1]\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn index_dict() {
        let tc = typeck("let d = {\"a\": 1}\nlet v = d[\"a\"]\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn list_comprehension() {
        let tc = typeck("let xs = [x for x in [1, 2, 3]]\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn set_comprehension() {
        let tc = typeck("let s = {x for x in [1, 2, 3]}\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn dict_comprehension() {
        let tc = typeck("let d = {x: x * 2 for x in [1, 2, 3]}\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn slice_expr() {
        let tc = typeck("let xs = [1, 2, 3]\nlet s = xs[1:3]\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn async_block() {
        let tc = typeck("let f = async:\n    42\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn binop_comparison_returns_bool() {
        let tc = typeck("let b = (1 < 2)\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn fstring_typechecks() {
        let tc = typeck("let s = f\"hello {42}\"\n");
        assert!(tc.errors.is_empty());
    }
}
