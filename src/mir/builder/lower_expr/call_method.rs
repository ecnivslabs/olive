use super::super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{CallArg, Expr, ExprKind};
use crate::semantic::types::Type;
use crate::span::Span;

/// Builtins whose runtime function is selected from the argument's concrete
/// static type (see `map_builtin_to_runtime`). Their nominal parameter is
/// `Any`, but boxing the argument would both pick the wrong runtime helper and
/// allocate a boxed scalar on every call, so their operands are passed raw.
fn is_type_dispatched_builtin(name: &str) -> bool {
    matches!(
        name,
        "print"
            | "str"
            | "int"
            | "float"
            | "bool"
            | "iter"
            | "next"
            | "has_next"
            | "slice"
            | "list"
            | "dict"
    )
}

impl<'a> MirBuilder<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_attr_method_call_section(
        &mut self,
        callee: &Expr,
        obj: &Expr,
        attr: &str,
        args: &[CallArg],
        arg_ops: Vec<Operand>,
        arg_kw_names: Vec<Option<String>>,
        arg_tys: Vec<Type>,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let obj_ty = self.get_type(obj.id);
        if obj_ty == Type::PyObject {
            let obj_op = self.lower_expr_as_copy(obj);
            let attr_local = self.new_local(Type::PyObject, None, true);
            self.push_statement(
                StatementKind::Assign(
                    attr_local,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_py_getattr".to_string(),
                        )),
                        args: vec![obj_op, Operand::Constant(Constant::Str(attr.to_string()))],
                    },
                ),
                span,
            );
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
                            func: Operand::Constant(Constant::Function(
                                "__olive_py_call".to_string(),
                            )),
                            args: vec![Operand::Copy(attr_local), Operand::Copy(args_list)],
                        },
                    ),
                    span,
                );
                let raw = self.operand_for_local(result);
                return self.coerce_pyobj_if_needed(raw, expr_id, span);
            } else {
                let kwargs_list = self.new_local(Type::List(Box::new(Type::Any)), None, true);
                self.push_statement(
                    StatementKind::Assign(
                        kwargs_list,
                        Rvalue::Aggregate(AggregateKind::List, kw_ops),
                    ),
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
                                Operand::Copy(attr_local),
                                Operand::Copy(args_list),
                                Operand::Copy(kwargs_list),
                            ],
                        },
                    ),
                    span,
                );
                let raw = self.operand_for_local(result);
                return self.coerce_pyobj_if_needed(raw, expr_id, span);
            }
        }

        if let Some(op) = self.lower_dict_method(obj, attr, &arg_ops, span, expr_id) {
            return op;
        }

        if let Some(op) = self.lower_list_method(obj, attr, &arg_ops, span, expr_id) {
            return op;
        }

        if let Some(op) = self.lower_str_method(obj, attr, &arg_ops, span, expr_id) {
            return op;
        }

        if let ExprKind::Identifier(name) = &obj.kind {
            let obj_ty = self.get_type(obj.id);
            let mut current_obj_ty = obj_ty.clone();
            while let Type::Ref(inner) | Type::MutRef(inner) = current_obj_ty {
                current_obj_ty = *inner;
            }
            let is_struct_var = matches!(
                current_obj_ty,
                Type::Struct(_, _) | Type::TraitObject(_, _) | Type::Any
            ) && self.lookup_var(name).is_some();
            if !is_struct_var {
                let mangled = format!("{}::{}", name, attr);

                let variant_info = self.enum_variants.get(&mangled).cloned();
                if let Some((enum_name, tag)) = variant_info {
                    let type_id = Self::enum_type_id(&enum_name);
                    let tmp = self.new_local(self.get_type(expr_id), None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            tmp,
                            Rvalue::Aggregate(AggregateKind::EnumVariant(type_id, tag), arg_ops),
                        ),
                        span,
                    );
                    return self.operand_for_local(tmp);
                }

                let mangled_str = mangled.clone();
                let callee_op = Operand::Constant(Constant::Function(mangled_str.clone()));
                let call_ret_ty = self.get_type(expr_id);

                if self.c_ffi_fns.contains(&mangled_str)
                    && let Type::Struct(ref sname, ref targs) = call_ret_ty.clone()
                    && sname == "Result"
                    && !targs.is_empty()
                {
                    return self.lower_ffi_result_wrapper(
                        callee_op,
                        arg_ops,
                        targs[0].clone(),
                        span,
                        expr_id,
                        &mangled_str,
                    );
                }

                let callee_ty = self.get_type(callee.id).clone();
                let param_tys = if let Type::Fn(ptys, _, _) = callee_ty {
                    ptys
                } else {
                    Vec::new()
                };

                let final_args = self.pack_fn_call_args(
                    &mangled_str,
                    &arg_ops,
                    &arg_tys,
                    &param_tys,
                    &arg_kw_names,
                    span,
                );

                let tmp = self.new_local(self.get_type(expr_id), None, false);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: callee_op,
                            args: final_args,
                        },
                    ),
                    span,
                );
                return self.operand_for_local(tmp);
            }
        }

        let obj_op = self.lower_expr_as_copy(obj);
        let tmp = self.new_local(self.get_type(expr_id), None, false);

        let mut method_args = vec![obj_op];
        method_args.extend(arg_ops);

        if attr == "copy" {
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__olive_copy".to_string())),
                        args: method_args,
                    },
                ),
                span,
            );
            return self.operand_for_local(tmp);
        }

        let obj_ty = self.get_type(obj.id);

        if let Type::Struct(struct_name, type_args) = &obj_ty {
            let base_method_name = format!("{}::{}", struct_name, attr);
            let method_name = if !type_args.is_empty() {
                self.monomorphize(&base_method_name, type_args)
            } else {
                base_method_name
            };
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(method_name)),
                        args: method_args,
                    },
                ),
                span,
            );
            self.operand_for_local(tmp)
        } else if let Type::TraitObject(trait_name, _) = &obj_ty {
            let method_idx = if let Some(t_def) = self.traits.get(trait_name) {
                t_def
                    .methods
                    .iter()
                    .position(|(n, _)| n == attr)
                    .unwrap_or(0)
            } else {
                0
            };

            let method_ptr_tmp = self.new_local(Type::Any, None, false);
            self.push_statement(
                StatementKind::Assign(
                    method_ptr_tmp,
                    Rvalue::VTableLoad {
                        vtable: method_args[0].clone(),
                        method_idx,
                    },
                ),
                span,
            );

            let data_ptr_tmp = self.new_local(Type::Any, None, false);
            self.push_statement(
                StatementKind::Assign(data_ptr_tmp, Rvalue::FatPtrData(method_args[0].clone())),
                span,
            );

            method_args[0] = Operand::Copy(data_ptr_tmp);

            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Copy(method_ptr_tmp),
                        args: method_args,
                    },
                ),
                span,
            );
            self.operand_for_local(tmp)
        } else {
            let mut inner_ty = obj_ty.clone();
            while let Type::Ref(inner) | Type::MutRef(inner) = &inner_ty {
                inner_ty = *inner.clone();
            }
            if let Type::Struct(struct_name, type_args) = &inner_ty {
                let base_method_name = format!("{}::{}", struct_name, attr);
                let method_name = if !type_args.is_empty() {
                    self.monomorphize(&base_method_name, type_args)
                } else {
                    base_method_name
                };
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(method_name)),
                            args: method_args,
                        },
                    ),
                    span,
                );
            } else {
                let method_name = format!("{:?}::{}", obj_ty, attr);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(method_name)),
                            args: method_args,
                        },
                    ),
                    span,
                );
            }
            self.operand_for_local(tmp)
        }
    }

    /// Routes the dict methods `keys`/`values`/`remove` to their runtime fns,
    /// for a `Dict` or `Any` receiver. `None` for anything else.
    /// Lowers the common string methods to their runtime calls. The receiver is
    /// the first argument, matching the runtime signatures.
    fn lower_str_method(
        &mut self,
        obj: &Expr,
        attr: &str,
        arg_ops: &[Operand],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        let runtime = match attr {
            "upper" => "__olive_str_upper",
            "lower" => "__olive_str_lower",
            "strip" => "__olive_str_trim",
            "lstrip" => "__olive_str_trim_start",
            "rstrip" => "__olive_str_trim_end",
            "split" => "__olive_str_split",
            "join" => "__olive_str_join",
            "replace" => "__olive_str_replace",
            "find" => "__olive_str_find",
            "repeat" => "__olive_str_repeat",
            "contains" => "__olive_str_contains",
            "startswith" => "__olive_str_starts_with",
            "endswith" => "__olive_str_ends_with",
            _ => return None,
        };
        let mut recv_ty = self.get_type(obj.id);
        while let Type::Ref(inner) | Type::MutRef(inner) = recv_ty {
            recv_ty = *inner;
        }
        // An `Any` receiver holding a string is the bare string pointer, so the
        // runtime string functions apply directly.
        if !matches!(recv_ty, Type::Str | Type::Any) {
            return None;
        }
        let obj_op = self.lower_expr_as_copy(obj);
        // `sep.join(list)` maps to `olive_str_join(list, sep)`, so the list comes
        // first; every other method takes the receiver first.
        let call_args = if attr == "join" {
            let mut a = arg_ops.to_vec();
            a.push(obj_op);
            a
        } else {
            let mut a = vec![obj_op];
            a.extend_from_slice(arg_ops);
            a
        };
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(runtime.to_string())),
                    args: call_args,
                },
            ),
            span,
        );
        Some(self.operand_for_local(tmp))
    }

    /// Lowers the mutating methods on a native list to their runtime calls.
    /// `append`/`insert`/`extend` mutate in place and yield the list; `pop` and
    /// `remove` return the removed element.
    fn lower_list_method(
        &mut self,
        obj: &Expr,
        attr: &str,
        arg_ops: &[Operand],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        if !matches!(
            attr,
            "append" | "insert" | "extend" | "remove" | "pop" | "sort" | "reverse"
        ) {
            return None;
        }
        let mut recv_ty = self.get_type(obj.id);
        while let Type::Ref(inner) | Type::MutRef(inner) = recv_ty {
            recv_ty = *inner;
        }
        // An `Any` receiver holding a list is the bare list pointer, so the
        // runtime list functions apply directly. Its element type is unknown, so
        // `sort` falls back to the integer ordering.
        let elem: Type = match &recv_ty {
            Type::List(e) => (**e).clone(),
            Type::Any => Type::Any,
            _ => return None,
        };
        let elem = &elem;
        let runtime = match attr {
            "append" => "__olive_list_append",
            "insert" => "__olive_list_insert",
            "extend" => "__olive_list_extend",
            "remove" => "__olive_list_remove",
            "pop" => "__olive_list_pop",
            "reverse" => "__olive_list_reverse",
            "sort" => match elem {
                Type::Float | Type::F32 => "__olive_list_sort_float",
                Type::Str => "__olive_list_sort_str",
                _ => "__olive_list_sort_int",
            },
            _ => return None,
        };
        let obj_op = self.lower_expr_as_copy(obj);
        let returns_elem = matches!(attr, "pop" | "remove");
        let mut call_args = vec![obj_op.clone()];
        call_args.extend_from_slice(arg_ops);
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(runtime.to_string())),
                    args: call_args,
                },
            ),
            span,
        );
        if returns_elem {
            Some(self.operand_for_local(tmp))
        } else {
            Some(obj_op)
        }
    }

    fn lower_dict_method(
        &mut self,
        obj: &Expr,
        attr: &str,
        arg_ops: &[Operand],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        let runtime = match attr {
            "keys" => "__olive_obj_keys",
            "values" => "__olive_obj_values",
            "items" => "__olive_obj_items",
            "get" => "__olive_obj_get",
            "remove" => "__olive_obj_remove",
            _ => return None,
        };
        let mut recv_ty = self.get_type(obj.id);
        while let Type::Ref(inner) | Type::MutRef(inner) = recv_ty {
            recv_ty = *inner;
        }
        if !matches!(recv_ty, Type::Dict(_, _) | Type::Any) {
            return None;
        }
        let obj_op = self.lower_expr_as_copy(obj);
        let mut call_args = vec![obj_op];
        call_args.extend_from_slice(arg_ops);
        let tmp = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(runtime.to_string())),
                    args: call_args,
                },
            ),
            span,
        );
        Some(self.operand_for_local(tmp))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_general_call_path(
        &mut self,
        callee: &Expr,
        mut func: Operand,
        arg_ops: Vec<Operand>,
        arg_kw_names: Vec<Option<String>>,
        arg_tys: Vec<Type>,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let mut call_fn_name = if let ExprKind::Identifier(name) = &callee.kind {
            Some(name.clone())
        } else if let ExprKind::Attr { obj, attr } = &callee.kind {
            if let ExprKind::Identifier(obj_name) = &obj.kind {
                Some(format!("{}::{}", obj_name, attr))
            } else {
                None
            }
        } else {
            None
        };

        if call_fn_name.is_none()
            && let Operand::Constant(Constant::Function(name)) = &func
        {
            call_fn_name = Some(name.clone());
        }

        if let Some(fn_name) = &call_fn_name {
            let callee_ty = self.get_type(callee.id);
            if let Type::Fn(_, _, type_args) = callee_ty
                && !type_args.is_empty()
                && self.generic_fns.contains_key(fn_name)
            {
                let specialized_name = self.monomorphize(fn_name, &type_args);
                func = Operand::Constant(Constant::Function(specialized_name.clone()));
                call_fn_name = Some(specialized_name);
            }
        }

        let ret_ty = self.get_type(expr_id);
        let mut is_ffi_result = false;
        let mut inner_ok_ty = Type::Any;

        if let Some(fn_name) = &call_fn_name
            && self.c_ffi_fns.contains(fn_name)
            && let Type::Struct(struct_name, type_args) = &ret_ty
            && struct_name == "Result"
            && !type_args.is_empty()
        {
            is_ffi_result = true;
            inner_ok_ty = type_args[0].clone();
        }

        let call_result_local = if is_ffi_result {
            self.new_local(inner_ok_ty.clone(), None, false)
        } else {
            self.new_local(self.get_type(expr_id), None, false)
        };

        let callee_ty = self.get_type(callee.id).clone();
        let param_tys = if let Type::Fn(ptys, _, _) = callee_ty {
            ptys
        } else {
            Vec::new()
        };

        // The type-dispatched builtins take a nominal `Any` parameter but
        // resolve a concrete runtime function from the argument's static type
        // (e.g. `int(f64)` -> `float_to_int`). Boxing the argument into `Any`
        // would defeat that dispatch and, in a hot loop, allocate a boxed
        // scalar per call, so they receive their operands raw.
        let final_args = if call_fn_name
            .as_deref()
            .is_some_and(is_type_dispatched_builtin)
        {
            arg_ops.clone()
        } else if let Some(name) = &call_fn_name {
            self.pack_fn_call_args(name, &arg_ops, &arg_tys, &param_tys, &arg_kw_names, span)
        } else {
            let mut res = Vec::new();
            for i in 0..arg_ops.len() {
                let p_ty = param_tys.get(i).unwrap_or(&Type::Any);
                res.push(self.coerce(arg_ops[i].clone(), &arg_tys[i], p_ty, span));
            }
            res
        };

        if is_ffi_result {
            self.clear_ffi_errno(span);
        }
        self.push_statement(
            StatementKind::Assign(
                call_result_local,
                Rvalue::Call {
                    func,
                    args: final_args,
                },
            ),
            span,
        );

        if is_ffi_result {
            let fn_name = call_fn_name.as_deref().unwrap_or("FFI call");
            return self.lower_ffi_result_post(
                call_result_local,
                inner_ok_ty,
                span,
                expr_id,
                fn_name,
            );
        }

        self.operand_for_local(call_result_local)
    }

    /// Resets `errno` to 0 before an FFI call so a value read afterwards is known
    /// to belong to that call rather than a stale value from an earlier one.
    fn clear_ffi_errno(&mut self, span: Span) {
        let sink = self.new_local(Type::Null, None, false);
        self.push_statement(
            StatementKind::Assign(
                sink,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(
                        "__olive_ffi_clear_errno".to_string(),
                    )),
                    args: vec![],
                },
            ),
            span,
        );
    }

    /// Reads `errno` immediately after an FFI call, before any other runtime call
    /// (such as a string allocation) can overwrite it.
    fn capture_ffi_errno(&mut self, span: Span) -> Local {
        let errno_local = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                errno_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_ffi_errno".to_string())),
                    args: vec![],
                },
            ),
            span,
        );
        errno_local
    }

    /// Builds the `<line>:<col>: <fn>: <strerror(errno)>` message string for a
    /// failed FFI call from a previously captured errno value. The call-site
    /// location lets a runtime error be traced back to the exact source line.
    fn ffi_error_message(&mut self, fn_name: &str, errno_local: Local, span: Span) -> Local {
        let located = match self.file_names.get(&span.file_id) {
            Some(file) => format!("{}:{}:{}: {}", file, span.line, span.col, fn_name),
            None => format!("{}:{}: {}", span.line, span.col, fn_name),
        };
        let name_local = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                name_local,
                Rvalue::Use(Operand::Constant(Constant::Str(located))),
            ),
            span,
        );
        let msg_local = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                msg_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_ffi_errmsg".to_string())),
                    args: vec![Operand::Copy(name_local), Operand::Copy(errno_local)],
                },
            ),
            span,
        );
        msg_local
    }

    pub(super) fn lower_ffi_result_wrapper(
        &mut self,
        callee_op: Operand,
        arg_ops: Vec<Operand>,
        inner_ok_ty: Type,
        span: Span,
        expr_id: usize,
        fn_name: &str,
    ) -> Operand {
        let raw_local = self.new_local(inner_ok_ty.clone(), None, false);
        self.clear_ffi_errno(span);
        self.push_statement(
            StatementKind::Assign(
                raw_local,
                Rvalue::Call {
                    func: callee_op,
                    args: arg_ops,
                },
            ),
            span,
        );
        let errno_local = self.capture_ffi_errno(span);

        let is_err_local = self.new_local(Type::Bool, None, false);
        match &inner_ok_ty {
            Type::Int => {
                self.push_statement(
                    StatementKind::Assign(
                        is_err_local,
                        Rvalue::BinaryOp(
                            crate::parser::BinOp::Eq,
                            Operand::Copy(raw_local),
                            Operand::Constant(Constant::Int(-1)),
                        ),
                    ),
                    span,
                );
            }
            Type::Ref(_) | Type::MutRef(_) | Type::Ptr(_) => {
                self.push_statement(
                    StatementKind::Assign(
                        is_err_local,
                        Rvalue::BinaryOp(
                            crate::parser::BinOp::Eq,
                            Operand::Copy(raw_local),
                            Operand::Constant(Constant::Int(0)),
                        ),
                    ),
                    span,
                );
            }
            _ => {
                self.push_statement(
                    StatementKind::Assign(
                        is_err_local,
                        Rvalue::Use(Operand::Constant(Constant::Bool(false))),
                    ),
                    span,
                );
            }
        }

        let ok_bb = self.new_block();
        let err_bb = self.new_block();
        let exit_bb = self.new_block();

        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::SwitchInt {
                    discr: Operand::Copy(is_err_local),
                    targets: vec![(1, err_bb)],
                    otherwise: ok_bb,
                },
                span,
            );
        }

        let result_local = self.new_local(self.get_type(expr_id), None, false);

        self.current_block = Some(ok_bb);
        self.push_statement(
            StatementKind::Assign(
                result_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_result_ok".to_string())),
                    args: vec![Operand::Copy(raw_local)],
                },
            ),
            span,
        );
        self.terminate_block(ok_bb, TerminatorKind::Goto { target: exit_bb }, span);

        self.current_block = Some(err_bb);
        let err_str_local = self.ffi_error_message(fn_name, errno_local, span);
        self.push_statement(
            StatementKind::Assign(
                result_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_result_err".to_string())),
                    args: vec![Operand::Copy(err_str_local)],
                },
            ),
            span,
        );
        self.terminate_block(err_bb, TerminatorKind::Goto { target: exit_bb }, span);

        self.current_block = Some(exit_bb);
        self.operand_for_local(result_local)
    }

    fn lower_ffi_result_post(
        &mut self,
        call_result_local: Local,
        inner_ok_ty: Type,
        span: Span,
        expr_id: usize,
        fn_name: &str,
    ) -> Operand {
        let errno_local = self.capture_ffi_errno(span);
        let is_err_tmp = self.new_local(Type::Bool, None, false);
        match &inner_ok_ty {
            Type::Int => {
                self.push_statement(
                    StatementKind::Assign(
                        is_err_tmp,
                        Rvalue::BinaryOp(
                            crate::parser::BinOp::Eq,
                            Operand::Copy(call_result_local),
                            Operand::Constant(Constant::Int(-1)),
                        ),
                    ),
                    span,
                );
            }
            Type::Ref(_) | Type::MutRef(_) | Type::Ptr(_) => {
                self.push_statement(
                    StatementKind::Assign(
                        is_err_tmp,
                        Rvalue::BinaryOp(
                            crate::parser::BinOp::Eq,
                            Operand::Copy(call_result_local),
                            Operand::Constant(Constant::Int(0)),
                        ),
                    ),
                    span,
                );
            }
            _ => {
                self.push_statement(
                    StatementKind::Assign(
                        is_err_tmp,
                        Rvalue::Use(Operand::Constant(Constant::Bool(false))),
                    ),
                    span,
                );
            }
        }

        let ok_bb = self.new_block();
        let err_bb = self.new_block();
        let exit_bb = self.new_block();

        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::SwitchInt {
                    discr: Operand::Copy(is_err_tmp),
                    targets: vec![(1, err_bb)],
                    otherwise: ok_bb,
                },
                span,
            );
        }

        let result_tmp = self.new_local(self.get_type(expr_id), None, false);

        self.current_block = Some(ok_bb);
        self.push_statement(
            StatementKind::Assign(
                result_tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_result_ok".to_string())),
                    args: vec![Operand::Copy(call_result_local)],
                },
            ),
            span,
        );
        self.terminate_block(ok_bb, TerminatorKind::Goto { target: exit_bb }, span);

        self.current_block = Some(err_bb);
        let err_str_tmp = self.ffi_error_message(fn_name, errno_local, span);
        self.push_statement(
            StatementKind::Assign(
                result_tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_result_err".to_string())),
                    args: vec![Operand::Copy(err_str_tmp)],
                },
            ),
            span,
        );
        self.terminate_block(err_bb, TerminatorKind::Goto { target: exit_bb }, span);

        self.current_block = Some(exit_bb);
        self.operand_for_local(result_tmp)
    }

    pub(super) fn lower_struct_construct_call(
        &mut self,
        struct_name: &str,
        type_args: &[Type],
        arg_ops: Vec<Operand>,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        let obj_tmp = self.new_unscoped_local(self.get_type(expr_id));
        let alloc_rval = if let Some(fields) = self.struct_fields.get(struct_name) {
            let n = fields.len() as i64;
            Rvalue::Call {
                func: Operand::Constant(Constant::Function("__olive_struct_alloc".to_string())),
                args: vec![Operand::Constant(Constant::Int(n))],
            }
        } else {
            Rvalue::Call {
                func: Operand::Constant(Constant::Function("__olive_obj_new".to_string())),
                args: vec![],
            }
        };
        self.push_statement(StatementKind::Assign(obj_tmp, alloc_rval), span);

        let base_init_name = format!("{}::__init__", struct_name);
        let init_name = if !type_args.is_empty() {
            self.monomorphize(&base_init_name, type_args)
        } else {
            base_init_name
        };
        let mut init_args = vec![Operand::Copy(obj_tmp)];
        init_args.extend(arg_ops);

        // Fill any omitted trailing fields with their declared default value, so
        // a struct with defaults can be built from a prefix of its fields. A
        // default's type is unified with its field, matching the provided args.
        if let Some(defaults) = self.struct_field_defaults.get(struct_name).cloned() {
            let supplied = init_args.len() - 1;
            for default in defaults.iter().skip(supplied) {
                let Some(default_expr) = default else {
                    break;
                };
                init_args.push(self.lower_expr_as_copy(default_expr));
            }
        }

        let init_res = self.new_local(self.get_type(expr_id), None, false);
        self.push_statement(
            StatementKind::Assign(
                init_res,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(init_name)),
                    args: init_args,
                },
            ),
            span,
        );

        Operand::Copy(obj_tmp)
    }
}
