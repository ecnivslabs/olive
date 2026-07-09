use super::super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{CallArg, Expr};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    pub(super) fn lower_call_args(
        &mut self,
        args: &[CallArg],
        _callee: &Expr,
        _span: Span,
    ) -> (Vec<Operand>, Vec<Option<String>>, Vec<Type>) {
        let mut arg_ops = Vec::new();
        let mut arg_kw_names: Vec<Option<String>> = Vec::new();
        let mut arg_tys: Vec<Type> = Vec::new();
        for arg in args {
            match arg {
                CallArg::Splat(e) if self.get_type(e.id) == crate::semantic::types::Type::Int => {
                    let ptr_op = self.lower_expr(e);
                    let tmp = self.new_local(crate::semantic::types::Type::Int, None, false);
                    self.push_statement(
                        StatementKind::Assign(tmp, Rvalue::PtrLoad(ptr_op)),
                        e.span,
                    );
                    arg_ops.push(Operand::Copy(tmp));
                    arg_kw_names.push(None);
                    arg_tys.push(crate::semantic::types::Type::Int);
                }
                CallArg::Positional(e) | CallArg::Splat(e) | CallArg::KwSplat(e) => {
                    let arg_ty = self.get_type(e.id);
                    // Arguments are borrows: the caller keeps ownership and
                    // frees the value when its own scope ends.
                    arg_ops.push(self.lower_expr_as_copy(e));
                    arg_kw_names.push(None);
                    arg_tys.push(arg_ty);
                }
                CallArg::Keyword(name, e) => {
                    let arg_ty = self.get_type(e.id);
                    arg_ops.push(self.lower_expr_as_copy(e));
                    arg_kw_names.push(Some(name.clone()));
                    arg_tys.push(arg_ty);
                }
            }
        }
        (arg_ops, arg_kw_names, arg_tys)
    }

    pub(super) fn lower_pyobject_call(
        &mut self,
        callee_op: Operand,
        args: &[CallArg],
        arg_ops: Vec<Operand>,
        arg_kw_names: Vec<Option<String>>,
        span: Span,
    ) -> Operand {
        let mut pos_ops = Vec::new();
        let mut kw_ops = Vec::new();
        for (i, (op, kw_name)) in arg_ops.into_iter().zip(arg_kw_names).enumerate() {
            let arg_ty = args
                .get(i)
                .map(|a| match a {
                    CallArg::Positional(e)
                    | CallArg::Splat(e)
                    | CallArg::KwSplat(e)
                    | CallArg::Keyword(_, e) => self.get_type(e.id),
                })
                .unwrap_or(Type::Any);
            if let Some(name) = kw_name {
                let py_op = self.emit_to_py_arg(op, &arg_ty, span);
                kw_ops.push(Operand::Constant(Constant::Str(name)));
                kw_ops.push(py_op);
            } else {
                pos_ops.push(self.emit_to_py_arg(op, &arg_ty, span));
            }
        }

        let args_list = self.new_local(Type::List(Box::new(Type::Any)), None, true);
        self.push_statement(
            StatementKind::Assign(args_list, Rvalue::Aggregate(AggregateKind::List, pos_ops)),
            span,
        );

        if kw_ops.is_empty() {
            let result = self.new_local(Type::PyObject, None, true);
            self.push_statement(
                StatementKind::Assign(
                    result,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__olive_py_call".to_string())),
                        args: vec![callee_op, Operand::Copy(args_list)],
                    },
                ),
                span,
            );
            self.operand_for_local(result)
        } else {
            let kwargs_list = self.new_local(Type::List(Box::new(Type::Any)), None, true);
            self.push_statement(
                StatementKind::Assign(kwargs_list, Rvalue::Aggregate(AggregateKind::List, kw_ops)),
                span,
            );
            let result = self.new_local(Type::PyObject, None, true);
            self.push_statement(
                StatementKind::Assign(
                    result,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_py_call_kw".to_string(),
                        )),
                        args: vec![
                            callee_op,
                            Operand::Copy(args_list),
                            Operand::Copy(kwargs_list),
                        ],
                    },
                ),
                span,
            );
            self.operand_for_local(result)
        }
    }

    pub(super) fn lower_type_builtin(&mut self, args: &[CallArg], span: Span) -> Option<Operand> {
        if args.is_empty() {
            return None;
        }
        let arg_expr = match &args[0] {
            CallArg::Positional(e)
            | CallArg::Keyword(_, e)
            | CallArg::Splat(e)
            | CallArg::KwSplat(e) => e,
        };
        let mut arg_ty = self.get_type(arg_expr.id);
        while let Type::Ref(inner) | Type::MutRef(inner) = arg_ty {
            arg_ty = *inner;
        }

        // A concrete static type gives a constant name; `Any`/Python is read at
        // runtime.
        let name = match arg_ty {
            Type::Int
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize
            | Type::IntegerLiteral(_) => "int",
            Type::Float | Type::F32 | Type::FloatLiteral(_) => "float",
            Type::Bool => "bool",
            Type::Str => "str",
            Type::Bytes => "bytes",
            Type::List(_) | Type::Tuple(_) => "list",
            Type::Dict(_, _) => "dict",
            Type::Set(_) => "set",
            Type::Null => "None",
            Type::Enum(_, _) => "enum",
            Type::Any | Type::PyObject | Type::PyNamed(_, _) | Type::Var(_) => {
                let arg = self.lower_expr(arg_expr);
                let result = self.new_local(Type::Str, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        result,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_typeof_str".to_string(),
                            )),
                            args: vec![arg],
                        },
                    ),
                    span,
                );
                return Some(self.operand_for_local(result));
            }
            other => return Some(Operand::Constant(Constant::Str(format!("{other}")))),
        };
        Some(Operand::Constant(Constant::Str(name.to_string())))
    }

    pub(super) fn lower_len_builtin(&mut self, args: &[CallArg], span: Span) -> Option<Operand> {
        if args.is_empty() {
            return None;
        }
        let arg_expr = match &args[0] {
            CallArg::Positional(e)
            | CallArg::Keyword(_, e)
            | CallArg::Splat(e)
            | CallArg::KwSplat(e) => e,
        };
        let arg_ty = self.get_type(arg_expr.id);
        let mut current_arg_ty = arg_ty;
        while let Type::Ref(inner) | Type::MutRef(inner) = current_arg_ty {
            current_arg_ty = *inner;
        }

        let func_name = if current_arg_ty == Type::Str {
            "__olive_str_len"
        } else if matches!(current_arg_ty, Type::Dict(_, _)) {
            // A dict is an object (key map), not a contiguous vector, so it has
            // its own length function.
            "__olive_obj_len"
        } else if matches!(
            current_arg_ty,
            Type::List(_) | Type::Tuple(_) | Type::Set(_) | Type::Any
        ) {
            "__olive_list_len"
        } else if current_arg_ty == Type::Bytes {
            "__olive_buf_len"
        } else if current_arg_ty.is_py_value() {
            "__olive_py_len"
        } else {
            return None;
        };

        let arg_op = self.lower_expr_as_copy(arg_expr);
        let tmp = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(func_name.to_string())),
                    args: vec![arg_op],
                },
            ),
            span,
        );
        Some(self.operand_for_local(tmp))
    }

    pub(super) fn lower_maxmin_builtin(
        &mut self,
        name: &str,
        args: &[CallArg],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        if args.len() != 2 {
            return None;
        }
        let a_expr = match &args[0] {
            CallArg::Positional(e)
            | CallArg::Keyword(_, e)
            | CallArg::Splat(e)
            | CallArg::KwSplat(e) => e,
        };
        let b_expr = match &args[1] {
            CallArg::Positional(e)
            | CallArg::Keyword(_, e)
            | CallArg::Splat(e)
            | CallArg::KwSplat(e) => e,
        };

        let a_op = self.lower_expr_as_copy(a_expr);
        let b_op = self.lower_expr_as_copy(b_expr);
        let result_ty = self.get_type(expr_id);

        let cmp_op = if name == "max" {
            crate::parser::BinOp::Gt
        } else {
            crate::parser::BinOp::Lt
        };
        let cond_local = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(
                cond_local,
                Rvalue::BinaryOp(cmp_op, a_op.clone(), b_op.clone()),
            ),
            span,
        );

        let result_local = self.new_local(result_ty.clone(), None, false);
        let true_bb = self.new_block();
        let false_bb = self.new_block();
        let exit_bb = self.new_block();

        if let Some(cur) = self.current_block {
            self.terminate_block(
                cur,
                TerminatorKind::SwitchInt {
                    discr: Operand::Copy(cond_local),
                    targets: vec![(1, true_bb)],
                    otherwise: false_bb,
                },
                span,
            );
        }

        self.current_block = Some(true_bb);
        self.push_statement(StatementKind::Assign(result_local, Rvalue::Use(a_op)), span);
        self.terminate_block(true_bb, TerminatorKind::Goto { target: exit_bb }, span);

        self.current_block = Some(false_bb);
        let b_ty = self.get_type(b_expr.id);
        let b_op = if result_ty.is_py_value() && !b_ty.is_py_value() {
            self.emit_to_py_arg(b_op, &b_ty, span)
        } else {
            b_op
        };
        self.push_statement(StatementKind::Assign(result_local, Rvalue::Use(b_op)), span);
        self.terminate_block(false_bb, TerminatorKind::Goto { target: exit_bb }, span);

        self.current_block = Some(exit_bb);
        Some(self.operand_for_local(result_local))
    }

    pub(super) fn lower_enum_variant_call(
        &mut self,
        name: &str,
        arg_ops: Vec<Operand>,
        span: Span,
        _expr_id: usize,
    ) -> Option<Operand> {
        if let Some((enum_name, tag)) = self.enum_variants.get(name).cloned() {
            let type_id = Self::enum_type_id(&enum_name);
            let tmp = self.new_local(self.get_type(_expr_id), None, false);
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Aggregate(AggregateKind::EnumVariant(type_id, tag), arg_ops),
                ),
                span,
            );
            Some(self.operand_for_local(tmp))
        } else {
            None
        }
    }

    pub(super) fn lower_list_new_builtin(
        &mut self,
        args: &[CallArg],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        if args.is_empty() {
            return None;
        }
        let arg_expr = match &args[0] {
            CallArg::Positional(e)
            | CallArg::Keyword(_, e)
            | CallArg::Splat(e)
            | CallArg::KwSplat(e) => e,
        };
        let arg_op = self.lower_expr(arg_expr);
        let result_ty = self.get_type(expr_id);
        let tmp = self.new_local(result_ty, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_new".to_string())),
                    args: vec![arg_op],
                },
            ),
            span,
        );
        Some(self.operand_for_local(tmp))
    }

    pub(super) fn lower_bytes_builtin(
        &mut self,
        name: &str,
        args: &[CallArg],
        span: Span,
    ) -> Option<Operand> {
        let bytes_builtin = match name {
            "bytes_new" => Some(("__olive_buf_new_zeroed", Type::Bytes)),
            "bytes_push" => Some(("__olive_buf_push", Type::Null)),
            "bytes_push_u16_le" => Some(("__olive_buf_push_u16_le", Type::Null)),
            "bytes_push_u32_le" => Some(("__olive_buf_push_u32_le", Type::Null)),
            _ => None,
        };
        if let Some((runtime_name, ret_ty)) = bytes_builtin {
            let arg_ops: Vec<Operand> = args
                .iter()
                .map(|a| match a {
                    CallArg::Positional(e)
                    | CallArg::Keyword(_, e)
                    | CallArg::Splat(e)
                    | CallArg::KwSplat(e) => self.lower_expr_as_copy(e),
                })
                .collect();
            let tmp = self.new_local(ret_ty, None, false);
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(runtime_name.to_string())),
                        args: arg_ops,
                    },
                ),
                span,
            );
            Some(self.operand_for_local(tmp))
        } else {
            None
        }
    }

    pub(super) fn lower_print_builtin(
        &mut self,
        _callee: &Expr,
        args: &[CallArg],
        arg_ops: &[Operand],
        span: Span,
        _expr_id: usize,
    ) -> Operand {
        if args.is_empty() {
            let nl = self.new_local(Type::Int, None, false);
            self.push_statement(
                StatementKind::Assign(
                    nl,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__olive_write_nl".to_string())),
                        args: vec![],
                    },
                ),
                span,
            );
            let ret = self.new_local(Type::Int, None, false);
            self.push_statement(
                StatementKind::Assign(ret, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
                span,
            );
            return self.operand_for_local(ret);
        }
        for (i, arg_op) in arg_ops.iter().enumerate() {
            if i > 0 {
                let space = self.new_local(Type::Int, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        space,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_write_char".to_string(),
                            )),
                            args: vec![Operand::Constant(Constant::Int(32))],
                        },
                    ),
                    span,
                );
            }
            let write = self.new_local(Type::Int, None, false);
            self.push_statement(
                StatementKind::Assign(
                    write,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_write_any".to_string(),
                        )),
                        args: vec![arg_op.clone()],
                    },
                ),
                span,
            );
        }
        let nl = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                nl,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_write_nl".to_string())),
                    args: vec![],
                },
            ),
            span,
        );
        let ret = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(ret, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            span,
        );
        self.operand_for_local(ret)
    }
}
