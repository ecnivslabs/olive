//! Checker arms for bare-function sequence builtins added in E3.3 onward
//! (`sorted`, `reversed`, `any`, `all`). Kept out of `expr.rs`, which is
//! already oversized, so new builtins land here instead of growing it further.

use super::TypeChecker;
use crate::parser::{CallArg, Expr};
use crate::semantic::types::Type;
use crate::span::Span;

fn arg_expr(a: &CallArg) -> &Expr {
    match a {
        CallArg::Positional(e)
        | CallArg::Keyword(_, e)
        | CallArg::Splat(e)
        | CallArg::KwSplat(e) => e,
    }
}

impl TypeChecker {
    /// `sorted(xs) -> [T]` and `reversed(xs) -> [T]`: both accept any list and
    /// return the same element type, leaving the source list untouched.
    fn check_sorted_reversed(&mut self, name: &str, arg: &Expr, span: Span) -> Type {
        let raw = self.check_expr(arg);
        let arg_ty = self.apply_subst(raw);
        match &arg_ty {
            Type::List(elem) => {
                if name == "sorted" {
                    self.check_struct_sort_needs_lt(elem, span);
                }
                arg_ty.clone()
            }
            Type::Any => Type::List(Box::new(Type::Any)),
            _ => {
                self.errors
                    .push(crate::semantic::error::SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0404",
                            format!("`{name}` requires a list argument, got `{arg_ty}`"),
                            span,
                        )
                        .label("expected a list"),
                    ));
                arg_ty
            }
        }
    }

    /// E6.3: `sort`/`sorted` with no `key=` on a struct element list needs
    /// `__lt__` -- without it the native int/float/str comparator would run
    /// on raw struct pointers, a meaningless order that still typechecks.
    pub(super) fn check_struct_sort_needs_lt(&mut self, elem_ty: &Type, span: Span) {
        let Type::Struct(struct_name, ..) = self.apply_subst(elem_ty.clone()) else {
            return;
        };
        if self
            .lookup_type(&format!("{struct_name}::__lt__"))
            .is_none()
        {
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0404",
                        format!("`{struct_name}` has no `__lt__` defined"),
                        span,
                    )
                    .label("sorting requires ordering; define `__lt__` or pass `key=`"),
                ));
        }
    }

    /// `any(xs) -> bool` and `all(xs) -> bool`: over `[bool]` or an
    /// `[Any]`/`Any` list, whose elements are checked for truthiness at
    /// runtime the same way `if x:` on an `Any` condition is (E1 step 1).
    fn check_any_all(&mut self, name: &str, arg: &Expr, span: Span) -> Type {
        let raw = self.check_expr(arg);
        let arg_ty = self.apply_subst(raw);
        let ok = match &arg_ty {
            Type::List(elem) => matches!(elem.as_ref(), Type::Bool | Type::Any),
            Type::Any => true,
            _ => false,
        };
        if !ok {
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0404",
                        format!("`{name}` requires a list of bool, got `{arg_ty}`"),
                        span,
                    )
                    .label("expected [bool]"),
                ));
        }
        Type::Bool
    }

    /// `str * int` / `[T] * int` and their int-on-left mirrors (E3.4).
    /// `None` when `l`/`r` isn't a repeat-operator combination, so
    /// `check_binop`'s general numeric path still handles plain `*`.
    pub(super) fn check_repeat_mul(&self, l: &Type, r: &Type) -> Option<Type> {
        let is_int = |t: &Type| matches!(t, Type::Int | Type::IntegerLiteral(_));
        let is_seq = |t: &Type| matches!(t, Type::Str | Type::List(_));
        match (l, r) {
            (a, b) if is_seq(a) && is_int(b) => Some(a.clone()),
            (a, b) if is_int(a) && is_seq(b) => Some(b.clone()),
            _ => None,
        }
    }

    /// `x in a..b` / `x not in a..b` (E3.9): the left operand must be `int`
    /// (or `Any`). `right` is only inspected syntactically here, so this is a
    /// no-op unless it's literally a range expression.
    pub(super) fn check_in_range_operand(
        &mut self,
        left_ty: &Type,
        right: &crate::parser::Expr,
        span: Span,
    ) {
        let crate::parser::ExprKind::Range { step, .. } = &right.kind else {
            return;
        };
        if let Some(step_expr) = step {
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0431",
                        "stepped range used with `in`",
                        step_expr.span,
                    )
                    .label("membership on a stepped range is not supported")
                    .help("use a plain range with `in`, or a step-aware condition"),
                ));
        }
        let resolved = self.apply_subst(left_ty.clone());
        if !matches!(resolved, Type::Int | Type::IntegerLiteral(_) | Type::Any) {
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0404",
                        format!("`in` on an int range requires an int operand, got `{resolved}`"),
                        span,
                    )
                    .label("expected int"),
                ));
        }
    }

    /// Entry point from the `Call` arm: `None` when `name` isn't one of these
    /// builtins, arity is wrong, or a user binding shadows the name.
    pub(super) fn check_sequence_builtin_call(
        &mut self,
        name: &str,
        args: &[CallArg],
        span: Span,
    ) -> Option<Type> {
        if self.lookup_type(name).is_some() {
            return None;
        }
        if name == "sorted" && args.len() == 2 {
            return Some(self.check_sorted_with_key(args, span));
        }
        if args.len() != 1 {
            return None;
        }
        let arg = arg_expr(&args[0]);
        match name {
            "sorted" | "reversed" => Some(self.check_sorted_reversed(name, arg, span)),
            "any" | "all" => Some(self.check_any_all(name, arg, span)),
            _ => None,
        }
    }

    /// `sorted(xs, key=f) -> [T]` (E5.5): checks `f` against the list's own
    /// element type as the expected fn signature (`check_expr_expecting`),
    /// same as any other call argument's context -- an unannotated `f`
    /// param infers from the list, not from a fresh, uninferable var.
    fn check_sorted_with_key(&mut self, args: &[CallArg], span: Span) -> Type {
        let list_expr = arg_expr(&args[0]);
        let raw = self.check_expr(list_expr);
        let arg_ty = self.apply_subst(raw);
        let elem_ty = match &arg_ty {
            Type::List(e) => (**e).clone(),
            Type::Any => Type::Any,
            _ => {
                self.errors
                    .push(crate::semantic::error::SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0404",
                            format!("`sorted` requires a list argument, got `{arg_ty}`"),
                            span,
                        )
                        .label("expected a list"),
                    ));
                return arg_ty;
            }
        };
        let result_ty = match &arg_ty {
            Type::Any => Type::List(Box::new(Type::Any)),
            _ => arg_ty,
        };
        let CallArg::Keyword(kw_name, key_expr) = &args[1] else {
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0403",
                        "`sorted`'s second argument must be `key=...`",
                        span,
                    )
                    .label("expected a `key` keyword argument"),
                ));
            return result_ty;
        };
        if kw_name != "key" {
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0403",
                        format!("unknown keyword argument `{kw_name}` to `sorted`"),
                        span,
                    )
                    .label("`sorted` only accepts `key`"),
                ));
            return result_ty;
        }
        let hint = Type::Fn(vec![elem_ty.clone()], Box::new(Type::Any), vec![]);
        let key_ty = self.check_expr_expecting(key_expr, &hint);
        self.check_sort_key(&elem_ty, key_expr, key_ty, span);
        result_ty
    }

    /// Shared `key=` validation for `sorted(xs, key=f)` and `xs.sort(key=f)`:
    /// `f: fn(elem) -> K`, `K` orderable -- the same set the native
    /// `__olive_list_sort_{int,float,str}` runtime already supports.
    pub(super) fn check_sort_key(
        &mut self,
        elem_ty: &Type,
        key_expr: &Expr,
        key_ty: Type,
        span: Span,
    ) {
        let key_ty = self.apply_subst(key_ty);
        let Type::Fn(params, ret, _) = &key_ty else {
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0404",
                        format!("`key` must be a function, got `{key_ty}`"),
                        key_expr.span,
                    )
                    .label("expected `fn(T) -> K`"),
                ));
            return;
        };
        if params.len() != 1 {
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0403",
                        format!(
                            "`key` function must take exactly one argument, found {}",
                            params.len()
                        ),
                        key_expr.span,
                    )
                    .label("expected one parameter"),
                ));
            return;
        }
        // `unify`, not a raw `!=`: an unannotated param already took `elem_ty`
        // as its call-site hint, so this is normally a no-op; an explicitly
        // annotated, incompatible param needs real unification to reject it
        // (a bare equality check misclassifies e.g. two distinct
        // `IntegerLiteral` instances as mismatched).
        let param_ty = self.apply_subst(params[0].clone());
        if !matches!(param_ty, Type::Any) && !matches!(elem_ty, Type::Any) {
            self.unify(&param_ty, elem_ty, key_expr.span);
        }
        let ret_ty = self.apply_subst((**ret).clone());
        if !matches!(
            ret_ty,
            Type::Int
                | Type::IntegerLiteral(_)
                | Type::Float
                | Type::FloatLiteral(_)
                | Type::Str
                | Type::Any
        ) {
            self.errors
                .push(crate::semantic::error::SemanticError::rich(
                    crate::compile::errors::Diagnostic::error(
                        "E0404",
                        format!(
                            "`key`'s result must be orderable (int, float, or str), got `{ret_ty}`"
                        ),
                        span,
                    )
                    .label("not an orderable key type"),
                ));
        }
    }

    /// Strips `&`/`&mut` and reads the element type an iterable of `ty`
    /// yields; `Any` for anything not directly iterable (matches the `for`
    /// lowering's own unwrap loop in `lower_control.rs`, kept in lockstep).
    fn iterable_elem_ty(&mut self, ty: &Type) -> Type {
        let mut t = self.apply_subst(ty.clone());
        while let Type::Ref(inner) | Type::MutRef(inner) = t {
            t = self.apply_subst(*inner);
        }
        match t {
            Type::Str => Type::Str,
            Type::List(e) | Type::Set(e) => *e,
            Type::Dict(k, _) => *k,
            _ => Type::Any,
        }
    }

    /// `for`-head / comprehension-clause iterable typing (E4.1/E4.2):
    /// `enumerate(xs)` / `enumerate(xs, start)` and `zip(a, b)` are desugars,
    /// not real calls, so their element types come from `xs`/`a`/`b`'s own
    /// checked types rather than a generic `Any`-shaped signature. Used
    /// directly in a `for`/comprehension iterable position only; elsewhere
    /// `enumerate`/`zip` are rejected by the `Call` arm's own arm for them
    /// (`expr.rs`), since neither has a real standalone runtime entry.
    pub(super) fn check_for_iter(&mut self, iter: &Expr) -> Type {
        if let crate::parser::ExprKind::Call { callee, args } = &iter.kind
            && let crate::parser::ExprKind::Identifier(name) = &callee.kind
            && self.lookup_type(name).is_none()
        {
            match name.as_str() {
                "enumerate" if matches!(args.len(), 1 | 2) => {
                    let inner = arg_expr(&args[0]);
                    let inner_ty = self.check_expr(inner);
                    if let Some(start_arg) = args.get(1) {
                        let start_expr = arg_expr(start_arg);
                        let start_ty = self.check_expr(start_expr);
                        let resolved = self.apply_subst(start_ty.clone());
                        if !matches!(resolved, Type::Int | Type::IntegerLiteral(_)) {
                            self.errors.push(crate::semantic::error::SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0404",
                                    format!(
                                        "`enumerate`'s start argument must be an int, got `{resolved}`"
                                    ),
                                    start_expr.span,
                                )
                                .label("expected int"),
                            ));
                        }
                    }
                    let elem_ty = self.iterable_elem_ty(&inner_ty);
                    return Type::List(Box::new(Type::Tuple(vec![Type::Int, elem_ty])));
                }
                "zip" if args.len() >= 2 => {
                    if args.len() > 2 {
                        self.errors
                            .push(crate::semantic::error::SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0421",
                                    format!("`zip` takes exactly 2 iterables, got {}", args.len()),
                                    iter.span,
                                )
                                .label("extra argument")
                                .help("zip two iterables at a time; nest calls for more"),
                            ));
                    }
                    let a_ty = self.check_expr(arg_expr(&args[0]));
                    let b_ty = self.check_expr(arg_expr(&args[1]));
                    for extra in &args[2..] {
                        self.check_expr(arg_expr(extra));
                    }
                    let a_elem = self.iterable_elem_ty(&a_ty);
                    let b_elem = self.iterable_elem_ty(&b_ty);
                    return Type::List(Box::new(Type::Tuple(vec![a_elem, b_elem])));
                }
                _ => {}
            }
        }
        self.check_expr(iter)
    }

    /// `let a, *rest = xs` / `a, *rest = xs` (E4.4): unlike exact tuple
    /// destructuring (a fixed, statically-known arity), a starred target
    /// needs a runtime-length source, so it only accepts a list (or `Any`).
    /// `rest` binds to the same list type; the other names bind to its
    /// element type. Arity is never checked here -- a shortfall faults at
    /// runtime when the loop-free indexed reads run out of bounds.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn check_starred_destructure(
        &mut self,
        names: &[String],
        starred_idx: usize,
        val_ty: &Type,
        type_ann: Option<&crate::parser::TypeExpr>,
        is_mut: bool,
        span: Span,
    ) {
        let resolved = self.apply_subst(val_ty.clone());
        let elem_ty = match &resolved {
            Type::List(e) => (**e).clone(),
            Type::Any | Type::PyObject => Type::Any,
            _ => {
                self.errors
                    .push(crate::semantic::error::SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0417",
                            "cannot destructure a non-list value with a starred target",
                            span,
                        )
                        .label(format!("this value is `{resolved}`, not a list"))
                        .help("a `*name` target requires a `[T]` on the right-hand side"),
                    ));
                for name in names {
                    self.define_type(name, Type::Any, is_mut);
                }
                return;
            }
        };
        let bound_elem_ty = if let Some(ann) = type_ann {
            let expected_ty = self.resolve_type_expr(ann);
            self.unify(&expected_ty, &elem_ty, span);
            expected_ty
        } else {
            elem_ty
        };
        for (i, name) in names.iter().enumerate() {
            if i == starred_idx {
                self.define_type(name, Type::List(Box::new(bound_elem_ty.clone())), is_mut);
            } else {
                self.define_type(name, bound_elem_ty.clone(), is_mut);
            }
        }
    }

    /// The plain-assignment counterpart of [`check_starred_destructure`]:
    /// `a, *rest = xs` reassigning existing bindings rather than declaring
    /// new ones, so each target unifies against its already-known type
    /// instead of defining a fresh one.
    pub(super) fn check_starred_assign_destructure(
        &mut self,
        targets: &[Expr],
        starred_idx: usize,
        val_ty: &Type,
        span: Span,
    ) {
        let resolved = self.apply_subst(val_ty.clone());
        let elem_ty = match &resolved {
            Type::List(e) => (**e).clone(),
            Type::Any | Type::PyObject => Type::Any,
            _ => {
                self.errors
                    .push(crate::semantic::error::SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0417",
                            "cannot destructure a non-list value with a starred target",
                            span,
                        )
                        .label(format!("this value is `{resolved}`, not a list"))
                        .help("a `*name` target requires a `[T]` on the right-hand side"),
                    ));
                return;
            }
        };
        for (i, target) in targets.iter().enumerate() {
            if i == starred_idx {
                let crate::parser::ExprKind::Starred(inner) = &target.kind else {
                    continue;
                };
                self.reject_global_write(&inner.kind, span);
                let inner_ty = self.check_expr(inner);
                self.unify(&inner_ty, &Type::List(Box::new(elem_ty.clone())), span);
            } else {
                self.reject_global_write(&target.kind, span);
                let target_ty = self.check_expr(target);
                self.unify(&target_ty, &elem_ty, span);
            }
        }
    }
}
