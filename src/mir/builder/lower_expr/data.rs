use super::super::{MirBuilder, NestedFnInfo};
use super::py_call::RET_HANDLE;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{Expr, ExprKind};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    /// Builds a `file:line:col` string operand for a fault site so a runtime
    /// bounds or nil-index panic can point back at the source line.
    pub(in crate::mir::builder) fn index_loc_operand(&self, span: Span) -> Operand {
        let loc = match self.file_names.get(&span.file_id) {
            Some(file) => format!("{}:{}:{}", file, span.line, span.col),
            None => format!("{}:{}", span.line, span.col),
        };
        Operand::Constant(Constant::Str(loc))
    }

    pub(super) fn lower_deref_expr(&mut self, inner: &Expr, expr_id: usize) -> Operand {
        let ptr_op = self.lower_expr(inner);
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::PtrLoad(ptr_op)),
            inner.span,
        );
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_borrow_expr(
        &mut self,
        inner: &Expr,
        expr_id: usize,
        span: Span,
    ) -> Operand {
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        let rval = if let ExprKind::Identifier(name) = &inner.kind {
            if let Some(local) = self.lookup_var(name) {
                Rvalue::Ref(local)
            } else {
                let op = self.lower_expr(inner);
                Rvalue::Use(op)
            }
        } else {
            let op = self.lower_expr(inner);
            Rvalue::Use(op)
        };
        self.push_statement(StatementKind::Assign(tmp, rval), span);
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_mut_borrow_expr(
        &mut self,
        inner: &Expr,
        expr_id: usize,
        span: Span,
    ) -> Operand {
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        let rval = if let ExprKind::Identifier(name) = &inner.kind {
            if let Some(local) = self.lookup_var(name) {
                Rvalue::MutRef(local)
            } else {
                let op = self.lower_expr(inner);
                Rvalue::Use(op)
            }
        } else {
            let op = self.lower_expr(inner);
            Rvalue::Use(op)
        };
        self.push_statement(StatementKind::Assign(tmp, rval), span);
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_identifier_expr(&mut self, name: &str, expr_id: usize) -> Operand {
        // A named nested fn read as a plain value (not the callee of a
        // direct call -- that path is `lower_nested_fn_call`) builds an
        // escaping closure record (E5.2), whether or not it captures
        // anything: every `Type::Fn` value is uniformly a record pointer, so
        // generic drop/copy/indirect-call codegen has exactly one shape to
        // handle instead of two.
        if self.lookup_var(name).is_none()
            && let Some(info) = self.lookup_nested_fn(name)
        {
            return self.build_closure_value(&info, expr_id, Span::default());
        }
        if let Some(local) = self.lookup_var(name) {
            let ty = self.current_locals[local.0].ty.clone();
            if matches!(ty, Type::PyObject) {
                return Operand::Copy(local);
            }
            // A flow-narrowed read: the checker resolved this occurrence to a
            // concrete member of the local's declared union. Codegen's binop
            // dispatch (`is_str_op` and kin) reads a Local's own declared
            // type, not the checked expression type, so retag the value via
            // a non-owning view local -- same bits, narrower static type, no
            // second owner to double-free. `Cast`, not `Use`: copy-prop
            // treats `Use` as a pure value alias and freely erases the view,
            // undoing the retag once anything downstream (e.g. inlining)
            // gives it a second reference to fold through.
            let narrowed = self.get_type(expr_id);
            if matches!(ty, Type::Union(_)) && narrowed != ty {
                let view = self.new_local_with_owning(narrowed.clone(), None, false, false);
                self.push_statement(
                    StatementKind::Assign(view, Rvalue::Cast(Operand::Copy(local), narrowed)),
                    Span::default(),
                );
                return Operand::Copy(view);
            }
            self.operand_for_local(local)
        } else if let Some(global_op) = self.globals.get(name).cloned() {
            if let Operand::Constant(Constant::GlobalData(_)) = &global_op {
                let ty = self.get_type(expr_id);
                // A raw read of the global's stored pointer, not a new
                // reference to it -- the global owns it permanently, so this
                // local must never be scope-dropped (would decref/free a
                // shared value, e.g. an imported Python module, on every use).
                let tmp = self.new_local_with_owning(ty, None, false, false);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::PtrLoad(global_op.clone())),
                    Span::default(),
                );
                Operand::Copy(tmp)
            } else {
                global_op
            }
        } else {
            // A plain top-level function read as a value (e.g. `apply(square,
            // 5)`): module-level fns never capture, but still build the
            // uniform record -- see the nested-fn case above.
            let info = NestedFnInfo {
                mangled: name.to_string(),
                raw_captures: Vec::new(),
                param_tys: Vec::new(),
            };
            self.build_closure_value(&info, expr_id, Span::default())
        }
    }

    pub(super) fn lower_list_or_tuple_expr(
        &mut self,
        elems: &[Expr],
        is_tuple: bool,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let has_splat = elems.iter().any(|e| {
            if let ExprKind::Deref(inner) = &e.kind {
                !matches!(self.get_type(inner.id), Type::Ptr(_) | Type::Int)
            } else {
                false
            }
        });
        let elem_box_ty = match self.get_type(expr_id) {
            Type::List(elem) => *elem,
            _ => Type::Any,
        };
        if has_splat {
            let zero = Operand::Constant(Constant::Int(0));
            let tmp = self.new_local(self.get_type(expr_id), None, false);
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__olive_list_new".to_string())),
                        args: vec![zero],
                    },
                ),
                span,
            );
            let void_dummy = self.new_local(Type::Null, None, false);
            for elem in elems {
                if let ExprKind::Deref(inner) = &elem.kind {
                    let inner_ty = self.get_type(inner.id);
                    if !matches!(inner_ty, Type::Ptr(_) | Type::Int) {
                        let mut src_ty = &inner_ty;
                        while let Type::Ref(i) | Type::MutRef(i) = src_ty {
                            src_ty = i;
                        }
                        let extend_fn = match src_ty {
                            Type::List(e) if Self::list_elem_needs_copy(e) => {
                                "__olive_list_extend_typed"
                            }
                            _ => "__olive_list_extend",
                        };
                        let source = self.lower_expr(inner);
                        self.push_statement(
                            StatementKind::Assign(
                                void_dummy,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        extend_fn.to_string(),
                                    )),
                                    args: vec![Operand::Copy(tmp), source],
                                },
                            ),
                            span,
                        );
                        continue;
                    }
                }
                let val = self.lower_expr(elem);
                let val = self.coerce_to_elem(val, elem, &elem_box_ty);
                self.push_statement(
                    StatementKind::Assign(
                        void_dummy,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_list_append".to_string(),
                            )),
                            args: vec![Operand::Copy(tmp), val],
                        },
                    ),
                    span,
                );
            }
            self.operand_for_local(tmp)
        } else {
            let ops: Vec<Operand> = elems
                .iter()
                .map(|e| {
                    let op = self.lower_expr(e);
                    if is_tuple {
                        op
                    } else {
                        self.coerce_to_elem(op, e, &elem_box_ty)
                    }
                })
                .collect();
            let tmp = self.new_local(self.get_type(expr_id), None, false);
            let kind = if is_tuple {
                AggregateKind::Tuple
            } else {
                AggregateKind::List
            };
            self.push_statement(
                StatementKind::Assign(tmp, Rvalue::Aggregate(kind, ops)),
                span,
            );
            self.operand_for_local(tmp)
        }
    }

    pub(super) fn lower_set_expr(&mut self, elems: &[Expr], span: Span, expr_id: usize) -> Operand {
        let elem_box_ty = match self.get_type(expr_id) {
            Type::Set(elem) => *elem,
            _ => Type::Any,
        };
        let ops: Vec<Operand> = elems
            .iter()
            .map(|e| {
                let op = self.lower_expr(e);
                self.coerce_to_hashable(op, e, &elem_box_ty)
            })
            .collect();
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::Aggregate(AggregateKind::Set, ops)),
            span,
        );
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_dict_expr(
        &mut self,
        pairs: &[(Expr, Expr)],
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let (key_box_ty, val_box_ty) = match self.get_type(expr_id) {
            Type::Dict(k, v) => (*k, *v),
            _ => (Type::Any, Type::Any),
        };
        let mut ops = Vec::new();
        for (k, v) in pairs {
            let kop = self.lower_expr(k);
            ops.push(self.coerce_to_hashable(kop, k, &key_box_ty));
            let vop = self.lower_expr(v);
            ops.push(self.coerce_to_elem(vop, v, &val_box_ty));
        }
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::Aggregate(AggregateKind::Dict, ops)),
            span,
        );
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_attr_expr(
        &mut self,
        obj: &Expr,
        attr: &str,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        if let ExprKind::Identifier(name) = &obj.kind {
            let obj_ty = self.get_type(obj.id);
            let mut current_obj_ty = obj_ty.clone();
            while let Type::Ref(inner) | Type::MutRef(inner) = current_obj_ty {
                current_obj_ty = *inner;
            }
            let is_struct_or_self = matches!(
                current_obj_ty,
                Type::Struct(_, _, _) | Type::Any | Type::Var(_)
            ) && self.lookup_var(name).is_some();
            if !is_struct_or_self && !obj_ty.is_py_value() && !current_obj_ty.is_py_value() {
                let mangled = format!("{}::{}", name, attr);
                if let Some(local) = self.lookup_var(&mangled) {
                    return Operand::Copy(local);
                }
                if let Some(global_op) = self.globals.get(&mangled) {
                    return global_op.clone();
                }
                // A namespaced function read as a value (e.g. an unbound
                // method or native-module fn passed as a callback) builds
                // the uniform closure record; any other namespaced symbol
                // reaching here (a module constant with no `globals` entry)
                // keeps the bare reference.
                if matches!(self.get_type(expr_id), Type::Fn(_, _, _)) {
                    let info = NestedFnInfo {
                        mangled,
                        raw_captures: Vec::new(),
                        param_tys: Vec::new(),
                    };
                    return self.build_closure_value(&info, expr_id, span);
                }
                return Operand::Constant(Constant::Function(mangled));
            }
        }

        let obj_ty = self.get_type(obj.id);
        if obj_ty.is_py_value() {
            let obj_op = self.lower_expr_as_copy(obj);
            let tmp = self.new_local(Type::PyObject, None, true);
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_py_getattr".to_string(),
                        )),
                        args: vec![obj_op, Operand::Constant(Constant::Str(attr.to_string()))],
                    },
                ),
                span,
            );
            return self.operand_for_local(tmp);
        }

        let o = self.lower_expr_as_copy(obj);
        let ty = self.get_type(expr_id);
        let tmp = self.new_local_with_owning(ty, None, true, false);
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::GetAttr(o, attr.to_string())),
            span,
        );
        self.operand_for_local(tmp)
    }

    /// GetAttr analogue of `lower_py_call_scalar_hint` (R10): `obj.attr` on
    /// a `PyObject` `obj` types as bare `PyObject` at the checker level (a
    /// dynamic module/object attribute has no static type of its own), so
    /// `lower_attr_expr` above always wraps a handle -- even when the
    /// surrounding context (a `let` annotation, an already-typed
    /// reassignment target, a declared return type) already knows the
    /// result must be a concrete scalar. Called from the same four
    /// statement sites `lower_py_call_scalar_hint` is (see
    /// `lower_py_scalar_hint`), fuses straight to `olive_py_getattr_ret`
    /// instead of `olive_py_getattr` plus a second boundary-crossing
    /// realize. `None` falls through to the plain `lower_expr` + `coerce`
    /// path, unchanged.
    pub(in crate::mir::builder) fn lower_py_getattr_scalar_hint(
        &mut self,
        expr: &Expr,
        hint: &Type,
    ) -> Option<Operand> {
        if self.get_type(expr.id) != Type::PyObject || Self::py_ret_tag(hint).0 == RET_HANDLE {
            return None;
        }
        let ExprKind::Attr { obj, attr } = &expr.kind else {
            return None;
        };
        if !self.get_type(obj.id).is_py_value() {
            return None;
        }

        let obj_op = self.lower_expr_as_copy(obj);
        let (ret_tag, local_ty) = Self::py_ret_tag(hint);
        let result = self.new_local(local_ty, None, true);
        self.push_statement(
            StatementKind::Assign(
                result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(
                        "__olive_py_getattr_ret".to_string(),
                    )),
                    args: vec![
                        obj_op,
                        Operand::Constant(Constant::Str(attr.clone())),
                        Operand::Constant(Constant::Int(ret_tag)),
                    ],
                },
            ),
            expr.span,
        );
        Some(self.operand_for_local(result))
    }

    /// `obj?.attr`: null-tests `obj`, skipping the field read entirely (no
    /// heap value touched) when it's `None`. Same block shape as `??`.
    /// Field reads never transfer ownership (plain `.attr` is a borrow, via
    /// `lower_attr_expr` above); the merged result stays non-owning too.
    pub(super) fn lower_opt_attr_expr(
        &mut self,
        obj: &Expr,
        attr: &str,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let result_ty = self.get_type(expr_id);
        let tmp = self.new_local_with_owning(result_ty.clone(), None, false, false);
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::Use(Operand::Constant(Constant::None))),
            span,
        );

        let obj_op = self.lower_expr(obj);
        let obj_ty = self.get_type(obj.id);
        let is_null = if matches!(obj_ty, Type::Any) {
            let is_null = self.new_local(Type::Bool, None, false);
            self.push_statement(
                StatementKind::Assign(
                    is_null,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_any_is_null".to_string(),
                        )),
                        args: vec![obj_op.clone()],
                    },
                ),
                span,
            );
            is_null
        } else {
            let raw = self.new_local(Type::Int, None, false);
            self.push_statement(
                StatementKind::Assign(raw, Rvalue::Use(obj_op.clone())),
                span,
            );
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

        let some_bb = self.new_block();
        let merge_bb = self.new_block();
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::SwitchInt {
                    discr: Operand::Copy(is_null),
                    targets: vec![(1, merge_bb)],
                    otherwise: some_bb,
                },
                span,
            );
        }

        self.current_block = Some(some_bb);
        // `obj` is provably non-null here; retag it to its narrowed member
        // type so `GetAttr` (and any codegen dispatch keyed off the operand
        // local's own declared type) sees the concrete type, not the union.
        let narrowed_ty = non_null_ty(&obj_ty);
        let struct_name = match &narrowed_ty {
            Type::Struct(name, ..) => Some(name.clone()),
            _ => None,
        };
        let narrowed_obj = if narrowed_ty != obj_ty {
            // `Cast`, not `Use`: see the identical retag in
            // `lower_identifier_expr` for why `Use` doesn't survive copy-prop.
            let view = self.new_local_with_owning(narrowed_ty.clone(), None, false, false);
            self.push_statement(
                StatementKind::Assign(view, Rvalue::Cast(obj_op, narrowed_ty)),
                span,
            );
            Operand::Copy(view)
        } else {
            obj_op
        };
        // The field's own declared type, not the `?.` expression's overall
        // (possibly `None`-wrapped) result type -- a field that is itself
        // already nullable (`inner: Inner | None`) must keep its own Null,
        // not have it stripped by the receiver's narrowing.
        let field_ty = struct_name
            .and_then(|sn| {
                self.struct_field_types
                    .get(&(sn, attr.to_string()))
                    .cloned()
            })
            .unwrap_or_else(|| non_null_ty(&result_ty));
        let field_tmp = self.new_local_with_owning(field_ty, None, true, false);
        self.push_statement(
            StatementKind::Assign(field_tmp, Rvalue::GetAttr(narrowed_obj, attr.to_string())),
            span,
        );
        self.push_statement(
            StatementKind::Assign(tmp, Rvalue::Use(Operand::Copy(field_tmp))),
            span,
        );
        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: merge_bb }, span);
        }

        self.current_block = Some(merge_bb);
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_index_expr(
        &mut self,
        obj: &Expr,
        index: &Expr,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let obj_ty = self.get_type(obj.id);
        let mut current_obj_ty = obj_ty;
        while let Type::Ref(inner) | Type::MutRef(inner) = current_obj_ty {
            current_obj_ty = *inner;
        }

        // `f[int]` is an explicit type argument on a function, not a real index;
        // it lowers to the function itself, with inference picking the type.
        if matches!(current_obj_ty, Type::Fn(_, _, _)) {
            return self.lower_expr(obj);
        }

        if let ExprKind::Slice { start, stop, step } = &index.kind {
            let func_name = match &current_obj_ty {
                Type::PyObject | Type::Any => "__olive_py_getslice",
                Type::Str => "__olive_str_getslice",
                Type::List(e) if Self::list_elem_needs_copy(e) => "__olive_list_getslice_typed",
                Type::List(_) | Type::Tuple(_) | Type::Set(_) => "__olive_list_getslice",
                _ => "__olive_list_getslice",
            };
            return self.lower_slice(
                obj,
                start.as_deref(),
                stop.as_deref(),
                step.as_deref(),
                func_name,
                span,
                expr_id,
            );
        }

        if current_obj_ty == Type::Str {
            let o = self.lower_expr_as_copy(obj);
            let i = self.lower_expr(index);
            let loc = self.index_loc_operand(span);
            let tmp = self.new_local(Type::Str, None, false);
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_str_get_checked".to_string(),
                        )),
                        args: vec![o, i, loc],
                    },
                ),
                span,
            );
            return self.operand_for_local(tmp);
        }
        let o = self.lower_expr_as_copy(obj);
        let i_raw = self.lower_expr(index);
        let ty = self.get_type(expr_id);
        // A py subscript returns a fresh owned handle; a container read is a view.
        let owning = current_obj_ty.is_py_value();
        let tmp = self.new_local_with_owning(ty, None, true, owning);
        if current_obj_ty.is_py_value() {
            let idx_ty = self.get_type(index.id).clone();
            let func_name = if Self::is_int_ty(&idx_ty) {
                "__olive_py_getitem_int"
            } else {
                "__olive_py_getitem"
            };
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(func_name.to_string())),
                        args: vec![o, i_raw],
                    },
                ),
                span,
            );
        } else {
            self.push_statement(
                StatementKind::Assign(tmp, Rvalue::GetIndex(o, i_raw, false)),
                span,
            );
        }
        self.operand_for_local(tmp)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_slice(
        &mut self,
        obj: &Expr,
        start: Option<&Expr>,
        stop: Option<&Expr>,
        step: Option<&Expr>,
        func_name: &str,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        const SLICE_HAS_START: i64 = 1;
        const SLICE_HAS_STOP: i64 = 2;
        const SLICE_HAS_STEP: i64 = 4;

        let o = self.lower_expr_as_copy(obj);
        let mut flags: i64 = 0;
        let start_op = if let Some(e) = start {
            flags |= SLICE_HAS_START;
            self.lower_expr(e)
        } else {
            Operand::Constant(Constant::Int(0))
        };
        let stop_op = if let Some(e) = stop {
            flags |= SLICE_HAS_STOP;
            self.lower_expr(e)
        } else {
            Operand::Constant(Constant::Int(0))
        };
        let step_op = if let Some(e) = step {
            flags |= SLICE_HAS_STEP;
            self.lower_expr(e)
        } else {
            Operand::Constant(Constant::Int(0))
        };
        let flags_op = Operand::Constant(Constant::Int(flags));
        let ty = self.get_type(expr_id);
        let tmp = self.new_local_with_owning(ty, None, true, true);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(func_name.to_string())),
                    args: vec![o, start_op, stop_op, step_op, flags_op],
                },
            ),
            span,
        );
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_list_comp_expr(
        &mut self,
        elt: &Expr,
        clauses: &[crate::parser::CompClause],
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let ty = self.get_type(expr_id);
        self.lower_comprehension(None, Some(elt), clauses, AggregateKind::List, span, ty)
    }

    pub(super) fn lower_set_comp_expr(
        &mut self,
        elt: &Expr,
        clauses: &[crate::parser::CompClause],
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let ty = self.get_type(expr_id);
        self.lower_comprehension(None, Some(elt), clauses, AggregateKind::Set, span, ty)
    }

    pub(super) fn lower_dict_comp_expr(
        &mut self,
        key: &Expr,
        value: &Expr,
        clauses: &[crate::parser::CompClause],
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let ty = self.get_type(expr_id);
        self.lower_comprehension(
            Some((key, value)),
            None,
            clauses,
            AggregateKind::Dict,
            span,
            ty,
        )
    }
}

/// `T | None` with `Null` removed, collapsing to the sole remaining member.
/// Mirrors `TypeChecker::non_null_member`; `ty` itself when there is nothing
/// to narrow (already checked to be a real member of the union).
fn non_null_ty(ty: &Type) -> Type {
    match ty {
        Type::Union(members) => {
            let filtered: Vec<Type> = members
                .iter()
                .filter(|m| **m != Type::Null)
                .cloned()
                .collect();
            match filtered.len() {
                1 => filtered.into_iter().next().unwrap(),
                0 => ty.clone(),
                _ => Type::Union(filtered),
            }
        }
        other => other.clone(),
    }
}
