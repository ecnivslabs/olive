use super::super::error::SemanticError;
use super::super::types::Type;
use super::TypeChecker;
use crate::parser::{TypeExpr, TypeExprKind};
use crate::span::Span;

impl TypeChecker {
    pub(super) fn unify(&mut self, t1: &Type, t2: &Type, span: Span) {
        let t1 = self.apply_subst(t1.clone());
        let t2 = self.apply_subst(t2.clone());

        if t1 == t2 {
            return;
        }

        match (&t1, &t2) {
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

            (Type::IntegerLiteral(id), other) | (other, Type::IntegerLiteral(id)) => match other {
                Type::Any | Type::PyObject => {}
                Type::Int
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Usize
                | Type::Float
                | Type::F32
                | Type::IntegerLiteral(_) => {
                    self.substitutions.insert(*id, other.clone());
                }
                Type::Var(var_id) => {
                    self.substitutions
                        .insert(*var_id, Type::IntegerLiteral(*id));
                }
                Type::Union(members) => {
                    let mut matched = false;
                    for m in members {
                        match m {
                            Type::Int
                            | Type::I8
                            | Type::I16
                            | Type::I32
                            | Type::U8
                            | Type::U16
                            | Type::U32
                            | Type::U64
                            | Type::Usize
                            | Type::Float
                            | Type::F32 => {
                                self.substitutions.insert(*id, m.clone());
                                matched = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                    if !matched {
                        self.errors.push(SemanticError::Custom {
                            msg: format!(
                                "type mismatch: expected `{}`, found integer literal",
                                other
                            ),
                            span,
                        });
                    }
                }
                _ => {
                    self.errors.push(SemanticError::Custom {
                        msg: format!("type mismatch: expected `{}`, found integer literal", other),
                        span,
                    });
                }
            },

            (Type::FloatLiteral(id), other) | (other, Type::FloatLiteral(id)) => match other {
                Type::Any | Type::PyObject => {}
                Type::Float | Type::F32 | Type::FloatLiteral(_) => {
                    self.substitutions.insert(*id, other.clone());
                }
                Type::Var(var_id) => {
                    self.substitutions.insert(*var_id, Type::FloatLiteral(*id));
                }
                Type::Union(members) => {
                    let mut matched = false;
                    for m in members {
                        match m {
                            Type::Float | Type::F32 => {
                                self.substitutions.insert(*id, m.clone());
                                matched = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                    if !matched {
                        self.errors.push(SemanticError::Custom {
                            msg: format!(
                                "type mismatch: expected `{}`, found float literal",
                                other
                            ),
                            span,
                        });
                    }
                }
                _ => {
                    self.errors.push(SemanticError::Custom {
                        msg: format!("type mismatch: expected `{}`, found float literal", other),
                        span,
                    });
                }
            },

            (Type::Any, _) | (_, Type::Any) => {}
            (Type::PyObject, _) | (_, Type::PyObject) => {}
            (Type::Never, _) | (_, Type::Never) => {}

            (Type::Ptr(a), Type::Ptr(b)) => self.unify(a, b, span),

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

            (Type::Fn(p1, r1, a1), Type::Fn(p2, r2, a2)) => {
                if p1.len() != p2.len() || a1.len() != a2.len() {
                    self.errors.push(SemanticError::Custom {
                        msg: format!("function signature mismatch: expected {}, found {}", t1, t2),
                        span,
                    });
                } else {
                    for (a, b) in p1.iter().zip(p2.iter()) {
                        self.unify(a, b, span);
                    }
                    self.unify(r1, r2, span);
                    for (x, y) in a1.iter().zip(a2.iter()) {
                        self.unify(x, y, span);
                    }
                }
            }

            (Type::U64, Type::Int) | (Type::Int, Type::U64) => {}

            (Type::Struct(name, _), Type::Int) | (Type::Int, Type::Struct(name, _))
                if self.c_ffi_structs.contains(name.as_str()) => {}

            (Type::Struct(a_name, a_args), Type::Struct(b_name, b_args)) => {
                if a_name != b_name || a_args.len() != b_args.len() {
                    self.errors.push(SemanticError::Custom {
                        msg: format!("type mismatch: expected `{}`, found `{}`", t1, t2),
                        span,
                    });
                } else {
                    for (x, y) in a_args.iter().zip(b_args.iter()) {
                        self.unify(x, y, span);
                    }
                }
            }

            (Type::Enum(a_name, a_args), Type::Enum(b_name, b_args)) => {
                if a_name != b_name || a_args.len() != b_args.len() {
                    self.errors.push(SemanticError::Custom {
                        msg: format!("type mismatch: expected `{}`, found `{}`", t1, t2),
                        span,
                    });
                } else {
                    for (x, y) in a_args.iter().zip(b_args.iter()) {
                        self.unify(x, y, span);
                    }
                }
            }

            (Type::TraitObject(a_name, a_args), Type::TraitObject(b_name, b_args)) => {
                if a_name != b_name || a_args.len() != b_args.len() {
                    self.errors.push(SemanticError::Custom {
                        msg: format!("type mismatch: expected `{}`, found `{}`", t1, t2),
                        span,
                    });
                } else {
                    for (x, y) in a_args.iter().zip(b_args.iter()) {
                        self.unify(x, y, span);
                    }
                }
            }

            (Type::TraitObject(trait_name, _), Type::Struct(struct_name, _))
            | (Type::Struct(struct_name, _), Type::TraitObject(trait_name, _)) => {
                if !self
                    .type_traits
                    .contains(&(struct_name.clone(), trait_name.clone()))
                {
                    self.errors.push(SemanticError::Custom {
                        msg: format!("type mismatch: expected `{}`, found `{}`", t1, t2),
                        span,
                    });
                }
            }

            (Type::Param(a), Type::Param(b)) => {
                if a != b {
                    self.errors.push(SemanticError::Custom {
                        msg: format!("type mismatch: expected `{}`, found `{}`", t1, t2),
                        span,
                    });
                }
            }

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

    pub(super) fn occurs_check(&self, id: usize, ty: &Type) -> bool {
        match ty {
            Type::Var(other_id) | Type::IntegerLiteral(other_id) | Type::FloatLiteral(other_id) => {
                if id == *other_id {
                    return true;
                }
                if let Some(resolved) = self.substitutions.get(other_id) {
                    return self.occurs_check(id, resolved);
                }
                false
            }
            Type::List(inner) | Type::Set(inner) | Type::Ptr(inner) => self.occurs_check(id, inner),
            Type::Dict(k, v) => self.occurs_check(id, k) || self.occurs_check(id, v),
            Type::Tuple(elems) => elems.iter().any(|e| self.occurs_check(id, e)),
            Type::Fn(params, ret, args) => {
                params.iter().any(|p| self.occurs_check(id, p))
                    || self.occurs_check(id, ret)
                    || args.iter().any(|a| self.occurs_check(id, a))
            }
            Type::Ref(inner) | Type::MutRef(inner) | Type::Future(inner) => {
                self.occurs_check(id, inner.as_ref())
            }
            Type::Struct(_, args) | Type::Enum(_, args) | Type::TraitObject(_, args) => {
                args.iter().any(|arg| self.occurs_check(id, arg))
            }
            Type::Union(members) => members.iter().any(|m| self.occurs_check(id, m)),
            _ => false,
        }
    }

    pub(super) fn apply_subst(&mut self, ty: Type) -> Type {
        self.apply_subst_impl(ty, false)
    }

    pub(super) fn apply_subst_final(&mut self, ty: Type) -> Type {
        self.apply_subst_impl(ty, true)
    }

    pub(super) fn apply_subst_impl(&mut self, ty: Type, finalize: bool) -> Type {
        match ty {
            Type::Var(id) => {
                if let Some(t) = self.substitutions.get(&id).cloned() {
                    let resolved = self.apply_subst_impl(t, finalize);
                    self.substitutions.insert(id, resolved.clone());
                    resolved
                } else {
                    Type::Var(id)
                }
            }
            Type::IntegerLiteral(id) => {
                if let Some(t) = self.substitutions.get(&id).cloned() {
                    let resolved = self.apply_subst_impl(t, finalize);
                    self.substitutions.insert(id, resolved.clone());
                    resolved
                } else {
                    if finalize {
                        Type::Int
                    } else {
                        Type::IntegerLiteral(id)
                    }
                }
            }
            Type::FloatLiteral(id) => {
                if let Some(t) = self.substitutions.get(&id).cloned() {
                    let resolved = self.apply_subst_impl(t, finalize);
                    self.substitutions.insert(id, resolved.clone());
                    resolved
                } else {
                    if finalize {
                        Type::Float
                    } else {
                        Type::FloatLiteral(id)
                    }
                }
            }
            Type::List(inner) => Type::List(Box::new(self.apply_subst_impl(*inner, finalize))),
            Type::Set(inner) => Type::Set(Box::new(self.apply_subst_impl(*inner, finalize))),
            Type::Ptr(inner) => Type::Ptr(Box::new(self.apply_subst_impl(*inner, finalize))),
            Type::Dict(k, v) => Type::Dict(
                Box::new(self.apply_subst_impl(*k, finalize)),
                Box::new(self.apply_subst_impl(*v, finalize)),
            ),
            Type::Tuple(elems) => Type::Tuple(
                elems
                    .into_iter()
                    .map(|e| self.apply_subst_impl(e, finalize))
                    .collect(),
            ),
            Type::Fn(params, ret, args) => Type::Fn(
                params
                    .into_iter()
                    .map(|p| self.apply_subst_impl(p, finalize))
                    .collect(),
                Box::new(self.apply_subst_impl(*ret, finalize)),
                args.into_iter()
                    .map(|a| self.apply_subst_impl(a, finalize))
                    .collect(),
            ),
            Type::Ref(inner) => Type::Ref(Box::new(self.apply_subst_impl(*inner, finalize))),
            Type::MutRef(inner) => Type::MutRef(Box::new(self.apply_subst_impl(*inner, finalize))),
            Type::Future(inner) => Type::Future(Box::new(self.apply_subst_impl(*inner, finalize))),
            Type::Struct(name, args) => Type::Struct(
                name,
                args.into_iter()
                    .map(|a| self.apply_subst_impl(a, finalize))
                    .collect(),
            ),
            Type::Enum(name, args) => Type::Enum(
                name,
                args.into_iter()
                    .map(|a| self.apply_subst_impl(a, finalize))
                    .collect(),
            ),
            Type::TraitObject(name, args) => Type::TraitObject(
                name,
                args.into_iter()
                    .map(|a| self.apply_subst_impl(a, finalize))
                    .collect(),
            ),
            Type::Union(members) => Type::Union(
                members
                    .into_iter()
                    .map(|m| self.apply_subst_impl(m, finalize))
                    .collect(),
            ),
            _ => ty,
        }
    }

    pub(super) fn resolve_type_expr(&self, expr: &TypeExpr) -> Type {
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
                "bytes" => Type::Bytes,
                "bool" => Type::Bool,
                "None" => Type::Null,
                "Never" => Type::Never,
                "Any" => Type::Any,
                "PyObject" => Type::PyObject,
                _ => {
                    if let Some(t) = self.lookup_type(name) {
                        t
                    } else {
                        Type::Param(name.clone())
                    }
                }
            },
            TypeExprKind::Generic(name, args) => {
                let resolved_args: Vec<Type> =
                    args.iter().map(|arg| self.resolve_type_expr(arg)).collect();
                match name.as_str() {
                    "list" if args.len() == 1 => Type::List(Box::new(resolved_args[0].clone())),
                    "set" if args.len() == 1 => Type::Set(Box::new(resolved_args[0].clone())),
                    "dict" if args.len() == 2 => Type::Dict(
                        Box::new(resolved_args[0].clone()),
                        Box::new(resolved_args[1].clone()),
                    ),
                    "Future" if args.len() == 1 => Type::Future(Box::new(resolved_args[0].clone())),
                    _ => {
                        if let Some(Type::Enum(enum_name, _)) = self.lookup_type(name) {
                            Type::Enum(enum_name, resolved_args)
                        } else if let Some(Type::TraitObject(trait_name, _)) =
                            self.lookup_type(name)
                        {
                            Type::TraitObject(trait_name, resolved_args)
                        } else {
                            Type::Struct(name.clone(), resolved_args)
                        }
                    }
                }
            }
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
                Vec::new(),
            ),
            TypeExprKind::Ref(inner) => Type::Ref(Box::new(self.resolve_type_expr(inner))),
            TypeExprKind::MutRef(inner) => Type::MutRef(Box::new(self.resolve_type_expr(inner))),
            TypeExprKind::Ptr(inner) => Type::Ptr(Box::new(self.resolve_type_expr(inner))),
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
            TypeExprKind::FixedArray(_, _) => Type::Int,
        }
    }
}
