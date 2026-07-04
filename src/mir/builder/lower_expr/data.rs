use super::super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{Expr, ExprKind};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    /// Builds a `file:line:col` string operand for a fault site so a runtime
    /// bounds or nil-index panic can point back at the source line.
    pub(super) fn index_loc_operand(&self, span: Span) -> Operand {
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
        // A non-capturing nested fn as a value is its lifted code pointer;
        // capturing ones are rejected as escapes by the resolver before here.
        if self.lookup_var(name).is_none()
            && let Some(info) = self.lookup_nested_fn(name)
            && self.resolve_captures(&info.raw_captures).is_empty()
        {
            return Operand::Constant(Constant::Function(info.mangled));
        }
        if let Some(local) = self.lookup_var(name) {
            let ty = self.current_locals[local.0].ty.clone();
            if matches!(ty, Type::PyObject) {
                Operand::Copy(local)
            } else {
                self.operand_for_local(local)
            }
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
            Operand::Constant(Constant::Function(name.to_string()))
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
                        let source = self.lower_expr(inner);
                        self.push_statement(
                            StatementKind::Assign(
                                void_dummy,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_list_extend".to_string(),
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
        let tmp = self.new_local_with_owning(ty, None, true, false);
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
