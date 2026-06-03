use super::super::MirBuilder;
use crate::mir::ir::*;
use crate::parser::Expr;
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    pub(crate) fn lower_binop_expr(
        &mut self,
        left: &Expr,
        op: &crate::parser::BinOp,
        right: &Expr,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let r_ty = self.get_type(right.id).clone();

        // E6.1: struct operator dunders. Checked first, ahead of every
        // built-in path below (`None` tests, derived structural `==`,
        // native `BinaryOp`) -- the checker already resolved these through
        // the impl method table, so lowering just builds the ordinary call.
        let l_struct = Self::deref_struct_ty(self.get_type(left.id).clone());
        if let Some((struct_name, type_args)) = l_struct {
            use crate::parser::BinOp;
            let arith_dunder = match op {
                BinOp::Add => Some("__add__"),
                BinOp::Sub => Some("__sub__"),
                BinOp::Mul => Some("__mul__"),
                BinOp::Div => Some("__truediv__"),
                BinOp::Mod => Some("__mod__"),
                _ => None,
            };
            if let Some(dunder) = arith_dunder {
                let l_op = self.lower_expr(left);
                let r_op = self.lower_expr(right);
                let ret_ty = self.get_type(expr_id).clone();
                return self.call_struct_dunder(
                    &struct_name,
                    &type_args,
                    dunder,
                    vec![l_op, r_op],
                    ret_ty,
                    span,
                );
            }

            let has_user_eq = self.fn_meta.contains_key(&format!("{struct_name}::__eq__"));
            if matches!(op, BinOp::Eq | BinOp::NotEq) && has_user_eq {
                // `__eq__` borrows (checker-enforced `&self`): the compiler
                // may compare the same value many times (containers,
                // sorting), and a by-value `self` would free it after the
                // first call.
                let (l_ref, _) = self.borrow_iterable(left);
                let (r_ref, _) = self.borrow_iterable(right);
                let result = self.call_struct_dunder(
                    &struct_name,
                    &type_args,
                    "__eq__",
                    vec![Operand::Copy(l_ref), Operand::Copy(r_ref)],
                    Type::Bool,
                    span,
                );
                self.last_cmp_operands = Some((Operand::Copy(l_ref), Operand::Copy(r_ref)));
                if matches!(op, BinOp::Eq) {
                    return result;
                }
                return self.negate_bool(result, span);
            }

            if matches!(op, BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq) {
                // `>`/`<=`/`>=` all derive from `__lt__` (checker requires
                // it exist for a struct operand): `a>b`=`b<a`,
                // `a<=b`=`!(b<a)`, `a>=b`=`!(a<b)`.
                let (l_ref, _) = self.borrow_iterable(left);
                let (r_ref, _) = self.borrow_iterable(right);
                let (lhs, rhs, negate) = match op {
                    BinOp::Lt => (l_ref, r_ref, false),
                    BinOp::Gt => (r_ref, l_ref, false),
                    BinOp::LtEq => (r_ref, l_ref, true),
                    BinOp::GtEq => (l_ref, r_ref, true),
                    _ => unreachable!(),
                };
                let result = self.call_struct_dunder(
                    &struct_name,
                    &type_args,
                    "__lt__",
                    vec![Operand::Copy(lhs), Operand::Copy(rhs)],
                    Type::Bool,
                    span,
                );
                self.last_cmp_operands = Some((Operand::Copy(l_ref), Operand::Copy(r_ref)));
                if negate {
                    return self.negate_bool(result, span);
                }
                return result;
            }
        }

        // `null` in an `Any` is a boxed sentinel, not a bare 0, so test it via
        // the runtime null check (negated for `!=`).
        if matches!(op, crate::parser::BinOp::Eq | crate::parser::BinOp::NotEq) {
            let l_ty = self.get_type(left.id);
            // PyObject None is a singleton, not bare 0; detect syntactically since type widens.
            let is_none_lit = |e: &Expr| matches!(e.kind, crate::parser::ast::ExprKind::Null);
            let py_operand = if is_none_lit(right) && matches!(l_ty, Type::PyObject) {
                Some((left, true))
            } else if is_none_lit(left) && matches!(r_ty, Type::PyObject) {
                Some((right, false))
            } else {
                None
            };
            if let Some((operand, is_left)) = py_operand {
                let v = self.lower_expr_as_copy(operand);
                let is_none = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        is_none,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_py_is_none".to_string(),
                            )),
                            args: vec![v.clone()],
                        },
                    ),
                    span,
                );
                self.last_cmp_operands = Some(if is_left {
                    (v, Operand::Constant(Constant::None))
                } else {
                    (Operand::Constant(Constant::None), v)
                });
                if matches!(op, crate::parser::BinOp::Eq) {
                    return self.operand_for_local(is_none);
                }
                let neg = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        neg,
                        Rvalue::UnaryOp(crate::parser::UnaryOp::Not, Operand::Copy(is_none)),
                    ),
                    span,
                );
                return self.operand_for_local(neg);
            }
            let any_operand = match (&l_ty, &r_ty) {
                (Type::Any, Type::Null) => Some((left, true)),
                (Type::Null, Type::Any) => Some((right, false)),
                _ => None,
            };
            if let Some((operand, is_left)) = any_operand {
                let v = self.lower_expr_as_copy(operand);
                let is_null = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        is_null,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_any_is_null".to_string(),
                            )),
                            args: vec![v.clone()],
                        },
                    ),
                    span,
                );
                self.last_cmp_operands = Some(if is_left {
                    (v, Operand::Constant(Constant::None))
                } else {
                    (Operand::Constant(Constant::None), v)
                });
                if matches!(op, crate::parser::BinOp::Eq) {
                    return self.operand_for_local(is_null);
                }
                let neg = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        neg,
                        Rvalue::UnaryOp(crate::parser::UnaryOp::Not, Operand::Copy(is_null)),
                    ),
                    span,
                );
                return self.operand_for_local(neg);
            }

            // With `None` statically on one side the result can fold to a
            // constant. Only `None` equals `None`, and a value of a scalar type
            // that can never hold null is never equal to `None`. Unions, `Any`,
            // and reference types may carry a null at runtime, so they fall
            // through to the ordinary comparison.
            let is_scalar = |t: &Type| {
                matches!(
                    t,
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
                )
            };
            let l_null = matches!(l_ty, Type::Null);
            let r_null = matches!(r_ty, Type::Null);
            if l_null && r_null {
                self.lower_expr_as_copy(left);
                self.lower_expr_as_copy(right);
                self.last_cmp_operands = Some((
                    Operand::Constant(Constant::None),
                    Operand::Constant(Constant::None),
                ));
                return Operand::Constant(Constant::Bool(matches!(op, crate::parser::BinOp::Eq)));
            }
            if (l_null && is_scalar(&r_ty)) || (r_null && is_scalar(&l_ty)) {
                let l_op = self.lower_expr_as_copy(left);
                let r_op = self.lower_expr_as_copy(right);
                self.last_cmp_operands = Some((l_op, r_op));
                return Operand::Constant(Constant::Bool(!matches!(op, crate::parser::BinOp::Eq)));
            }
            // Comparing a pointer-backed value against `None` is a raw null
            // check, not a structural or Python-level comparison, so test the
            // pointer against 0 directly and never dereference it.
            if l_null || r_null {
                let operand = if l_null { right } else { left };
                let operand_is_left = !l_null;
                let v = self.lower_expr_as_copy(operand);
                self.last_cmp_operands = Some(if operand_is_left {
                    (v.clone(), Operand::Constant(Constant::None))
                } else {
                    (Operand::Constant(Constant::None), v.clone())
                });
                // Raw-int reinterpret before the 0 test; a PyObject-typed operand
                // would route through Python eq (boxes the 0, compares identity).
                let raw = self.new_local(Type::Int, None, false);
                self.push_statement(StatementKind::Assign(raw, Rvalue::Use(v)), span);
                let is_zero = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        is_zero,
                        Rvalue::BinaryOp(
                            crate::parser::BinOp::Eq,
                            Operand::Copy(raw),
                            Operand::Constant(Constant::Int(0)),
                        ),
                    ),
                    span,
                );
                if matches!(op, crate::parser::BinOp::Eq) {
                    return self.operand_for_local(is_zero);
                }
                let neg = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        neg,
                        Rvalue::UnaryOp(crate::parser::UnaryOp::Not, Operand::Copy(is_zero)),
                    ),
                    span,
                );
                return self.operand_for_local(neg);
            }
        }

        // Derived structural `==`/`!=`: the checker only allows this when both
        // sides resolve to the same aggregate type (or one side is `None`,
        // handled above as a raw sentinel test), so the left operand's
        // descriptor is valid for both.
        if matches!(op, crate::parser::BinOp::Eq | crate::parser::BinOp::NotEq) {
            let l_ty = self.get_type(left.id);
            let needs_structural = |t: &Type| {
                matches!(
                    t,
                    Type::Struct(..)
                        | Type::Enum(..)
                        | Type::Tuple(_)
                        | Type::List(_)
                        | Type::Set(_)
                        | Type::Dict(_, _)
                )
            };
            if needs_structural(&l_ty) || needs_structural(&r_ty) {
                let l_op = self.lower_expr_as_copy(left);
                let r_op = self.lower_expr_as_copy(right);
                self.last_cmp_operands = Some((l_op.clone(), r_op.clone()));
                let call_tmp = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        call_tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_eq_typed".to_string(),
                            )),
                            args: vec![l_op, r_op],
                        },
                    ),
                    span,
                );
                if matches!(op, crate::parser::BinOp::Eq) {
                    return self.operand_for_local(call_tmp);
                }
                let not_tmp = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        not_tmp,
                        Rvalue::UnaryOp(crate::parser::UnaryOp::Not, Operand::Copy(call_tmp)),
                    ),
                    span,
                );
                return self.operand_for_local(not_tmp);
            }
        }

        // Membership in an `[Any]`/`{Any}` compares the needle word against the
        // stored element words. A scalar element is boxed on the way in, so the
        // needle is boxed the same way; equal inline scalars share one word and
        // match exactly.
        if matches!(op, crate::parser::BinOp::In | crate::parser::BinOp::NotIn)
            && matches!(&r_ty, Type::List(e) | Type::Set(e) if **e == Type::Any)
        {
            let l_ty = self.get_type(left.id).clone();
            let haystack = self.lower_expr_as_copy(right);
            let needle = self.lower_expr_as_copy(left);
            let needle = self.box_into_any(needle, &l_ty, span);
            let call_tmp = self.new_local(Type::Bool, None, false);
            self.push_statement(
                StatementKind::Assign(
                    call_tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__olive_in_list".to_string())),
                        args: vec![needle, haystack],
                    },
                ),
                span,
            );
            if matches!(op, crate::parser::BinOp::In) {
                return self.operand_for_local(call_tmp);
            }
            let not_tmp = self.new_local(Type::Bool, None, false);
            self.push_statement(
                StatementKind::Assign(
                    not_tmp,
                    Rvalue::UnaryOp(crate::parser::UnaryOp::Not, Operand::Copy(call_tmp)),
                ),
                span,
            );
            return self.operand_for_local(not_tmp);
        }

        if r_ty == Type::Str && matches!(op, crate::parser::BinOp::In | crate::parser::BinOp::NotIn)
        {
            let haystack = self.lower_expr_as_copy(right);
            let needle = self.lower_expr_as_copy(left);

            let call_tmp = self.new_local(Type::Bool, None, false);
            self.push_statement(
                StatementKind::Assign(
                    call_tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_str_contains".to_string(),
                        )),
                        args: vec![haystack, needle],
                    },
                ),
                span,
            );

            if matches!(op, crate::parser::BinOp::In) {
                return self.operand_for_local(call_tmp);
            } else {
                let not_tmp = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        not_tmp,
                        Rvalue::UnaryOp(crate::parser::UnaryOp::Not, Operand::Copy(call_tmp)),
                    ),
                    span,
                );
                return self.operand_for_local(not_tmp);
            }
        }

        if matches!(
            op,
            crate::parser::BinOp::And | crate::parser::BinOp::Or | crate::parser::BinOp::Coalesce
        ) {
            let tmp = self.new_local(self.get_type(expr_id), None, false);
            let l = self.lower_expr(left);

            if matches!(op, crate::parser::BinOp::Coalesce) {
                self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(l.clone())), span);
                // Null check: for `Any` use runtime check, otherwise compare against 0.
                let l_ty = self.get_type(left.id);
                let null_check = if matches!(l_ty, Type::Any) {
                    let is_null = self.new_local(Type::Bool, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            is_null,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_any_is_null".to_string(),
                                )),
                                args: vec![l],
                            },
                        ),
                        span,
                    );
                    is_null
                } else {
                    let raw = self.new_local(Type::Int, None, false);
                    self.push_statement(StatementKind::Assign(raw, Rvalue::Use(l)), span);
                    let is_zero = self.new_local(Type::Bool, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            is_zero,
                            Rvalue::BinaryOp(
                                crate::parser::BinOp::Eq,
                                Operand::Copy(raw),
                                Operand::Constant(Constant::Int(0)),
                            ),
                        ),
                        span,
                    );
                    is_zero
                };

                let rhs_bb = self.new_block();
                let merge_bb = self.new_block();

                if let Some(bb) = self.current_block {
                    self.terminate_block(
                        bb,
                        TerminatorKind::SwitchInt {
                            discr: Operand::Copy(null_check),
                            targets: vec![(1, rhs_bb)],
                            otherwise: merge_bb,
                        },
                        span,
                    );
                }

                self.current_block = Some(rhs_bb);
                let r = self.lower_expr(right);
                self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(r)), span);
                if let Some(bb) = self.current_block {
                    self.terminate_block(bb, TerminatorKind::Goto { target: merge_bb }, span);
                }

                self.current_block = Some(merge_bb);
                return self.operand_for_local(tmp);
            }
            let tmp = self.new_local(self.get_type(expr_id), None, false);
            let l = self.lower_expr(left);
            // Result is the original left value, but branch on its truthiness so
            // a boxed `Any` is tested by value, not by its pointer word.
            let l_ty = self.get_type(left.id);
            self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(l.clone())), span);
            let l_disc = self.truthify(l, &l_ty, span);

            let rhs_bb = self.new_block();
            let merge_bb = self.new_block();

            if let Some(bb) = self.current_block {
                if matches!(op, crate::parser::BinOp::And) {
                    self.terminate_block(
                        bb,
                        TerminatorKind::SwitchInt {
                            discr: l_disc,
                            targets: vec![(1, rhs_bb)],
                            otherwise: merge_bb,
                        },
                        span,
                    );
                } else {
                    self.terminate_block(
                        bb,
                        TerminatorKind::SwitchInt {
                            discr: l_disc,
                            targets: vec![(0, rhs_bb)],
                            otherwise: merge_bb,
                        },
                        span,
                    );
                }
            }

            self.current_block = Some(rhs_bb);
            let r = self.lower_expr(right);
            self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(r)), span);
            if let Some(bb) = self.current_block {
                self.terminate_block(bb, TerminatorKind::Goto { target: merge_bb }, span);
            }

            self.current_block = Some(merge_bb);
            return self.operand_for_local(tmp);
        }

        // Set algebra: | & - ^ on Set types dispatch to runtime functions.
        let l_ty = self.get_type(left.id);
        if matches!(l_ty, Type::Set(_)) && matches!(&r_ty, Type::Set(_)) {
            let fn_name = match op {
                crate::parser::BinOp::BitOr => "__olive_set_union",
                crate::parser::BinOp::BitAnd => "__olive_set_intersection",
                crate::parser::BinOp::Sub => "__olive_set_diff",
                crate::parser::BinOp::BitXor => "__olive_set_sym_diff",
                _ => return self.lower_expr(left),
            };
            let l = self.lower_expr_as_copy(left);
            let r = self.lower_expr_as_copy(right);
            let tmp = self.new_local(self.get_type(expr_id), None, false);
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(fn_name.to_string())),
                        args: vec![l, r],
                    },
                ),
                span,
            );
            return self.operand_for_local(tmp);
        }

        let l = if matches!(
            op,
            crate::parser::BinOp::Eq
                | crate::parser::BinOp::NotEq
                | crate::parser::BinOp::Lt
                | crate::parser::BinOp::LtEq
                | crate::parser::BinOp::Gt
                | crate::parser::BinOp::GtEq
                | crate::parser::BinOp::In
                | crate::parser::BinOp::NotIn
        ) {
            self.lower_expr_as_copy(left)
        } else {
            self.lower_expr(left)
        };
        let r = if matches!(
            op,
            crate::parser::BinOp::Eq
                | crate::parser::BinOp::NotEq
                | crate::parser::BinOp::Lt
                | crate::parser::BinOp::LtEq
                | crate::parser::BinOp::Gt
                | crate::parser::BinOp::GtEq
                | crate::parser::BinOp::In
                | crate::parser::BinOp::NotIn
        ) {
            self.lower_expr_as_copy(right)
        } else {
            self.lower_expr(right)
        };
        // The runtime `Any` operators dispatch on a self-describing value, so a
        // concrete operand paired with an `Any` one is boxed first. Without this
        // a raw scalar word reaches the dispatch and a large odd int is
        // misread as a tagged string pointer.
        use crate::parser::BinOp;
        let any_dispatch = matches!(
            op,
            BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Mod
                | BinOp::Lt
                | BinOp::LtEq
                | BinOp::Gt
                | BinOp::GtEq
                | BinOp::Eq
                | BinOp::NotEq
        );
        let l_ty = self.get_type(left.id).clone();
        let (l, r) = if any_dispatch {
            let l = if r_ty == Type::Any && l_ty != Type::Any {
                self.box_into_any(l, &l_ty, span)
            } else {
                l
            };
            let r = if l_ty == Type::Any && r_ty != Type::Any {
                self.box_into_any(r, &r_ty, span)
            } else {
                r
            };
            (l, r)
        } else {
            (l, r)
        };
        // Deep-copy elements; plain concat shares pointers and double-frees on drop.
        let mut deref_l = &l_ty;
        while let Type::Ref(inner) | Type::MutRef(inner) = deref_l {
            deref_l = inner;
        }
        let mut deref_r = &r_ty;
        while let Type::Ref(inner) | Type::MutRef(inner) = deref_r {
            deref_r = inner;
        }
        // Repeat: str*int / [T]*int, either operand order. Heap-owning
        // elements deep-copy per repetition (rule 3), scalars copy raw.
        if matches!(op, BinOp::Mul) {
            let seq = if matches!(deref_l, Type::Str | Type::List(_)) {
                Some((l.clone(), deref_l.clone(), r.clone()))
            } else if matches!(deref_r, Type::Str | Type::List(_)) {
                Some((r.clone(), deref_r.clone(), l.clone()))
            } else {
                None
            };
            if let Some((seq_op, seq_ty, count_op)) = seq {
                let runtime = match &seq_ty {
                    Type::Str => "__olive_str_repeat",
                    Type::List(elem) if Self::list_elem_needs_copy(elem) => {
                        "__olive_list_repeat_typed"
                    }
                    Type::List(_) => "__olive_list_repeat",
                    _ => unreachable!(),
                };
                let tmp = self.new_local(self.get_type(expr_id), None, false);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(runtime.to_string())),
                            args: vec![seq_op, count_op],
                        },
                    ),
                    span,
                );
                return self.operand_for_local(tmp);
            }
        }
        if matches!(op, BinOp::Add)
            && let Type::List(elem) = deref_l
            && Self::list_elem_needs_copy(elem)
        {
            let tmp = self.new_local(self.get_type(expr_id), None, false);
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_list_concat_typed".into(),
                        )),
                        args: vec![l, r],
                    },
                ),
                span,
            );
            return self.operand_for_local(tmp);
        }
        if matches!(
            op,
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq
        ) {
            self.last_cmp_operands = Some((l.clone(), r.clone()));
        }
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::BinaryOp(op.clone(), l, r)),
            span,
        );
        self.operand_for_local(tmp)
    }

    /// Unwraps `&`/`&mut` and returns the struct name and type args
    /// underneath, or `None` for anything else. Used to detect a struct
    /// operand in operator-dunder dispatch (E6.1).
    pub(super) fn deref_struct_ty(ty: Type) -> Option<(String, Vec<Type>)> {
        let mut t = ty;
        while let Type::Ref(inner) | Type::MutRef(inner) = t {
            t = *inner;
        }
        match t {
            Type::Struct(name, type_args, _) => Some((name, type_args)),
            _ => None,
        }
    }

    /// Builds an ordinary call to a struct's operator dunder, monomorphizing
    /// the mangled name first if the struct is generic (E6.1/E6.2).
    pub(super) fn call_struct_dunder(
        &mut self,
        struct_name: &str,
        type_args: &[Type],
        dunder: &str,
        args: Vec<Operand>,
        ret_ty: Type,
        span: Span,
    ) -> Operand {
        let base = format!("{struct_name}::{dunder}");
        let method_name = if type_args.is_empty() {
            base
        } else {
            self.monomorphize(&base, type_args)
        };
        let tmp = self.new_local(ret_ty, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(method_name)),
                    args,
                },
            ),
            span,
        );
        self.operand_for_local(tmp)
    }

    fn negate_bool(&mut self, val: Operand, span: Span) -> Operand {
        let tmp = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::UnaryOp(crate::parser::UnaryOp::Not, val)),
            span,
        );
        self.operand_for_local(tmp)
    }

    /// Whether a dict key / set element needs structural hash+eq (the same
    /// rule the checker's derived `==` uses) instead of the fast raw-word
    /// path `classify_key` already handles for scalars/strings.
    pub(crate) fn type_needs_structural_key(ty: &Type) -> bool {
        matches!(
            ty,
            Type::Struct(..)
                | Type::Enum(..)
                | Type::Tuple(_)
                | Type::List(_)
                | Type::Set(_)
                | Type::Dict(_, _)
        )
    }

    /// Whether list elements own heap data (double-frees if shared across lists).
    pub(crate) fn list_elem_needs_copy(elem: &Type) -> bool {
        !matches!(
            elem,
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
        )
    }

    pub(super) fn lower_unary_op_expr(
        &mut self,
        op: &crate::parser::UnaryOp,
        operand: &Expr,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let o = self.lower_expr(operand);
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::UnaryOp(op.clone(), o)),
            span,
        );
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_cast_expr(
        &mut self,
        operand: &Expr,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let op = self.lower_expr(operand);
        let tmp = self.new_local(self.get_type(expr_id), None, false);

        let target_ty = self.get_type(expr_id);
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::Cast(op, target_ty)),
            span,
        );
        self.operand_for_local(tmp)
    }
}
