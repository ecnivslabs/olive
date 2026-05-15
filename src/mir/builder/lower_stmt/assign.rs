use super::super::MirBuilder;
use crate::mir::ir::*;
use crate::parser::ExprKind;
use crate::semantic::types::Type;

impl<'a> MirBuilder<'a> {
    pub(super) fn lower_assign(
        &mut self,
        target: &crate::parser::Expr,
        value: &crate::parser::Expr,
    ) {
        let mut rval = self.lower_expr(value);
        let target_ty = self.get_type(target.id).clone();
        let value_ty = self.get_type(value.id).clone();
        rval = self.coerce(rval, &value_ty, &target_ty, value.span);
        match &target.kind {
            ExprKind::Identifier(name) => {
                if let Some(local) = self.lookup_var(name) {
                    self.push_statement(
                        StatementKind::Assign(local, Rvalue::Use(rval)),
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
                    if let ExprKind::Identifier(name) = &elem.kind
                        && let Some(local) = self.lookup_var(name)
                    {
                        self.push_statement(
                            StatementKind::Assign(local, Rvalue::Use(Operand::Copy(elem_tmp))),
                            elem.span,
                        );
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
