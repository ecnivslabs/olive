use super::super::error::SemanticError;
use super::super::types::Type;
use super::TypeChecker;
use crate::parser::{TypeExpr, TypeExprKind};
use crate::span::Span;

impl TypeChecker {
    /// Whether `None` may be assigned to a value of this type. True for the
    /// pointer- and heap-backed types, where the null pointer is a real runtime
    /// value; false for the scalar value types.
    pub(super) fn is_nullable_target(t: &Type) -> bool {
        matches!(
            t,
            Type::Struct(_, _, _)
                | Type::Enum(_, _)
                | Type::TraitObject(_, _)
                | Type::PyObject
                | Type::PyNamed(_, _)
                | Type::Ptr(_)
                | Type::Ref(_)
                | Type::MutRef(_)
                | Type::Fn(_, _, _)
                | Type::List(_)
                | Type::Dict(_, _)
                | Type::Set(_)
                | Type::Tuple(_)
                | Type::Future(_)
                | Type::Vector(_, _)
        )
    }

    pub(super) fn unify(&mut self, t1: &Type, t2: &Type, span: Span) {
        let t1 = self.apply_subst(t1.clone());
        let t2 = self.apply_subst(t2.clone());

        if t1 == t2 {
            return;
        }

        match (&t1, &t2) {
            (Type::Var(id), other) | (other, Type::Var(id)) => {
                if self.occurs_check(*id, other) {
                    self.errors.push(SemanticError::rich(
                        crate::compile::errors::Diagnostic::error("E0420", "infinite type", span)
                            .label(format!("this type would contain itself: `{other}`"))
                            .note("Olive types must have a finite size known at compile time")
                            .help("introduce an indirection such as `ptr[T]` to break the cycle"),
                    ));
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
                        self.errors.push(SemanticError::literal_mismatch(
                            span,
                            other.to_string(),
                            "integer literal",
                        ));
                    }
                }
                _ => {
                    self.errors.push(SemanticError::literal_mismatch(
                        span,
                        other.to_string(),
                        "integer literal",
                    ));
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
                        self.errors.push(SemanticError::literal_mismatch(
                            span,
                            other.to_string(),
                            "float literal",
                        ));
                    }
                }
                _ => {
                    self.errors.push(SemanticError::literal_mismatch(
                        span,
                        other.to_string(),
                        "float literal",
                    ));
                }
            },

            (Type::Any, _) | (_, Type::Any) => {}
            (Type::PyObject, _) | (_, Type::PyObject) => {}
            (Type::PyNamed(m1, n1), Type::PyNamed(m2, n2)) => {
                if m1 != m2 || n1 != n2 {
                    self.errors.push(SemanticError::type_mismatch(
                        span,
                        format!("{m1}.{n1}"),
                        format!("{m2}.{n2}"),
                    ));
                }
            }
            (Type::Never, _) | (_, Type::Never) => {}

            (Type::Ptr(a), Type::Ptr(b)) => self.unify(a, b, span),

            (Type::Ref(a), Type::Ref(b)) | (Type::MutRef(a), Type::MutRef(b)) => {
                self.unify(a, b, span)
            }
            // &mut T satisfies &T (but not the reverse).
            (Type::Ref(a), Type::MutRef(b)) => self.unify(a, b, span),

            (Type::List(a), Type::List(b)) => self.unify_elem(a, b, span),
            (Type::Set(a), Type::Set(b)) => self.unify_elem(a, b, span),
            (Type::Future(a), Type::Future(b)) => self.unify(a, b, span),

            (Type::Dict(k1, v1), Type::Dict(k2, v2)) => {
                self.unify_elem(k1, k2, span);
                self.unify_elem(v1, v2, span);
            }

            (Type::Tuple(a), Type::Tuple(b)) => {
                if a.len() != b.len() {
                    self.errors.push(SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0401",
                            "tuple length mismatch",
                            span,
                        )
                        .label(format!(
                            "expected a {}-tuple, found a {}-tuple",
                            a.len(),
                            b.len()
                        ))
                        .note(format!("expected `{t1}`"))
                        .note(format!("   found `{t2}`")),
                    ));
                } else {
                    for (x, y) in a.iter().zip(b.iter()) {
                        self.unify(x, y, span);
                    }
                }
            }

            (Type::Fn(p1, r1, a1), Type::Fn(p2, r2, a2)) => {
                if p1.len() != p2.len() || a1.len() != a2.len() {
                    self.errors.push(SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0402",
                            "function signature mismatch",
                            span,
                        )
                        .label(format!(
                            "expected {} parameter(s), found {}",
                            p1.len(),
                            p2.len()
                        ))
                        .note(format!("expected `{t1}`"))
                        .note(format!("   found `{t2}`")),
                    ));
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

            (Type::Struct(name, _, _), Type::Int) | (Type::Int, Type::Struct(name, _, _))
                if self.c_ffi_structs.contains(name.as_str()) => {}

            (Type::Struct(a_name, a_args, _), Type::Struct(b_name, b_args, _)) => {
                if a_name != b_name || a_args.len() != b_args.len() {
                    self.errors.push(SemanticError::type_mismatch(
                        span,
                        t1.to_string(),
                        t2.to_string(),
                    ));
                } else {
                    for (x, y) in a_args.iter().zip(b_args.iter()) {
                        self.unify(x, y, span);
                    }
                }
            }

            (Type::Enum(a_name, a_args), Type::Enum(b_name, b_args)) => {
                if a_name != b_name || a_args.len() != b_args.len() {
                    self.errors.push(SemanticError::type_mismatch(
                        span,
                        t1.to_string(),
                        t2.to_string(),
                    ));
                } else {
                    for (x, y) in a_args.iter().zip(b_args.iter()) {
                        self.unify(x, y, span);
                    }
                }
            }

            (Type::TraitObject(a_name, a_args), Type::TraitObject(b_name, b_args)) => {
                if a_name != b_name || a_args.len() != b_args.len() {
                    self.errors.push(SemanticError::type_mismatch(
                        span,
                        t1.to_string(),
                        t2.to_string(),
                    ));
                } else {
                    for (x, y) in a_args.iter().zip(b_args.iter()) {
                        self.unify(x, y, span);
                    }
                }
            }

            (Type::TraitObject(trait_name, _), Type::Struct(struct_name, _, _))
            | (Type::Struct(struct_name, _, _), Type::TraitObject(trait_name, _)) => {
                if !self
                    .type_traits
                    .contains(&(struct_name.clone(), trait_name.clone()))
                {
                    self.errors.push(SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0415",
                            format!("`{struct_name}` does not implement trait `{trait_name}`"),
                            span,
                        )
                        .label(format!(
                            "the trait `{trait_name}` is not implemented for `{struct_name}`"
                        ))
                        .help(format!(
                            "add `impl {trait_name} for {struct_name}:` with the required methods"
                        )),
                    ));
                }
            }

            (Type::Param(a), Type::Param(b)) => {
                if a != b {
                    self.errors.push(SemanticError::type_mismatch(
                        span,
                        t1.to_string(),
                        t2.to_string(),
                    ));
                }
            }

            // `None` is the null pointer for any heap- or pointer-backed type,
            // so it may stand in for one that is not yet initialised. Value
            // types stay strict, since there `None` and `0` would be
            // indistinguishable.
            (Type::Null, other) | (other, Type::Null) if Self::is_nullable_target(other) => {}

            // Two unions fit when one side's members each fit somewhere in
            // the other: same set in a different order, or an inferred subset
            // (e.g. `PyObject | Error` against a declared `T | Error`).
            (Type::Union(a_ms), Type::Union(b_ms)) => {
                let a_fits = a_ms
                    .iter()
                    .all(|m| b_ms.iter().any(|y| self.probe_compatible(m, y)));
                let b_fits = b_ms
                    .iter()
                    .all(|m| a_ms.iter().any(|y| self.probe_compatible(m, y)));
                if !a_fits && !b_fits {
                    self.errors.push(SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0400",
                            "mismatched types",
                            span,
                        )
                        .label(format!("`{t1}` is not compatible with `{t2}`"))
                        .note(format!("expected `{t1}`"))
                        .note(format!("   found `{t2}`")),
                    ));
                }
            }

            (other, Type::Union(members)) | (Type::Union(members), other) => {
                // Struct satisfies union if it implements one of the union's trait-object members.
                let implements_member = if let Type::Struct(sname, _, _) = other {
                    members.iter().any(|m| {
                        matches!(m, Type::TraitObject(tname, _)
                            if self.type_traits.contains(&(sname.clone(), tname.clone())))
                    })
                } else {
                    false
                };
                if !members.contains(other) && !implements_member {
                    self.errors.push(SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0400",
                            "mismatched types",
                            span,
                        )
                        .label(format!("`{t1}` is not a member of `{t2}`"))
                        .note(format!("expected one of the members of `{t2}`"))
                        .note(format!("   found `{t1}`")),
                    ));
                }
            }

            (_t1_match, _t2_match) => {
                self.errors.push(SemanticError::type_mismatch(
                    span,
                    t1.to_string(),
                    t2.to_string(),
                ));
            }
        }
    }

    /// Non-binding compatibility probe: whether `unify` would accept the pair
    /// without an error. Conservative on structured types (exact equality),
    /// so a `false` keeps the old diagnostic.
    fn probe_compatible(&mut self, a: &Type, b: &Type) -> bool {
        let a = self.apply_subst(a.clone());
        let b = self.apply_subst(b.clone());
        if a == b {
            return true;
        }
        match (&a, &b) {
            (Type::Any, _)
            | (_, Type::Any)
            | (Type::PyObject, _)
            | (_, Type::PyObject)
            | (Type::Never, _)
            | (_, Type::Never)
            | (Type::Var(_), _)
            | (_, Type::Var(_)) => true,
            (Type::Null, other) | (other, Type::Null) => Self::is_nullable_target(other),
            (Type::Union(ms), other) | (other, Type::Union(ms)) => {
                ms.iter().any(|m| self.probe_compatible(m, other))
            }
            _ => false,
        }
    }

    /// Join of two branch types (`match` arms, ternary branches). Python and
    /// native values don't share a runtime representation, so when only one
    /// side is Python-typed the join is their union; `unify`'s `PyObject`
    /// wildcard would silently drop one side's repr and mistype the slot.
    pub(super) fn join_branch_ty(&mut self, acc: Type, next: Type, span: Span) -> Type {
        let acc = self.apply_subst(acc);
        let next = self.apply_subst(next);
        if acc == next {
            return acc;
        }
        if matches!(acc, Type::Never) {
            return next;
        }
        if matches!(next, Type::Never) {
            return acc;
        }
        let is_join_transparent = |t: &Type| matches!(t, Type::Any | Type::Var(_));
        if acc.is_py_value() != next.is_py_value()
            && !is_join_transparent(&acc)
            && !is_join_transparent(&next)
        {
            let mut members: Vec<Type> = Vec::new();
            for t in [acc, next] {
                match t {
                    Type::Union(ms) => {
                        for m in ms {
                            if !members.contains(&m) {
                                members.push(m);
                            }
                        }
                    }
                    other => {
                        if !members.contains(&other) {
                            members.push(other);
                        }
                    }
                }
            }
            return Type::Union(members);
        }
        self.unify(&acc, &next, span);
        let resolved = self.apply_subst(acc);
        if matches!(resolved, Type::Var(_)) {
            self.apply_subst(next)
        } else {
            resolved
        }
    }

    /// Types that share `Any`'s tagged-i64 representation (no additional boxing).
    fn needs_any_boxing(ty: &Type) -> bool {
        matches!(
            ty,
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
                | Type::Bool
                | Type::Null
                | Type::IntegerLiteral(_)
                | Type::FloatLiteral(_)
        )
    }

    /// Unifies container element types, erroring if a scalar aliases `Any`.
    pub(super) fn unify_elem(&mut self, a: &Type, b: &Type, span: Span) {
        let ra = self.apply_subst(a.clone());
        let rb = self.apply_subst(b.clone());
        let boxing_side = match (&ra, &rb) {
            (Type::Any, other) if Self::needs_any_boxing(other) => Some(other.clone()),
            (other, Type::Any) if Self::needs_any_boxing(other) => Some(other.clone()),
            _ => None,
        };
        if let Some(concrete) = boxing_side {
            self.errors.push(SemanticError::rich(
                crate::compile::errors::Diagnostic::error(
                    "E0425",
                    "element type cannot alias an `Any` container",
                    span,
                )
                .label(format!(
                    "this element is `{concrete}` here, but the container is also used as \
                     `Any` elsewhere; an `Any` element is stored boxed, a `{concrete}` is not"
                ))
                .help("annotate the container `Any`-typed at the point it is created"),
            ));
            return;
        }
        self.unify(a, b, span);
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
            Type::Struct(_, args, _) | Type::Enum(_, args) | Type::TraitObject(_, args) => {
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
                } else if finalize {
                    Type::Any
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
            Type::Struct(name, args, is_ffi) => Type::Struct(
                name,
                args.into_iter()
                    .map(|a| self.apply_subst_impl(a, finalize))
                    .collect(),
                is_ffi,
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
            TypeExprKind::Qualified(parts) => {
                if parts.len() >= 2 {
                    let module = &parts[0];
                    let type_name = &parts[1];
                    if let Some(type_map) = self.py_module_types.get(module)
                        && let Some(ty) = type_map.get(type_name)
                    {
                        return ty.clone();
                    }
                    if self.py_aliases.contains(module) {
                        return Type::PyNamed(module.clone(), type_name.clone());
                    }
                }
                Type::PyObject
            }
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
                // Bare collection names in type position are the collection type,
                // not the same-named builtin constructor function.
                "list" => Type::List(Box::new(Type::Any)),
                "set" => Type::Set(Box::new(Type::Any)),
                "dict" => Type::Dict(Box::new(Type::Any), Box::new(Type::Any)),
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
                            let is_ffi =
                                matches!(self.lookup_type(name), Some(Type::Struct(_, _, true)));
                            Type::Struct(name.clone(), resolved_args, is_ffi)
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

#[cfg(test)]
mod tests {
    use super::super::super::TypeChecker;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::semantic::Resolver;

    fn pipeline(src: &str) -> TypeChecker {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        let mut tc = TypeChecker::new();
        tc.check_program(&prog);
        tc
    }

    #[test]
    fn integer_literal_unifies_with_int() {
        let tc = pipeline("let x: i64 = 42\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn integer_literal_unifies_with_i8() {
        let tc = pipeline("let x: i8 = 42\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn float_literal_unifies_with_float() {
        let tc = pipeline("let x: float = 3.14\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn float_literal_unifies_with_f32() {
        let tc = pipeline("let x: f32 = 1.5\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn tuple_length_mismatch_in_return() {
        let tc = pipeline("fn f() -> (i64, i64):\n    return (1, 2, 3)\n");
        assert!(!tc.errors.is_empty());
    }

    #[test]
    fn tuple_length_match_ok() {
        let tc = pipeline("fn f() -> (i64, str):\n    return (1, \"a\")\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn struct_type_mismatch_reported() {
        let tc = pipeline(
            "struct A:\n    x: i64\nstruct B:\n    x: i64\nfn f(a: A):\n    pass\nlet b = B(1)\nf(b)\n",
        );
        assert!(!tc.errors.is_empty());
    }

    #[test]
    fn struct_type_match_ok() {
        let tc = pipeline("struct A:\n    x: i64\nfn f(a: A):\n    pass\nf(A(1))\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn function_signature_mismatch() {
        let tc = pipeline("fn f() -> i64:\n    return \"wrong\"\n");
        assert!(!tc.errors.is_empty());
    }

    #[test]
    fn union_member_ok() {
        let tc = pipeline("fn f(x: i64 | str):\n    pass\nf(42)\nf(\"hello\")\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn union_member_mismatch() {
        let tc = pipeline("let x: i64 | str = True\n");
        assert!(!tc.errors.is_empty());
    }

    #[test]
    fn unify_any_with_anything() {
        let tc = pipeline("let x: Any = 42\nlet y: Any = \"hello\"\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn unify_pyobject_with_anything() {
        let tc = pipeline("let x: PyObject = 42\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn generic_param_passed_int() {
        let tc = pipeline("fn id[T](x: T) -> T:\n    return x\nlet y = id(42)\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn generic_param_passed_str() {
        let tc = pipeline("fn id[T](x: T) -> T:\n    return x\nlet y = id(\"hi\")\n");
        assert!(tc.errors.is_empty());
    }
}
