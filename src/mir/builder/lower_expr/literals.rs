use super::super::MirBuilder;
use crate::mir::ir::*;
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    pub(super) fn lower_fstr_expr(
        &mut self,
        parts: &[crate::parser::FStrPart],
        span: Span,
    ) -> Operand {
        let mut current_res: Option<Operand> = None;

        for part in parts {
            let e = &part.expr;
            let op = self.lower_expr_as_copy(e);
            let ty = self.get_type(e.id);

            let str_op = if let Some(spec) = &part.spec {
                self.lower_fstr_format(op, &ty, spec, e.span)
            } else if ty == Type::Str {
                op
            } else if let Some(str_op) = self.lower_struct_str_call(op.clone(), &ty, e.span) {
                // E6.2: a struct defining `__str__` uses it in an
                // interpolation the same way `print`/`str()` do.
                str_op
            } else {
                // `None` is a bare `0` at runtime, so a plain `str` call would
                // dispatch to the integer path; name the null formatter directly.
                let str_fn = if ty == Type::Null {
                    "__olive_none_to_str"
                } else {
                    "str"
                };
                let tmp = self.new_local(Type::Str, None, true);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(str_fn.to_string())),
                            args: vec![op],
                        },
                    ),
                    e.span,
                );
                self.operand_for_local(tmp)
            };

            if let Some(res) = current_res {
                let tmp = self.new_local(Type::Str, None, true);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::BinaryOp(crate::parser::BinOp::Add, res, str_op),
                    ),
                    span,
                );
                current_res = Some(Operand::Copy(tmp));
            } else {
                current_res = Some(str_op);
            }
        }

        current_res.unwrap_or(Operand::Constant(Constant::Str(String::new())))
    }

    /// Lowers an interpolated value carrying a Python format spec into a call to
    /// the type-appropriate runtime formatter.
    fn lower_fstr_format(&mut self, val: Operand, ty: &Type, spec: &str, span: Span) -> Operand {
        let func = match ty {
            Type::Float | Type::F32 => "__olive_format_float",
            Type::Bool => "__olive_format_bool",
            Type::Str => "__olive_format_str",
            Type::Any | Type::Union(_) | Type::PyObject | Type::PyNamed(_, _) => {
                "__olive_format_any"
            }
            _ => "__olive_format_int",
        };
        let spec_op = Operand::Constant(Constant::Str(spec.to_string()));
        let tmp = self.new_local(Type::Str, None, true);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(func.to_string())),
                    args: vec![val, spec_op],
                },
            ),
            span,
        );
        self.operand_for_local(tmp)
    }
}
