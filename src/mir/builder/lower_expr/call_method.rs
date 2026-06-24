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
            | "abs"
            | "round"
            | "input"
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
        if let ExprKind::Identifier(name) = &obj.kind
            && self.has_native_module_fn(name, attr)
        {
            let mangled = format!("{}::{}", name, attr);
            let func_op = Operand::Constant(Constant::Function(mangled));
            return self.lower_general_call_path(
                callee,
                func_op,
                arg_ops,
                arg_kw_names,
                arg_tys,
                span,
                expr_id,
            );
        }

        let obj_ty = self.get_type(obj.id);

        // `obj.field(...)` where `field` is a plain `fn`-typed struct field,
        // not a method: read the closure value and call it indirectly (E5.3).
        // A struct cannot declare a field and a method of the same name, so
        // field presence alone disambiguates from a real method call.
        let mut struct_ty = obj_ty.clone();
        while let Type::Ref(inner) | Type::MutRef(inner) = struct_ty {
            struct_ty = *inner;
        }
        if let Type::Struct(struct_name, _, _) = &struct_ty
            && let Some(field_ty) = self
                .struct_field_types
                .get(&(struct_name.clone(), attr.to_string()))
                .cloned()
            && matches!(field_ty, Type::Fn(_, _, _))
        {
            let obj_op = self.lower_expr_as_copy(obj);
            let field_local = self.new_local(field_ty, None, true);
            self.push_statement(
                StatementKind::Assign(field_local, Rvalue::GetAttr(obj_op, attr.to_string())),
                span,
            );
            return self.lower_general_call_path(
                callee,
                Operand::Copy(field_local),
                arg_ops,
                arg_kw_names,
                arg_tys,
                span,
                expr_id,
            );
        }

        if obj_ty.is_py_value() {
            let obj_op = self.lower_expr_as_copy(obj);
            let call_args = self.build_py_call_args(args, arg_ops, arg_kw_names, span);
            // The real declared result type, not always `PyObject`: when it's
            // one of the scalars R10 fuses, `emit_py_method_call` converts
            // inside the call itself and returns an already-realized operand
            // of exactly that type, making the `coerce_pyobj_if_needed` below
            // a no-op identity cast (its target and the operand's actual type
            // already agree) instead of a second boundary crossing.
            let raw = self.emit_py_method_call(
                obj_op,
                attr.to_string(),
                call_args,
                super::py_call::PyCallFlavor::Unsafe,
                self.get_type(expr_id),
                span,
            );
            return self.coerce_pyobj_if_needed(raw, expr_id, span);
        }

        if let Some(op) = self.lower_dict_method(obj, attr, &arg_ops, &arg_tys, span, expr_id) {
            return op;
        }

        if let Some(op) = self.lower_list_method(obj, attr, args, &arg_ops, &arg_tys, span, expr_id)
        {
            return op;
        }

        if let Some(op) = self.lower_set_method(obj, attr, &arg_ops, &arg_tys, span, expr_id) {
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
                Type::Struct(_, _, _) | Type::Enum(_, _) | Type::TraitObject(_, _) | Type::Any
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
                    && let Type::Struct(ref sname, ref targs, _) = call_ret_ty.clone()
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

        // Coerce args to param types so scalars box into Any, matching free-function call path.
        let callee_ty = self.get_type(callee.id);
        let coerced_args: Vec<Operand> = if let Type::Fn(ptys, _, _) = &callee_ty {
            // Skip only the receiver `self`; trailing defaults are filled
            // separately, so they must not shift the positional alignment.
            let offset = ptys.len().saturating_sub(arg_ops.len()).min(1);
            arg_ops
                .iter()
                .enumerate()
                .map(|(i, op)| {
                    let from = arg_tys.get(i).cloned().unwrap_or(Type::Any);
                    let to = ptys.get(i + offset).cloned().unwrap_or(Type::Any);
                    self.coerce(op.clone(), &from, &to, span)
                })
                .collect()
        } else {
            arg_ops
        };

        let mut method_args = vec![obj_op];
        method_args.extend(coerced_args);

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

        if let Type::Struct(struct_name, type_args, _) = &obj_ty {
            let base_method_name = format!("{}::{}", struct_name, attr);
            let method_name = if !type_args.is_empty() {
                self.monomorphize(&base_method_name, type_args)
            } else {
                base_method_name
            };
            self.fill_trailing_defaults(&method_name, &mut method_args, callee.id, span);
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
        } else if let Type::Enum(enum_name, type_args) = &obj_ty {
            let base_method_name = format!("{}::{}", enum_name, attr);
            let method_name = if !type_args.is_empty() {
                self.monomorphize(&base_method_name, type_args)
            } else {
                base_method_name
            };
            self.fill_trailing_defaults(&method_name, &mut method_args, callee.id, span);
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
            if let Type::Struct(struct_name, type_args, _) = &inner_ty {
                let base_method_name = format!("{}::{}", struct_name, attr);
                let method_name = if !type_args.is_empty() {
                    self.monomorphize(&base_method_name, type_args)
                } else {
                    base_method_name
                };
                self.fill_trailing_defaults(&method_name, &mut method_args, callee.id, span);
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
            } else if let Type::Enum(enum_name, type_args) = &inner_ty {
                let base_method_name = format!("{}::{}", enum_name, attr);
                let method_name = if !type_args.is_empty() {
                    self.monomorphize(&base_method_name, type_args)
                } else {
                    base_method_name
                };
                self.fill_trailing_defaults(&method_name, &mut method_args, callee.id, span);
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

    /// Fills omitted trailing-default params so the call matches the method's
    /// arity. `callee_id` types the params for coercion. Positional only.
    fn fill_trailing_defaults(
        &mut self,
        fn_name: &str,
        args: &mut Vec<Operand>,
        callee_id: usize,
        span: Span,
    ) {
        let meta = match self.fn_meta.get(fn_name).cloned() {
            Some(m) => m,
            None => return,
        };
        let param_tys = match self.get_type(callee_id) {
            Type::Fn(ptys, _, _) => ptys,
            _ => Vec::new(),
        };
        while args.len() < meta.param_names.len() {
            let i = args.len();
            match meta.default_exprs.get(i) {
                Some(Some(default_expr)) => {
                    let op = self.lower_expr_as_copy(default_expr);
                    let from_ty = self.get_type(default_expr.id);
                    let to_ty = param_tys.get(i).unwrap_or(&Type::Any);
                    let coerced = self.coerce(op, &from_ty, to_ty, span);
                    args.push(coerced);
                }
                _ => break,
            }
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
        // Methods with one runtime function regardless of argument count.
        let fixed = match attr {
            "upper" => Some("__olive_str_upper"),
            "lower" => Some("__olive_str_lower"),
            "join" => Some("__olive_str_join"),
            "replace" => Some("__olive_str_replace"),
            "find" => Some("__olive_str_find"),
            "rfind" => Some("__olive_str_rfind"),
            "count" => Some("__olive_str_count"),
            "repeat" => Some("__olive_str_repeat"),
            "contains" => Some("__olive_str_contains"),
            "startswith" => Some("__olive_str_starts_with"),
            "endswith" => Some("__olive_str_ends_with"),
            "splitlines" => Some("__olive_str_splitlines"),
            "title" => Some("__olive_str_title"),
            "capitalize" => Some("__olive_str_capitalize"),
            "zfill" => Some("__olive_str_zfill"),
            "removeprefix" => Some("__olive_str_removeprefix"),
            "removesuffix" => Some("__olive_str_removesuffix"),
            "isdigit" => Some("__olive_str_isdigit"),
            "isalpha" => Some("__olive_str_isalpha"),
            "isspace" => Some("__olive_str_isspace"),
            "isupper" => Some("__olive_str_isupper"),
            "islower" => Some("__olive_str_islower"),
            "partition" => Some("__olive_str_partition"),
            "to_int" => Some("__olive_str_to_int_opt"),
            "to_float" => Some("__olive_str_to_float_opt"),
            _ => None,
        };
        // Optional-argument methods: the no-arg form uses the whitespace/plain
        // runtime fn, a given argument switches to the `_chars`/explicit-arg
        // variant. `ljust`/`rjust`/`center` default the fill char to a space.
        let variable = match (attr, arg_ops.len()) {
            ("strip", 0) => Some(("__olive_str_trim", vec![])),
            ("strip", _) => Some(("__olive_str_trim_chars", arg_ops.to_vec())),
            ("lstrip", 0) => Some(("__olive_str_trim_start", vec![])),
            ("lstrip", _) => Some(("__olive_str_trim_start_chars", arg_ops.to_vec())),
            ("rstrip", 0) => Some(("__olive_str_trim_end", vec![])),
            ("rstrip", _) => Some(("__olive_str_trim_end_chars", arg_ops.to_vec())),
            ("split", 0) => Some((
                "__olive_str_split",
                vec![Operand::Constant(Constant::Int(0))],
            )),
            ("split", _) => Some(("__olive_str_split", arg_ops.to_vec())),
            ("ljust", _) => Some(("__olive_str_ljust", Self::pad_args(arg_ops))),
            ("rjust", _) => Some(("__olive_str_rjust", Self::pad_args(arg_ops))),
            ("center", _) => Some(("__olive_str_center", Self::pad_args(arg_ops))),
            _ => None,
        };
        let (runtime, method_args): (&str, Vec<Operand>) = match (fixed, variable) {
            (Some(r), _) => (r, arg_ops.to_vec()),
            (None, Some((r, a))) => (r, a),
            (None, None) => return None,
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
            let mut a = method_args;
            a.push(obj_op);
            a
        } else {
            let mut a = vec![obj_op];
            a.extend(method_args);
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

    /// `ljust`/`rjust`/`center` default the fill char to a space when omitted.
    fn pad_args(arg_ops: &[Operand]) -> Vec<Operand> {
        if arg_ops.len() >= 2 {
            arg_ops.to_vec()
        } else {
            let mut v = arg_ops.to_vec();
            v.push(Operand::Constant(Constant::Str(" ".to_string())));
            v
        }
    }

    /// Lowers the mutating methods on a native list to their runtime calls.
    /// `append`/`insert`/`extend` mutate in place and yield the list; `pop` and
    /// `remove` return the removed element.
    #[allow(clippy::too_many_arguments)]
    fn lower_list_method(
        &mut self,
        obj: &Expr,
        attr: &str,
        raw_args: &[CallArg],
        arg_ops: &[Operand],
        arg_tys: &[Type],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        if matches!(attr, "count" | "index" | "clear") {
            return self.lower_list_method_ext(obj, attr, arg_ops, span, expr_id);
        }
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

        if attr == "sort"
            && let Some(key_idx) = raw_args
                .iter()
                .position(|a| matches!(a, CallArg::Keyword(name, _) if name == "key"))
        {
            let key_op = arg_ops[key_idx].clone();
            let key_ret_ty = match &arg_tys[key_idx] {
                Type::Fn(_, ret, _) => (**ret).clone(),
                _ => Type::Int,
            };
            let obj_op = self.lower_expr_as_copy(obj);
            return Some(self.lower_sort_by_key(obj_op, elem, key_op, &key_ret_ty, span));
        }

        // E6.3: `.sort()` with no key on a struct element list uses the
        // struct's own `__lt__` (checker already required it) instead of
        // falling into the integer-ordering default below.
        if attr == "sort"
            && let Type::Struct(struct_name, ..) = elem
            && self.fn_meta.contains_key(&format!("{struct_name}::__lt__"))
        {
            let obj_op = self.lower_expr_as_copy(obj);
            return Some(self.lower_sort_by_lt(obj_op, elem, span));
        }

        let runtime = match attr {
            "append" => "__olive_list_append",
            "insert" => "__olive_list_insert",
            // Heap-element source keeps its elements; target needs independent copies.
            "extend" if matches!(&recv_ty, Type::List(e) if Self::list_elem_needs_copy(e)) => {
                "__olive_list_extend_typed"
            }
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
        // A scalar stored into or matched against an `[Any]` element slot is
        // boxed the same way a list literal element is, so the stored word stays
        // self-describing and a lookup word matches it. `insert(i, v)` boxes its
        // value (second) argument; the rest box their first.
        let value_arg = match attr {
            "append" | "remove" => Some(0usize),
            "insert" => Some(1usize),
            _ => None,
        };
        let mut call_args = vec![obj_op.clone()];
        if *elem == Type::Any && value_arg.is_some() {
            for (i, op) in arg_ops.iter().enumerate() {
                if Some(i) == value_arg {
                    let from_ty = arg_tys.get(i).cloned().unwrap_or(Type::Any);
                    call_args.push(self.box_into_any(op.clone(), &from_ty, span));
                } else {
                    call_args.push(op.clone());
                }
            }
        } else {
            call_args.extend_from_slice(arg_ops);
        }
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

    /// `count(x)`/`index(x)`/`clear()` (E3.6): `count`/`index` always go
    /// through the descriptor-typed comparison (`olive_eq_typed`'s raw
    /// fast path already covers scalars), the descriptor synthesized at
    /// codegen time from the value argument's own type (arg position 1),
    /// same pattern as set add/remove/contains. `index` faults on a miss.
    fn lower_list_method_ext(
        &mut self,
        obj: &Expr,
        attr: &str,
        arg_ops: &[Operand],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        let mut recv_ty = self.get_type(obj.id);
        while let Type::Ref(inner) | Type::MutRef(inner) = recv_ty {
            recv_ty = *inner;
        }
        if !matches!(recv_ty, Type::List(_) | Type::Any) {
            return None;
        }
        let obj_op = self.lower_expr_as_copy(obj);
        let val_op = arg_ops
            .first()
            .cloned()
            .unwrap_or(Operand::Constant(Constant::Int(0)));
        let (runtime, call_args): (&str, Vec<Operand>) = match attr {
            "count" => ("__olive_list_count_typed", vec![obj_op.clone(), val_op]),
            "index" => (
                "__olive_list_index_typed",
                vec![obj_op.clone(), val_op, self.index_loc_operand(span)],
            ),
            "clear" => ("__olive_list_clear", vec![obj_op.clone()]),
            _ => return None,
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
        if attr == "clear" {
            Some(obj_op)
        } else {
            Some(self.operand_for_local(tmp))
        }
    }

    /// Lowers `add`/`remove` on a native set to the runtime calls. Both mutate in
    /// place; `add` yields the set, `remove` yields the removed element.
    fn lower_set_method(
        &mut self,
        obj: &Expr,
        attr: &str,
        arg_ops: &[Operand],
        arg_tys: &[Type],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        if matches!(attr, "clear") {
            let obj_op = self.lower_expr_as_copy(obj);
            let tmp = self.new_local(self.get_type(expr_id), None, false);
            self.push_statement(
                StatementKind::Assign(
                    tmp,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__olive_set_clear".into())),
                        args: vec![obj_op.clone()],
                    },
                ),
                span,
            );
            return Some(obj_op);
        }
        if !matches!(attr, "add" | "remove" | "discard" | "contains") {
            return None;
        }
        let mut recv_ty = self.get_type(obj.id);
        while let Type::Ref(inner) | Type::MutRef(inner) = recv_ty {
            recv_ty = *inner;
        }
        let elem: Type = match &recv_ty {
            Type::Set(e) => (**e).clone(),
            _ => return None,
        };
        // A struct/enum/tuple/collection element needs the same structural
        // hash+eq `==` derives, which the plain runtime ops don't have a
        // type descriptor to compute; the `_typed` variants set it (see
        // `hash_typed.rs`), the descriptor synthesized at codegen time from
        // this call's own value argument (arg position 1), same pattern as
        // `__olive_list_extend_typed`. `remove` faults on a miss (Python
        // semantics), `discard` keeps the old silent behavior.
        let structural = Self::type_needs_structural_key(&elem);
        let runtime = match (attr, structural) {
            ("add", false) => "__olive_set_add",
            ("add", true) => "__olive_set_add_typed",
            ("remove", false) => "__olive_set_remove_checked",
            ("remove", true) => "__olive_set_remove_checked_typed",
            ("discard", false) => "__olive_set_remove",
            ("discard", true) => "__olive_set_remove_typed",
            ("contains", false) => "__olive_set_contains",
            ("contains", true) => "__olive_set_contains_typed",
            _ => return None,
        };
        let obj_op = self.lower_expr_as_copy(obj);
        let mut call_args = vec![obj_op.clone()];
        // An `Any`-element set boxes its scalar argument so the stored word stays
        // self-describing, the same as list elements.
        if let Some(op) = arg_ops.first() {
            if elem == Type::Any {
                let from_ty = arg_tys.first().cloned().unwrap_or(Type::Any);
                call_args.push(self.box_into_any(op.clone(), &from_ty, span));
            } else {
                call_args.push(op.clone());
            }
        }
        if attr == "remove" {
            call_args.push(self.index_loc_operand(span));
        }
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
        if attr == "add" {
            Some(obj_op)
        } else {
            Some(self.operand_for_local(tmp))
        }
    }

    fn lower_dict_method(
        &mut self,
        obj: &Expr,
        attr: &str,
        arg_ops: &[Operand],
        arg_tys: &[Type],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        if matches!(attr, "pop" | "setdefault" | "update" | "clear") {
            return self.lower_dict_method_ext(obj, attr, arg_ops, arg_tys, span, expr_id);
        }
        let mut recv_ty = self.get_type(obj.id);
        while let Type::Ref(inner) | Type::MutRef(inner) = recv_ty {
            recv_ty = *inner;
        }
        let key_structural = match &recv_ty {
            Type::Dict(k, _) => Self::type_needs_structural_key(k),
            _ => false,
        };
        // A struct/enum/tuple/collection key needs the structural hash+eq
        // `==` derives; the `_typed` variants set the descriptor `hash_typed`
        // consults, synthesized at codegen time from this call's own key
        // argument (arg position 1), same pattern as set add/remove/contains.
        let runtime = match (attr, key_structural) {
            ("keys", _) => "__olive_obj_keys",
            ("values", _) => "__olive_obj_values",
            ("items", _) => "__olive_obj_items",
            ("get", false) if arg_ops.len() == 2 => "__olive_obj_get_default",
            ("get", true) if arg_ops.len() == 2 => "__olive_obj_get_default_typed",
            ("get", false) => "__olive_obj_get",
            ("get", true) => "__olive_obj_get_typed",
            ("remove", _) => "__olive_obj_remove",
            _ => return None,
        };
        let val_ty = match &recv_ty {
            Type::Dict(_, v) => (**v).clone(),
            Type::Any => Type::Any,
            _ => return None,
        };
        let obj_op = self.lower_expr_as_copy(obj);
        let mut call_args = vec![obj_op];
        if runtime == "__olive_obj_get_default" {
            // Default must be boxed into Any so it reads back the same as stored values.
            call_args.push(arg_ops[0].clone());
            let default = arg_ops[1].clone();
            if val_ty == Type::Any {
                let from_ty = arg_tys.get(1).cloned().unwrap_or(Type::Any);
                call_args.push(self.box_into_any(default, &from_ty, span));
            } else {
                call_args.push(default);
            }
        } else {
            call_args.extend_from_slice(arg_ops);
        }
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

    /// `pop`/`setdefault`/`update`/`clear` (E3.6). `pop`/`setdefault` reuse
    /// `get`'s structural-key dispatch (arg position 1 is the key); `update`
    /// dispatches on whether the dict's *value* type owns heap data (arg
    /// position 1 there is the whole source dict, matching
    /// `__olive_list_extend_typed`).
    fn lower_dict_method_ext(
        &mut self,
        obj: &Expr,
        attr: &str,
        arg_ops: &[Operand],
        arg_tys: &[Type],
        span: Span,
        expr_id: usize,
    ) -> Option<Operand> {
        let mut recv_ty = self.get_type(obj.id);
        while let Type::Ref(inner) | Type::MutRef(inner) = recv_ty {
            recv_ty = *inner;
        }
        let (key_ty, val_ty): (Type, Type) = match &recv_ty {
            Type::Dict(k, v) => ((**k).clone(), (**v).clone()),
            Type::Any => (Type::Any, Type::Any),
            _ => return None,
        };
        let key_structural = Self::type_needs_structural_key(&key_ty);
        let obj_op = self.lower_expr_as_copy(obj);
        let zero = || Operand::Constant(Constant::Int(0));
        let box_val = |b: &mut Self, op: Operand, idx: usize| {
            if val_ty == Type::Any {
                let from_ty = arg_tys.get(idx).cloned().unwrap_or(Type::Any);
                b.box_into_any(op, &from_ty, span)
            } else {
                op
            }
        };
        let (runtime, call_args): (&str, Vec<Operand>) = match attr {
            "clear" => ("__olive_obj_clear", vec![obj_op.clone()]),
            "pop" if arg_ops.len() >= 2 => {
                let key_op = arg_ops[0].clone();
                let default = box_val(self, arg_ops[1].clone(), 1);
                let f = if key_structural {
                    "__olive_obj_pop_default_typed"
                } else {
                    "__olive_obj_pop_default"
                };
                (f, vec![obj_op.clone(), key_op, default])
            }
            "pop" => {
                let key_op = arg_ops.first().cloned().unwrap_or(zero());
                let loc = self.index_loc_operand(span);
                let f = if key_structural {
                    "__olive_obj_pop_checked_typed"
                } else {
                    "__olive_obj_pop_checked"
                };
                (f, vec![obj_op.clone(), key_op, loc])
            }
            "setdefault" => {
                let key_op = arg_ops.first().cloned().unwrap_or(zero());
                let default = box_val(self, arg_ops.get(1).cloned().unwrap_or(zero()), 1);
                let f = if key_structural {
                    "__olive_obj_setdefault_typed"
                } else {
                    "__olive_obj_setdefault"
                };
                (f, vec![obj_op.clone(), key_op, default])
            }
            "update" => {
                let other = arg_ops.first().cloned().unwrap_or(zero());
                let f = if Self::list_elem_needs_copy(&val_ty) {
                    "__olive_obj_update_typed"
                } else {
                    "__olive_obj_update"
                };
                (f, vec![obj_op.clone(), other])
            }
            _ => return None,
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
        if matches!(attr, "clear" | "update") {
            Some(obj_op)
        } else {
            Some(self.operand_for_local(tmp))
        }
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
        // A local (a fn-typed param/var, e.g. `apply(f: fn(int) -> int, ...)`)
        // must never be treated as a global fn name here: nothing stops it
        // from sharing a name with an unrelated global of a different
        // shape, and `fn_meta`/`pack_fn_call_args` below is keyed globally
        // by name, not scope. Reached only when `lower_call_expr`'s own
        // `nested_fns`/`bound_lambdas` lookups (also local-aware) missed.
        // Same exception as a local: a module-scope `let` (`self.globals`)
        // is never a real function symbol under its own bare name either --
        // `func` already carries the correctly-resolved callable operand for
        // that case (see the `Constant::Function` fallback just below, or an
        // indirect call through a loaded closure record).
        let mut call_fn_name = if let ExprKind::Identifier(name) = &callee.kind
            && self.lookup_var(name).is_none()
            && !self.globals.contains_key(name)
        {
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
            && let Type::Struct(struct_name, type_args, _) = &ret_ty
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

        // A synthesized call (with-exit, defer) carries no checked callee
        // expr; fall back to the declared signature so args still coerce to
        // the real param types instead of a boxing Any default.
        let callee_ty = self.get_type(callee.id).clone();
        let param_tys = if let Type::Fn(ptys, _, _) = callee_ty {
            ptys
        } else if let Some(Type::Fn(ptys, _, _)) = call_fn_name
            .as_deref()
            .and_then(|name| self.global_types.get(name))
        {
            ptys.clone()
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
        mut arg_ops: Vec<Operand>,
        arg_tys: Vec<Type>,
        span: Span,
        expr_id: usize,
    ) -> Operand {
        // Unbox Python scalars supplied for concrete native fields, and tag
        // scalars landing in scalar-union fields; generic (`Param`) fields
        // fall through `coerce` untouched.
        if let Some(field_names) = self.struct_fields.get(struct_name).cloned() {
            for (i, op) in arg_ops.iter_mut().enumerate() {
                let from_ty = arg_tys.get(i).cloned().unwrap_or(Type::Any);
                let Some(field_name) = field_names.get(i) else {
                    break;
                };
                if let Some(field_ty) = self
                    .struct_field_types
                    .get(&(struct_name.to_string(), field_name.clone()))
                    .cloned()
                    && (from_ty.is_py_value() || field_ty.is_tag_encoded_union())
                {
                    *op = self.coerce(op.clone(), &from_ty, &field_ty, span);
                }
            }
        }
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

        // `__init__`'s call result is a required assignment target, not a
        // meaningful value -- the constructed object is `obj_tmp`, returned
        // below. Unscoped like `obj_tmp` so it is never scope-dropped as if it
        // owned a second, aliased copy of the struct.
        let init_res = self.new_unscoped_local(self.get_type(expr_id));
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

    pub(super) fn has_native_module_fn(&self, name: &str, attr: &str) -> bool {
        let mangled = format!("{}::{}", name, attr);
        self.fn_meta.contains_key(&mangled)
            || self.lookup_var(&mangled).is_some()
            || self.globals.contains_key(&mangled)
    }
}
