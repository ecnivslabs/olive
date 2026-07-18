use super::super::MirBuilder;
use crate::mir::ir::*;
use crate::parser::{BinOp, Expr, ExprKind};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    /// Shared machinery for a starred destructure (`a, *rest = xs` / `let
    /// a, *rest = xs`, E4.4): proves the source has at least `slot_count -
    /// 1` elements (faulting E0710 otherwise, since the exact count is
    /// never known statically for a runtime-length list), and hands back
    /// the pieces each slot's read is built from.
    pub(super) fn lower_starred_destructure_source(
        &mut self,
        value: &Expr,
        rval: Operand,
        slot_count: usize,
        span: Span,
    ) -> (Local, Local, Type) {
        let rhs_local = self.new_tmp_for_expr(value);
        self.push_statement(
            StatementKind::Assign(rhs_local, Rvalue::Use(rval)),
            value.span,
        );
        let rhs_ty = self.current_locals[rhs_local.0].ty.clone();
        let elem_ty = match &rhs_ty {
            Type::List(inner) => (**inner).clone(),
            _ => Type::Any,
        };
        let min_len = (slot_count - 1) as i64;
        let len_local = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                len_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(
                        "__olive_check_list_min_len".to_string(),
                    )),
                    args: vec![
                        Operand::Copy(rhs_local),
                        Operand::Constant(Constant::Int(min_len)),
                        self.index_loc_operand(span),
                    ],
                },
            ),
            span,
        );
        (rhs_local, len_local, elem_ty)
    }

    /// Reads slot `i` of a `slot_count`-slot starred destructure: an
    /// indexed read before the star, a slice at the star (from `i` to
    /// `len - trailing`), an indexed read counted back from `len` after
    /// it. Every index here is proven in-bounds by the length check in
    /// [`lower_starred_destructure_source`], so reads skip the redundant
    /// runtime bounds check.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn starred_slot_operand(
        &mut self,
        rhs_local: Local,
        len_local: Local,
        elem_ty: &Type,
        i: usize,
        starred_idx: usize,
        slot_count: usize,
        span: Span,
    ) -> Operand {
        if i < starred_idx {
            let idx_op = Operand::Constant(Constant::Int(i as i64));
            let tmp = self.new_local_with_owning(elem_ty.clone(), None, false, false);
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::GetIndex(Operand::Copy(rhs_local), idx_op, true),
                ),
                span,
            );
            return self.operand_for_local(tmp);
        }
        if i > starred_idx {
            let back_offset = (slot_count - i) as i64;
            let idx_local = self.new_local(Type::Int, None, false);
            self.push_statement(
                StatementKind::Assign(
                    idx_local,
                    Rvalue::BinaryOp(
                        BinOp::Sub,
                        Operand::Copy(len_local),
                        Operand::Constant(Constant::Int(back_offset)),
                    ),
                ),
                span,
            );
            let tmp = self.new_local_with_owning(elem_ty.clone(), None, false, false);
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::GetIndex(Operand::Copy(rhs_local), Operand::Copy(idx_local), true),
                ),
                span,
            );
            return self.operand_for_local(tmp);
        }
        let trailing = (slot_count - starred_idx - 1) as i64;
        let stop_local = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                stop_local,
                Rvalue::BinaryOp(
                    BinOp::Sub,
                    Operand::Copy(len_local),
                    Operand::Constant(Constant::Int(trailing)),
                ),
            ),
            span,
        );
        let func_name = if Self::list_elem_needs_copy(elem_ty) {
            "__olive_list_getslice_typed"
        } else {
            "__olive_list_getslice"
        };
        const SLICE_HAS_START: i64 = 1;
        const SLICE_HAS_STOP: i64 = 2;
        let list_ty = Type::List(Box::new(elem_ty.clone()));
        let tmp = self.new_local_with_owning(list_ty, None, true, true);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(func_name.to_string())),
                    args: vec![
                        Operand::Copy(rhs_local),
                        Operand::Constant(Constant::Int(starred_idx as i64)),
                        Operand::Copy(stop_local),
                        Operand::Constant(Constant::Int(0)),
                        Operand::Constant(Constant::Int(SLICE_HAS_START | SLICE_HAS_STOP)),
                    ],
                },
            ),
            span,
        );
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_assign(
        &mut self,
        target: &crate::parser::Expr,
        value: &crate::parser::Expr,
    ) {
        // A starred target has no single meaningful type of its own (the
        // checker never records one for it), so it must be handled before
        // the generic `target_ty`/`coerce` path below, which assumes every
        // target has one.
        if let ExprKind::Tuple(elems) = &target.kind
            && let Some(starred_idx) = elems
                .iter()
                .position(|e| matches!(e.kind, ExprKind::Starred(_)))
        {
            let rval = self.lower_expr(value);
            let (rhs_local, len_local, elem_ty) =
                self.lower_starred_destructure_source(value, rval, elems.len(), target.span);
            for (i, elem) in elems.iter().enumerate() {
                let op = self.starred_slot_operand(
                    rhs_local,
                    len_local,
                    &elem_ty,
                    i,
                    starred_idx,
                    elems.len(),
                    elem.span,
                );
                let name_expr = if i == starred_idx {
                    let ExprKind::Starred(inner) = &elem.kind else {
                        unreachable!()
                    };
                    inner.as_ref()
                } else {
                    elem
                };
                if let ExprKind::Identifier(name) = &name_expr.kind {
                    if let Some(local) = self.lookup_var(name) {
                        self.push_statement(
                            StatementKind::Assign(local, Rvalue::Use(op)),
                            elem.span,
                        );
                    } else if let Some(Operand::Constant(Constant::GlobalData(_))) =
                        self.globals.get(name)
                    {
                        let ty = self.get_type(name_expr.id).clone();
                        self.store_module_global(name, op, ty, true, elem.span);
                    }
                }
            }
            return;
        }

        let target_ty = self.get_type(target.id).clone();
        let (mut rval, value_ty) = match self.lower_py_scalar_hint(value, &target_ty) {
            Some(op) => (op, target_ty.clone()),
            None => (self.lower_expr(value), self.get_type(value.id).clone()),
        };
        rval = self.coerce(rval, &value_ty, &target_ty, value.span);
        match &target.kind {
            ExprKind::Identifier(name) => {
                if let Some(local) = self.lookup_var(name) {
                    self.push_statement(
                        StatementKind::Assign(local, Rvalue::Use(rval)),
                        target.span,
                    );
                } else if let Some(Operand::Constant(Constant::GlobalData(_))) =
                    self.globals.get(name)
                {
                    // Module-scope reassignment of a `let mut` global; writes
                    // from inside a function are already rejected earlier (E0434).
                    let global_op = self.globals[name].clone();
                    let tmp = self.new_local(target_ty, None, false);
                    self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(rval)), target.span);
                    self.push_statement(
                        StatementKind::PtrStore(global_op, Operand::Copy(tmp)),
                        target.span,
                    );
                }
            }
            ExprKind::Attr { obj, attr } => {
                let obj_ty = self.get_type(obj.id).clone();
                let obj_op = self.lower_expr_as_copy(obj);
                if obj_ty.is_py_value() {
                    // `rval` was already coerced to `target_ty` above; key the
                    // to-Python conversion off that so a float isn't wrapped twice.
                    let py_rval = self.emit_to_py_arg(rval, &target_ty, target.span);
                    let dummy = self.new_local(Type::Any, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            dummy,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_py_setattr".to_string(),
                                )),
                                args: vec![
                                    obj_op,
                                    Operand::Constant(Constant::Str(attr.clone())),
                                    py_rval,
                                ],
                            },
                        ),
                        target.span,
                    );
                } else {
                    self.push_statement(
                        StatementKind::SetAttr(obj_op, attr.clone(), rval),
                        target.span,
                    );
                }
            }
            ExprKind::Index { obj, index } => {
                let obj_ty = self.get_type(obj.id).clone();
                let obj_op = self.lower_expr_as_copy(obj);
                let idx_op = self.lower_expr(index);
                if obj_ty.is_py_value() {
                    let idx_ty = self.get_type(index.id).clone();
                    // `rval` was already coerced to `target_ty` above; key the
                    // to-Python conversion off that so a float isn't wrapped twice.
                    let py_rval = self.emit_to_py_arg(rval, &target_ty, target.span);
                    let func_name = if Self::is_int_ty(&idx_ty) {
                        "__olive_py_setitem_int"
                    } else {
                        "__olive_py_setitem"
                    };
                    let dummy = self.new_local(Type::Any, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            dummy,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(func_name.to_string())),
                                args: vec![obj_op, idx_op, py_rval],
                            },
                        ),
                        target.span,
                    );
                } else {
                    self.push_statement(
                        StatementKind::SetIndex(obj_op, idx_op, rval, false),
                        target.span,
                    );
                }
            }
            ExprKind::Deref(ptr_expr) => {
                let ptr_op = self.lower_expr(ptr_expr);
                self.push_statement(StatementKind::PtrStore(ptr_op, rval), target.span);
            }
            ExprKind::Tuple(elems) => {
                let rhs_local = self.new_tmp_for_expr(value);
                self.push_statement(
                    StatementKind::Assign(rhs_local, Rvalue::Use(rval)),
                    value.span,
                );
                for (i, elem) in elems.iter().enumerate() {
                    let idx_op = Operand::Constant(Constant::Int(i as i64));
                    let elem_tmp = self.new_tmp_for_expr_with_owning(elem, false);
                    self.push_statement(
                        StatementKind::Assign(
                            elem_tmp,
                            Rvalue::GetIndex(Operand::Copy(rhs_local), idx_op, false),
                        ),
                        elem.span,
                    );
                    if let ExprKind::Identifier(name) = &elem.kind {
                        if let Some(local) = self.lookup_var(name) {
                            self.push_statement(
                                StatementKind::Assign(local, Rvalue::Use(Operand::Copy(elem_tmp))),
                                elem.span,
                            );
                        } else if let Some(Operand::Constant(Constant::GlobalData(_))) =
                            self.globals.get(name)
                        {
                            let ty = self.get_type(elem.id).clone();
                            self.store_module_global(
                                name,
                                Operand::Copy(elem_tmp),
                                ty,
                                true,
                                elem.span,
                            );
                        }
                    }
                }
            }
            _ => {
                let tmp = self.new_tmp_for_expr(target);
                self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(rval)), target.span);
            }
        }
    }
}
