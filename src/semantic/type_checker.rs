use super::error::SemanticError;
use super::types::Type;
use crate::parser::{
    BinOp, CallArg, Expr, ExprKind, ForTarget, MatchPattern, ParamKind, Program, Stmt, StmtKind,
    TypeExpr, TypeExprKind, UnaryOp,
};
use crate::span::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

// type checking
pub struct TypeChecker {
    substitutions: HashMap<usize, Type>,
    pub expr_types: HashMap<usize, Type>,
    pub type_env: Vec<HashMap<String, Type>>,
    current_return_type: Option<Type>,
    pub errors: Vec<SemanticError>,
    mut_env: Vec<HashMap<String, bool>>,
    pub field_types: HashMap<(String, String), Type>,
    pub enum_variants: HashMap<String, Vec<String>>,
    current_struct: Option<String>,
    async_depth: usize,
    vararg_fns: HashSet<String>,
    // trait_name -> list of required method names
    traits: HashMap<String, Vec<String>>,
    // (type_name, trait_name) -> implemented
    type_traits: HashSet<(String, String)>,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut global_env = HashMap::default();

        // built-ins
        let builtins = [
            ("print", Type::Fn(vec![Type::Any], Box::new(Type::Int))),
            ("str", Type::Fn(vec![Type::Any], Box::new(Type::Str))),
            ("int", Type::Fn(vec![Type::Any], Box::new(Type::Int))),
            ("i64", Type::Fn(vec![Type::Any], Box::new(Type::Int))),
            ("i32", Type::Fn(vec![Type::Any], Box::new(Type::I32))),
            ("i16", Type::Fn(vec![Type::Any], Box::new(Type::I16))),
            ("i8", Type::Fn(vec![Type::Any], Box::new(Type::I8))),
            ("u64", Type::Fn(vec![Type::Any], Box::new(Type::U64))),
            ("u32", Type::Fn(vec![Type::Any], Box::new(Type::U32))),
            ("u16", Type::Fn(vec![Type::Any], Box::new(Type::U16))),
            ("u8", Type::Fn(vec![Type::Any], Box::new(Type::U8))),
            ("float", Type::Fn(vec![Type::Any], Box::new(Type::Float))),
            ("f64", Type::Fn(vec![Type::Any], Box::new(Type::Float))),
            ("f32", Type::Fn(vec![Type::Any], Box::new(Type::F32))),
            ("bool", Type::Fn(vec![Type::Any], Box::new(Type::Bool))),
            ("type", Type::Fn(vec![Type::Any], Box::new(Type::Str))),
            ("len", Type::Fn(vec![Type::Any], Box::new(Type::Int))),
            (
                "list_new",
                Type::Fn(vec![Type::Int], Box::new(Type::List(Box::new(Type::Any)))),
            ),
            (
                "__olive_async_file_read",
                Type::Fn(vec![Type::Str], Box::new(Type::Future(Box::new(Type::Str)))),
            ),
            (
                "__olive_async_file_write",
                Type::Fn(
                    vec![Type::Str, Type::Str],
                    Box::new(Type::Future(Box::new(Type::Int))),
                ),
            ),
            (
                "__olive_gather",
                Type::Fn(vec![Type::Any], Box::new(Type::List(Box::new(Type::Any)))),
            ),
            (
                "__olive_free_future",
                Type::Fn(vec![Type::Any], Box::new(Type::Int)),
            ),
            (
                "__olive_math_sin",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_cos",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_tan",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_asin",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_acos",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_atan",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_atan2",
                Type::Fn(vec![Type::Float, Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_log",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_log10",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_math_exp",
                Type::Fn(vec![Type::Float], Box::new(Type::Float)),
            ),
            (
                "__olive_random_seed",
                Type::Fn(vec![Type::Int], Box::new(Type::Null)),
            ),
            (
                "__olive_random_get",
                Type::Fn(vec![], Box::new(Type::Float)),
            ),
            (
                "__olive_random_int",
                Type::Fn(vec![Type::Int, Type::Int], Box::new(Type::Int)),
            ),
            (
                "__olive_net_tcp_connect",
                Type::Fn(vec![Type::Str], Box::new(Type::Int)),
            ),
            (
                "__olive_net_tcp_send",
                Type::Fn(vec![Type::Int, Type::Str], Box::new(Type::Int)),
            ),
            (
                "__olive_net_tcp_recv",
                Type::Fn(vec![Type::Int, Type::Int], Box::new(Type::Str)),
            ),
            (
                "__olive_net_tcp_close",
                Type::Fn(vec![Type::Int], Box::new(Type::Null)),
            ),
            (
                "__olive_http_get",
                Type::Fn(vec![Type::Str], Box::new(Type::Str)),
            ),
            (
                "__olive_http_post",
                Type::Fn(vec![Type::Str, Type::Str], Box::new(Type::Str)),
            ),
            (
                "__olive_spawn_task",
                Type::Fn(vec![Type::Any], Box::new(Type::Future(Box::new(Type::Any)))),
            ),
        ];

        for (name, ty) in builtins {
            global_env.insert(name.to_string(), ty);
        }

        Self {
            substitutions: HashMap::default(),
            expr_types: HashMap::default(),
            type_env: vec![global_env],
            current_return_type: None,
            errors: Vec::new(),
            mut_env: vec![HashMap::default()],
            field_types: HashMap::default(),
            enum_variants: HashMap::default(),
            current_struct: None,
            async_depth: 0,
            vararg_fns: HashSet::default(),
            traits: HashMap::default(),
            type_traits: HashSet::default(),
        }
    }

    fn enter_scope(&mut self) {
        self.type_env.push(HashMap::default());
        self.mut_env.push(HashMap::default());
    }

    fn leave_scope(&mut self) {
        self.type_env.pop();
        self.mut_env.pop();
    }

    fn define_type(&mut self, name: &str, ty: Type, is_mut: bool) {
        if let Some(scope) = self.type_env.last_mut() {
            scope.insert(name.to_string(), ty);
        }
        if let Some(scope) = self.mut_env.last_mut() {
            scope.insert(name.to_string(), is_mut);
        }
    }

    fn lookup_type(&self, name: &str) -> Option<Type> {
        for scope in self.type_env.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        None
    }

    fn is_mutable(&self, name: &str) -> bool {
        for scope in self.mut_env.iter().rev() {
            if let Some(is_mut) = scope.get(name) {
                return *is_mut;
            }
        }
        false
    }

    pub fn check_program(&mut self, program: &Program) {
        for stmt in &program.stmts {
            self.check_stmt(stmt);
        }

        let ids: Vec<usize> = self.expr_types.keys().cloned().collect();
        for id in ids {
            let ty = self.expr_types.get(&id).unwrap().clone();
            let final_ty = self.apply_subst(ty);
            self.expr_types.insert(id, final_ty);
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
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
                // consts are immutable
                self.define_type(name, var_ty, false);
            }

            StmtKind::ExprStmt(expr) => {
                self.check_expr(expr);
            }

            StmtKind::Assign { target, value } => {
                let val_ty = self.check_expr(value);
                let target_ty = self.check_expr(target);
                self.unify(&target_ty, &val_ty, stmt.span);

                if let ExprKind::Attr { obj, attr } = &target.kind {
                    let obj_ty = self.check_expr(obj);
                    let resolved_obj = self.apply_subst(obj_ty);
                    if let Type::Struct(struct_name) = resolved_obj {
                        self.field_types.insert((struct_name, attr.clone()), val_ty);
                    }
                }

                if let ExprKind::Identifier(name) = &target.kind
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
                // inside async fn body, `return T` is valid (Future wrapping is implicit)
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
                // register the struct as a named type
                self.define_type(name, Type::Struct(name.clone()), false);

                // register field types so attribute access resolves
                for field in fields {
                    let field_ty = field
                        .type_ann
                        .as_ref()
                        .map(|ann| self.resolve_type_expr(ann))
                        .unwrap_or(Type::Any);
                    self.field_types
                        .insert((name.clone(), field.name.clone()), field_ty);
                }

                // check any associated stmts in the struct body
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
                // verify all required trait methods are present
                if let Some(tr) = trait_name {
                    if let Some(required) = self.traits.get(tr).cloned() {
                        let provided: HashSet<String> = body
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
                    // variant returns enum type
                    let fn_ty = Type::Fn(param_types, Box::new(Type::Enum(name.clone())));
                    let variant_mangled = format!("{}::{}", name, variant.name);
                    self.define_type(&variant_mangled, fn_ty, false);
                }
                self.enum_variants.insert(name.clone(), variant_names);
            }
        }
    }

    fn check_block(&mut self, stmts: &[Stmt]) {
        self.enter_scope();
        for s in stmts {
            self.check_stmt(s);
        }
        self.leave_scope();
    }

    fn check_expr(&mut self, expr: &Expr) -> Type {
        let ty = self.infer_expr(expr);
        let final_ty = self.apply_subst(ty);
        self.expr_types.insert(expr.id, final_ty.clone());
        final_ty
    }

    fn infer_expr(&mut self, expr: &Expr) -> Type {
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
                    self.errors.push(SemanticError::Custom {
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
                let resolved_callee = self.apply_subst(callee_ty.clone());

                if let Type::Struct(name) = resolved_callee {
                    let mut arg_types = Vec::new();
                    for arg in args {
                        arg_types.push(self.check_expr(match arg {
                            CallArg::Positional(e)
                            | CallArg::Keyword(_, e)
                            | CallArg::Splat(e)
                            | CallArg::KwSplat(e) => e,
                        }));
                    }

                    // check __init__
                    let init_name = format!("{}::__init__", name);
                    if let Some(Type::Fn(params, _)) = self.lookup_type(&init_name) {
                        // skip self
                        if params.len() != arg_types.len() + 1 {
                            self.errors.push(SemanticError::Custom {
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

                    return Type::Struct(name);
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

                let mut final_callee_ty = callee_ty.clone();
                // Special case for method calls: if it's an attribute access that resolved to a method,
                // we need to account for the implicit 'self' argument.
                if let ExprKind::Attr { .. } = &callee.kind
                    && let Type::Fn(params, _) = &resolved_callee
                    && !params.is_empty()
                    && params.len() == arg_types.len() + 1
                {
                    // It's a method call with self implicit.
                    // Construct a type that matches the CALL SITE arity.
                    final_callee_ty = Type::Fn(
                        params.iter().skip(1).cloned().collect(),
                        Box::new(self.apply_subst(Type::new_var())),
                    );
                }

                // For vararg/kwarg functions, relax strict arity - just extract return type
                let is_vararg = if let ExprKind::Identifier(name) = &callee.kind {
                    self.vararg_fns.contains(name.as_str())
                } else {
                    false
                };
                if is_vararg {
                    let ret_ty = Type::new_var();
                    if let Type::Fn(_, fn_ret) = self.apply_subst(final_callee_ty) {
                        self.unify(&ret_ty, &fn_ret, expr.span);
                    }
                    self.apply_subst(ret_ty)
                } else {
                    let ret_ty = Type::new_var();
                    let expected_fn = Type::Fn(arg_types, Box::new(ret_ty.clone()));
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
                if let ExprKind::Identifier(name) = &obj.kind {
                    let mangled = format!("{}::{}", name, attr);
                    if let Some(ty) = self.lookup_type(&mangled) {
                        return ty;
                    }
                }

                let obj_ty = self.check_expr(obj);
                let resolved_obj = self.apply_subst(obj_ty);
                if let Type::Struct(ref struct_name) = resolved_obj {
                    let mangled = format!("{}::{}", struct_name, attr);
                    if let Some(ty) = self.lookup_type(&mangled) {
                        return ty;
                    }

                    // Check if it's a known field.
                    if let Some(ty) = self.field_types.get(&(struct_name.clone(), attr.clone())) {
                        return ty.clone();
                    }
                }

                if attr == "copy" {
                    return Type::Fn(vec![], Box::new(resolved_obj));
                }

                if attr == "str" {
                    return Type::Fn(vec![], Box::new(Type::Str));
                }
                if attr == "int" || attr == "i64" {
                    return Type::Fn(vec![], Box::new(Type::Int));
                }
                match attr.as_str() {
                    "i32" => return Type::Fn(vec![], Box::new(Type::I32)),
                    "i16" => return Type::Fn(vec![], Box::new(Type::I16)),
                    "i8" => return Type::Fn(vec![], Box::new(Type::I8)),
                    "u64" => return Type::Fn(vec![], Box::new(Type::U64)),
                    "u32" => return Type::Fn(vec![], Box::new(Type::U32)),
                    "u16" => return Type::Fn(vec![], Box::new(Type::U16)),
                    "u8" => return Type::Fn(vec![], Box::new(Type::U8)),
                    "float" | "f64" => return Type::Fn(vec![], Box::new(Type::Float)),
                    "f32" => return Type::Fn(vec![], Box::new(Type::F32)),
                    _ => {}
                }
                if attr == "bool" {
                    return Type::Fn(vec![], Box::new(Type::Bool));
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
                            if let StmtKind::ExprStmt(e) = &stmt.kind {
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
                        Type::Enum(enum_name) => {
                            if let Some(all_variants) = self.enum_variants.get(enum_name) {
                                for v in all_variants {
                                    if !matched_variants.contains(v) {
                                        self.errors.push(SemanticError::Custom {
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
                                if let Type::Enum(en) = ty
                                    && let Some(all_variants) = self.enum_variants.get(en)
                                {
                                    for v in all_variants {
                                        if !matched_variants.contains(v) {
                                            self.errors.push(SemanticError::Custom {
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
                    return variants[0].clone();
                }
                inner_ty
            }

            ExprKind::Await(inner) => {
                let inner_ty = self.check_expr(inner);
                match inner_ty {
                    Type::Future(t) => *t,
                    Type::Any | Type::Var(_) => Type::Any,
                    other => {
                        self.errors.push(SemanticError::Custom {
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
                        && let StmtKind::ExprStmt(e) = &s.kind
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

    fn check_binop(&mut self, op: &BinOp, l: &Type, r: &Type, span: Span) -> Type {
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

    fn check_aug_op(
        &mut self,
        _op: &crate::parser::AugOp,
        target: &Type,
        val: &Type,
        span: Span,
    ) -> Type {
        self.unify(target, val, span);
        self.apply_subst(target.clone())
    }

    fn expect_truthy(&mut self, _ty: &Type, _span: Span) {}

    fn bind_for_target(&mut self, target: &ForTarget, iter_ty: &Type, span: Span) {
        let resolved = self.apply_subst(iter_ty.clone());
        let elem_ty = match resolved {
            Type::List(inner) => *inner,
            Type::Set(inner) => *inner,
            Type::Dict(k, _) => *k,
            Type::Str => Type::Str,
            Type::Tuple(elems) => {
                if elems.is_empty() {
                    Type::Any
                } else {
                    let common = Type::new_var();
                    for e in &elems {
                        self.unify(&common, e, span);
                    }
                    self.apply_subst(common)
                }
            }
            _ => Type::new_var(),
        };

        match target {
            ForTarget::Name(name, _) => {
                self.define_type(name, elem_ty, true);
            }
            ForTarget::Tuple(names) => match self.apply_subst(elem_ty) {
                Type::Tuple(elems) if elems.len() == names.len() => {
                    for ((name, _), ty) in names.iter().zip(elems) {
                        self.define_type(name, ty, true);
                    }
                }
                _ => {
                    for (name, _) in names {
                        self.define_type(name, Type::new_var(), false);
                    }
                }
            },
        }
    }

    fn check_comp_clauses(&mut self, clauses: &[crate::parser::CompClause], span: Span) {
        for clause in clauses {
            let iter_ty = self.check_expr(&clause.iter);
            self.bind_for_target(&clause.target, &iter_ty, span);
            if let Some(cond) = &clause.condition {
                self.check_expr(cond);
            }
        }
    }

    fn unify(&mut self, t1: &Type, t2: &Type, span: Span) {
        let t1 = self.apply_subst(t1.clone());
        let t2 = self.apply_subst(t2.clone());

        if t1 == t2 {
            return;
        }

        match (&t1, &t2) {
            (Type::Any, _) | (_, Type::Any) => {}
            (Type::Never, _) | (_, Type::Never) => {}

            (Type::Var(id), other) | (other, Type::Var(id)) => {
                if self.occurs_check(*id, other) {
                    self.errors.push(SemanticError::Custom {
                        msg: "recursive type detected during unification".into(),
                        span,
                    });
                } else {
                    self.substitutions.insert(*id, other.clone());
                }
            }

            (Type::List(a), Type::List(b)) => self.unify(a, b, span),
            (Type::Set(a), Type::Set(b)) => self.unify(a, b, span),
            (Type::Future(a), Type::Future(b)) => self.unify(a, b, span),

            (Type::Dict(k1, v1), Type::Dict(k2, v2)) => {
                self.unify(k1, k2, span);
                self.unify(v1, v2, span);
            }

            (Type::Tuple(a), Type::Tuple(b)) => {
                if a.len() != b.len() {
                    self.errors.push(SemanticError::Custom {
                        msg: format!(
                            "tuple length mismatch: expected {}, found {}",
                            a.len(),
                            b.len()
                        ),
                        span,
                    });
                } else {
                    for (x, y) in a.iter().zip(b.iter()) {
                        self.unify(x, y, span);
                    }
                }
            }

            (Type::Fn(p1, r1), Type::Fn(p2, r2)) => {
                if p1.len() != p2.len() {
                    self.errors.push(SemanticError::Custom {
                        msg: format!(
                            "function arity mismatch: expected {}, found {}",
                            p1.len(),
                            p2.len()
                        ),
                        span,
                    });
                } else {
                    for (a, b) in p1.iter().zip(p2.iter()) {
                        self.unify(a, b, span);
                    }
                    self.unify(r1, r2, span);
                }
            }

            (Type::Struct(a_name), Type::Struct(b_name)) => {
                if a_name != b_name {
                    self.errors.push(SemanticError::Custom {
                        msg: format!("type mismatch: expected `{}`, found `{}`", t1, t2),
                        span,
                    });
                }
            }

            // T is a subtype of any union that contains T.
            (other, Type::Union(members)) | (Type::Union(members), other) => {
                if !members.contains(other) {
                    self.errors.push(SemanticError::Custom {
                        msg: format!("type mismatch: expected `{}`, found `{}`", t2, t1),
                        span,
                    });
                }
            }

            (_t1_match, _t2_match) => {
                self.errors.push(SemanticError::Custom {
                    msg: format!("type mismatch: expected `{}`, found `{}`", t1, t2),
                    span,
                });
            }
        }
    }

    fn occurs_check(&self, id: usize, ty: &Type) -> bool {
        match ty {
            Type::Var(other_id) => {
                if id == *other_id {
                    return true;
                }
                if let Some(resolved) = self.substitutions.get(other_id) {
                    return self.occurs_check(id, resolved);
                }
                false
            }
            Type::List(inner) | Type::Set(inner) => self.occurs_check(id, inner),
            Type::Dict(k, v) => self.occurs_check(id, k) || self.occurs_check(id, v),
            Type::Tuple(elems) => elems.iter().any(|e| self.occurs_check(id, e)),
            Type::Fn(params, ret) => {
                params.iter().any(|p| self.occurs_check(id, p)) || self.occurs_check(id, ret)
            }
            Type::Ref(inner) | Type::MutRef(inner) => self.occurs_check(id, inner),
            _ => false,
        }
    }

    fn apply_subst(&mut self, ty: Type) -> Type {
        match ty {
            Type::Var(id) => {
                if let Some(t) = self.substitutions.get(&id).cloned() {
                    let resolved = self.apply_subst(t);
                    self.substitutions.insert(id, resolved.clone());
                    resolved
                } else {
                    Type::Var(id)
                }
            }
            Type::List(inner) => Type::List(Box::new(self.apply_subst(*inner))),
            Type::Set(inner) => Type::Set(Box::new(self.apply_subst(*inner))),
            Type::Dict(k, v) => Type::Dict(
                Box::new(self.apply_subst(*k)),
                Box::new(self.apply_subst(*v)),
            ),
            Type::Tuple(elems) => {
                Type::Tuple(elems.into_iter().map(|e| self.apply_subst(e)).collect())
            }
            Type::Fn(params, ret) => Type::Fn(
                params.into_iter().map(|p| self.apply_subst(p)).collect(),
                Box::new(self.apply_subst(*ret)),
            ),
            Type::Ref(inner) => Type::Ref(Box::new(self.apply_subst(*inner))),
            Type::MutRef(inner) => Type::MutRef(Box::new(self.apply_subst(*inner))),
            _ => ty,
        }
    }

    fn resolve_type_expr(&self, expr: &TypeExpr) -> Type {
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
                    if let Some(Type::Enum(e)) = self.lookup_type(name) {
                        Type::Enum(e)
                    } else {
                        Type::Struct(name.clone())
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
                ("Future", 1) => Type::Future(Box::new(self.resolve_type_expr(&args[0]))),
                _ => Type::Struct(name.clone()),
            },
            TypeExprKind::List(inner) => Type::List(Box::new(self.resolve_type_expr(inner))),
            TypeExprKind::Dict(k, v) => Type::Dict(
                Box::new(self.resolve_type_expr(k)),
                Box::new(self.resolve_type_expr(v)),
            ),
            TypeExprKind::Tuple(types) => {
                let mut resolved = Vec::new();
                for ty in types {
                    resolved.push(self.resolve_type_expr(ty));
                }
                Type::Tuple(resolved)
            }
            TypeExprKind::Fn { params, ret } => Type::Fn(
                params.iter().map(|p| self.resolve_type_expr(p)).collect(),
                Box::new(self.resolve_type_expr(ret)),
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
        }
    }

    fn check_pattern(&mut self, pattern: &MatchPattern, match_ty: &Type, span: Span) {
        match pattern {
            MatchPattern::Wildcard => {}
            MatchPattern::Identifier(name) => {
                self.define_type(name, match_ty.clone(), false);
            }
            MatchPattern::Variant(v_name, inner_patterns) => {
                // Resolve the enum that owns this variant — either direct Enum or a Union member.
                let resolved_enum = match match_ty {
                    Type::Enum(name) => Some(name.clone()),
                    Type::Union(members) => members.iter().find_map(|ty| {
                        if let Type::Enum(en) = ty {
                            let mangled = format!("{}::{}", en, v_name);
                            if self.lookup_type(&mangled).is_some() {
                                Some(en.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }),
                    _ => None,
                };

                if let Some(enum_name) = resolved_enum {
                    let variant_mangled = format!("{}::{}", enum_name, v_name);
                    if let Some(Type::Fn(param_types, _)) = self.lookup_type(&variant_mangled) {
                        if param_types.len() == inner_patterns.len() {
                            for (p, p_ty) in inner_patterns.iter().zip(param_types) {
                                self.check_pattern(p, &p_ty, span);
                            }
                        } else {
                            self.errors.push(SemanticError::Custom {
                                msg: format!(
                                    "expected {} arguments for variant {}, found {}",
                                    param_types.len(),
                                    v_name,
                                    inner_patterns.len()
                                ),
                                span,
                            });
                        }
                    } else {
                        self.errors.push(SemanticError::UndefinedName {
                            name: variant_mangled,
                            span,
                        });
                    }
                } else {
                    self.errors.push(SemanticError::Custom {
                        msg: format!("expected Enum or Union type, found {}", match_ty),
                        span,
                    });
                }
            }
            MatchPattern::Literal(expr) => {
                let expr_ty = self.check_expr(expr);
                self.unify(match_ty, &expr_ty, span);
            }
        }
    }
}
