use super::error::SemanticError;
use super::types::Type;
use crate::parser::{
    BinOp, CallArg, Expr, ExprKind, ForTarget, MatchPattern, Program, Stmt, StmtKind, TypeExpr,
    TypeExprKind, UnaryOp,
};
use crate::span::Span;
use rustc_hash::FxHashMap as HashMap;

// type checking
pub struct TypeChecker {
    substitutions: HashMap<usize, Type>,
    pub expr_types: HashMap<usize, Type>,
    pub type_env: Vec<HashMap<String, Type>>,
    current_return_type: Option<Type>,
    pub errors: Vec<SemanticError>,
    pub class_hierarchy: HashMap<String, Vec<String>>,
    mut_env: Vec<HashMap<String, bool>>,
    pub field_types: HashMap<(String, String), Type>,
    pub enum_variants: HashMap<String, Vec<String>>,
    current_class: Option<String>,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut global_env = HashMap::default();

        // built-ins
        let builtins = [
            ("print", Type::Fn(vec![Type::Any], Box::new(Type::Int))),
            ("str", Type::Fn(vec![Type::Any], Box::new(Type::Str))),
            ("int", Type::Fn(vec![Type::Any], Box::new(Type::Int))),
            ("float", Type::Fn(vec![Type::Any], Box::new(Type::Float))),
            ("bool", Type::Fn(vec![Type::Any], Box::new(Type::Bool))),
            ("type", Type::Fn(vec![Type::Any], Box::new(Type::Str))),
            ("len", Type::Fn(vec![Type::Any], Box::new(Type::Int))),
            (
                "list_new",
                Type::Fn(vec![Type::Int], Box::new(Type::List(Box::new(Type::Any)))),
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
            class_hierarchy: HashMap::default(),
            mut_env: vec![HashMap::default()],
            field_types: HashMap::default(),
            enum_variants: HashMap::default(),
            current_class: None,
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
                    if let Type::Class(class_name) = resolved_obj {
                        self.field_types.insert((class_name, attr.clone()), val_ty);
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
                ..
            } => {
                let ret_ty = return_type
                    .as_ref()
                    .map(|ann| self.resolve_type_expr(ann))
                    .unwrap_or_else(Type::new_var);
                let mut param_types = Vec::with_capacity(params.len());
                for param in params {
                    let p_ty = param
                        .type_ann
                        .as_ref()
                        .map(|ann| self.resolve_type_expr(ann))
                        .unwrap_or_else(Type::new_var);
                    param_types.push(p_ty);
                }

                let final_name = if let Some(class_name) = &self.current_class {
                    format!("{}::{}", class_name, name)
                } else {
                    name.clone()
                };

                let fn_ty = Type::Fn(param_types.clone(), Box::new(ret_ty.clone()));
                if self.current_class.is_some() && self.type_env.len() >= 2 {
                    let outer_idx = self.type_env.len() - 2;
                    self.type_env[outer_idx].insert(final_name.clone(), fn_ty.clone());
                } else {
                    self.define_type(&final_name, fn_ty.clone(), false);
                }

                self.enter_scope();
                let prev_ret = self.current_return_type.take();
                self.current_return_type = Some(ret_ty);

                for (i, (param, mut p_ty)) in params.iter().zip(param_types).enumerate() {
                    if i == 0 && self.current_class.is_some() && param.name == "self" {
                        p_ty = Type::Class(self.current_class.clone().unwrap());
                    }
                    self.define_type(&param.name, p_ty, param.is_mut);
                }

                for s in body {
                    self.check_stmt(s);
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

            StmtKind::Class { name, bases, body } => {
                let mut base_names = Vec::new();
                for base in bases {
                    if let ExprKind::Identifier(base_name) = &base.kind {
                        base_names.push(base_name.clone());
                    }
                }
                self.class_hierarchy.insert(name.clone(), base_names);
                self.define_type(name, Type::Class(name.clone()), false);

                let prev_class = self.current_class.take();
                self.current_class = Some(name.clone());

                self.enter_scope();
                for s in body {
                    self.check_stmt(s);
                }
                self.leave_scope();

                self.current_class = prev_class;
            }

            StmtKind::Try {
                body,
                handlers,
                else_body,
                finally_body,
            } => {
                self.check_block(body);
                for handler in handlers {
                    if let Some(exc) = &handler.exc_type {
                        self.check_expr(exc);
                    }
                    self.enter_scope();
                    if let Some(name) = &handler.name {
                        self.define_type(name, Type::Any, false);
                    }
                    for s in &handler.body {
                        self.check_stmt(s);
                    }
                    self.leave_scope();
                }
                if let Some(body) = else_body {
                    self.check_block(body);
                }
                if let Some(body) = finally_body {
                    self.check_block(body);
                }
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

            StmtKind::Raise(Some(expr)) => {
                self.check_expr(expr);
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
            | StmtKind::Raise(None)
            | StmtKind::Import(_)
            | StmtKind::FromImport { .. } => {}
            StmtKind::Enum { name, variants } => {
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
            ExprKind::Null => Type::Null,

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

                if let Type::Class(name) = resolved_callee {
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
                        for (p, a) in params.iter().skip(1).zip(arg_types) {
                            self.unify(p, &a, expr.span);
                        }
                    }

                    return Type::Class(name);
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
                let ret_ty = Type::new_var();
                let expected_fn = Type::Fn(arg_types, Box::new(ret_ty.clone()));
                self.unify(&callee_ty, &expected_fn, expr.span);
                self.apply_subst(ret_ty)
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
                if let Type::Class(ref class_name) = resolved_obj {
                    let mut queue = vec![class_name.clone()];
                    let mut seen = std::collections::HashSet::new();

                    while let Some(current) = queue.pop() {
                        if !seen.insert(current.clone()) {
                            continue;
                        }

                        let mangled = format!("{}::{}", current, attr);
                        if let Some(ty) = self.lookup_type(&mangled) {
                            return ty;
                        }

                        // Check if it's a known field.
                        if let Some(ty) = self.field_types.get(&(current.clone(), attr.clone())) {
                            return ty.clone();
                        }

                        // check base classes
                        if let Some(bases) = self.class_hierarchy.get(&current) {
                            for base in bases {
                                queue.push(base.clone());
                            }
                        }
                    }
                }

                if attr == "copy" {
                    return Type::Fn(vec![], Box::new(resolved_obj));
                }

                if attr == "str" {
                    return Type::Fn(vec![], Box::new(Type::Str));
                }
                if attr == "int" {
                    return Type::Fn(vec![], Box::new(Type::Int));
                }
                if attr == "float" {
                    return Type::Fn(vec![], Box::new(Type::Float));
                }
                if attr == "bool" {
                    return Type::Fn(vec![], Box::new(Type::Bool));
                }

                Type::new_var()
            }

            ExprKind::Walrus { name, value } => {
                let val_ty = self.check_expr(value);
                self.define_type(name, val_ty.clone(), false);
                val_ty
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
                    
                    match &case.pattern {
                        crate::parser::ast::MatchPattern::Variant(v_name, _) => {
                            matched_variants.insert(v_name.clone());
                        }
                        _ => {}
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
                    if let Type::Enum(enum_name) = &match_ty {
                        if let Some(all_variants) = self.enum_variants.get(enum_name) {
                            for v in all_variants {
                                if !matched_variants.contains(v) {
                                    self.errors.push(SemanticError::Custom {
                                        msg: format!("non-exhaustive patterns: variant {} not covered", v),
                                        span: expr.span,
                                    });
                                }
                            }
                        }
                    }
                }
                
                return_ty
            }
        }
    }

    fn check_binop(&mut self, op: &BinOp, l: &Type, r: &Type, span: Span) -> Type {
        match op {
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::FloorDiv
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
            | BinOp::NotIn
            | BinOp::Is
            | BinOp::IsNot => Type::Bool,
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
            ForTarget::Tuple(names, _) => match self.apply_subst(elem_ty) {
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

        match (t1, t2) {
            (Type::Any, _) | (_, Type::Any) => {}
            (Type::Never, _) | (_, Type::Never) => {}

            (Type::Var(id), other) | (other, Type::Var(id)) => {
                if self.occurs_check(id, &other) {
                    self.errors.push(SemanticError::Custom {
                        msg: "recursive type detected during unification".into(),
                        span,
                    });
                } else {
                    self.substitutions.insert(id, other);
                }
            }

            (Type::List(a), Type::List(b)) => self.unify(&a, &b, span),
            (Type::Set(a), Type::Set(b)) => self.unify(&a, &b, span),

            (Type::Dict(k1, v1), Type::Dict(k2, v2)) => {
                self.unify(&k1, &k2, span);
                self.unify(&v1, &v2, span);
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
                    self.unify(&r1, &r2, span);
                }
            }

            (t1, t2) => {
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
                "float" | "f64" => Type::Float,
                "str" => Type::Str,
                "bool" => Type::Bool,
                "None" => Type::Null,
                "Any" => Type::Any,
                "Never" => Type::Never,
                _ => {
                    if let Some(Type::Enum(e)) = self.lookup_type(name) {
                        Type::Enum(e)
                    } else {
                        Type::Class(name.clone())
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
                _ => Type::Class(name.clone()),
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
            ),
            TypeExprKind::Ref(inner) => Type::Ref(Box::new(self.resolve_type_expr(inner))),
            TypeExprKind::MutRef(inner) => Type::MutRef(Box::new(self.resolve_type_expr(inner))),
        }
    }

    fn check_pattern(&mut self, pattern: &MatchPattern, match_ty: &Type, span: Span) {
        match pattern {
            MatchPattern::Wildcard => {}
            MatchPattern::Identifier(name) => {
                self.define_type(name, match_ty.clone(), false);
            }
            MatchPattern::Variant(v_name, inner_patterns) => {
                if let Type::Enum(enum_name) = match_ty {
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
                        msg: format!("expected Enum type, found {}", match_ty),
                        span,
                    });
                }
            }
        }
    }
}
