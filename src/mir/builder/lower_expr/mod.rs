mod call;
mod call_method;
mod control;
mod data;
mod literals;
mod ops;
mod py_call;
mod py_call_kw_arity;
mod sort_key;

use super::{MirBuilder, NestedFnInfo};
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{CallArg, Expr, ExprKind, Stmt, StmtKind};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    /// E6.2: if `ty` is a struct defining `__str__`, calls it and returns
    /// the resulting `str` operand -- shared by `print`, `str()`, and
    /// f-string interpolation, all three of which fall back to the
    /// descriptor-driven auto-repr when the struct has none.
    pub(super) fn lower_struct_str_call(
        &mut self,
        op: Operand,
        ty: &Type,
        span: Span,
    ) -> Option<Operand> {
        let (struct_name, type_args) = Self::deref_struct_ty(ty.clone())?;
        let base = format!("{struct_name}::__str__");
        if !self.fn_meta.contains_key(&base) {
            return None;
        }
        Some(self.call_struct_dunder(
            &struct_name,
            &type_args,
            "__str__",
            vec![op],
            Type::Str,
            span,
        ))
    }

    /// Boxes a scalar into an `Any` container slot so a read sees a
    /// self-describing value.
    pub(super) fn coerce_to_elem(&mut self, op: Operand, elem: &Expr, elem_ty: &Type) -> Operand {
        // A trait-object element holds a fat pointer (data + vtable), so a
        // concrete struct stored into one must be widened the same way an
        // argument or assignment is.
        if let Type::TraitObject(_, _) = elem_ty {
            let from_ty = self.get_type(elem.id);
            return self.coerce(op, &from_ty, elem_ty, elem.span);
        }
        let from_ty = self.get_type(elem.id);
        if from_ty.is_py_value() && !matches!(elem_ty, Type::Any | Type::PyObject) {
            return self.coerce(op, &from_ty, elem_ty, elem.span);
        }
        if elem_ty.is_scalar_nullable_union() {
            return self.coerce(op, &from_ty, elem_ty, elem.span);
        }
        if *elem_ty != Type::Any {
            return op;
        }
        self.box_into_any(op, &from_ty, elem.span)
    }

    /// Coerces a dict key or set element into an `Any` slot. Unlike a value
    /// slot, these are hashed and compared by their raw word, and store and
    /// lookup take separate paths, so an integer stays bare to hash identically
    /// on both sides. `null` still boxes since a bare `0` key is reserved as the
    /// runtime's "absent" sentinel.
    pub(super) fn coerce_to_hashable(
        &mut self,
        op: Operand,
        elem: &Expr,
        elem_ty: &Type,
    ) -> Operand {
        let from_ty = self.get_type(elem.id);
        if from_ty.is_py_value() && !matches!(elem_ty, Type::Any | Type::PyObject) {
            return self.coerce(op, &from_ty, elem_ty, elem.span);
        }
        if *elem_ty != Type::Any {
            return op;
        }
        if from_ty == Type::Null {
            return self.box_into_any(op, &from_ty, elem.span);
        }
        op
    }

    /// Boxes a scalar (int, float, bool, or null) into its self-describing `Any`
    /// heap form; passes pointers and aggregates through unchanged.
    pub(super) fn box_into_any(&mut self, op: Operand, from_ty: &Type, span: Span) -> Operand {
        // A raw MIR constant is never pre-boxed, whatever its inferred static
        // type says: unification can widen a bare literal's own type to
        // `Any` to satisfy a container element elsewhere (e.g. `list_new`'s
        // element var), without the literal itself ever passing through a
        // boxing step. Dispatch on the constant's own kind in that case
        // instead of trusting a from_ty that would otherwise look like a
        // no-op "already Any" value and skip boxing entirely.
        let from_ty = match (&op, from_ty) {
            (Operand::Constant(Constant::Int(_)), Type::Any) => &Type::Int,
            (Operand::Constant(Constant::Float(_)), Type::Any) => &Type::Float,
            (Operand::Constant(Constant::Bool(_)), Type::Any) => &Type::Bool,
            (Operand::Constant(Constant::None), Type::Any) => &Type::Null,
            _ => from_ty,
        };
        let boxer = match from_ty {
            Type::Int
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize => Some(("__olive_box_int", true)),
            Type::Float | Type::F32 => Some(("__olive_box_float", true)),
            Type::Bool => Some(("__olive_box_bool", true)),
            Type::Null => Some(("__olive_box_null", false)),
            // A Python value put into an `Any` slot is materialized to a real
            // Olive value, so string methods and arithmetic on it work without
            // an explicit cast at the read site.
            Type::PyObject | Type::PyNamed(_, _) => Some(("__olive_py_to_any", true)),
            _ => return op,
        };
        let (boxer, takes_arg) = boxer.unwrap();
        let tmp = self.new_local(Type::Any, None, false);
        let args = if takes_arg { vec![op] } else { vec![] };
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(boxer.to_string())),
                    args,
                },
            ),
            span,
        );
        Operand::Copy(tmp)
    }

    pub(super) fn coerce(
        &mut self,
        op: Operand,
        from_ty: &Type,
        to_ty: &Type,
        span: Span,
    ) -> Operand {
        // Struct → Trait|None coercion must go through the trait fat pointer, not the union.
        if let (Type::Struct(_, _, _), Type::Union(members)) = (from_ty, to_ty) {
            let traits: Vec<&Type> = members
                .iter()
                .filter(|m| matches!(m, Type::TraitObject(_, _)))
                .collect();
            if let [trait_member] = traits.as_slice() {
                return self.coerce(op, from_ty, trait_member, span);
            }
        }

        if let Type::TraitObject(trait_name, _) = to_ty
            && let Type::Struct(struct_name, _, _) = from_ty
        {
            let vtable_name = format!("__vtable_{}_{}", trait_name, struct_name);
            if !self.vtables.contains_key(&vtable_name)
                && let Some(trait_def) = self.traits.get(trait_name)
            {
                let mut method_names = Vec::new();
                for (method_name, _) in &trait_def.methods {
                    let mangled = format!("{}::{}", struct_name, method_name);
                    if let Type::Struct(_, type_args, _) = from_ty
                        && !type_args.is_empty()
                    {
                        method_names.push(self.monomorphize(&mangled, type_args));
                        continue;
                    }
                    method_names.push(mangled);
                }
                self.vtables.insert(vtable_name.clone(), method_names);
            }
            let vtable_op = Operand::Constant(Constant::GlobalData(vtable_name.clone()));
            let (struct_name, type_args, is_ffi) = match from_ty {
                Type::Struct(n, a, f) => (n.clone(), a.clone(), *f),
                _ => unreachable!("guarded above"),
            };
            let drop_shim_name = self.build_trait_drop_shim(&struct_name, &type_args, is_ffi);
            let drop_shim_op = Operand::Constant(Constant::Function(drop_shim_name));
            let fat_ptr_tmp = self.new_local(to_ty.clone(), None, false);
            self.push_statement(
                StatementKind::Assign(
                    fat_ptr_tmp,
                    Rvalue::Aggregate(AggregateKind::FatPtr, vec![op, vtable_op, drop_shim_op]),
                ),
                span,
            );
            return Operand::Copy(fat_ptr_tmp);
        }

        // Olive value -> PyObject: assigning a native or `Any` value into a
        // `PyObject` converts it to a real Python object (`1` -> a Python int),
        // the inverse of reading a `PyObject` into a native slot.
        if matches!(to_ty, Type::PyObject) && !from_ty.is_py_value() {
            // R19: a function value (`let cb: PyObject = my_fn`, `return
            // my_fn`, a struct field typed `PyObject`, ...) exports as a
            // real `PyCFunction` instead of the scalar/str/Any conversions
            // below -- it needs two call args (the record and its packed
            // tags), not the uniform one-arg shape those share.
            if let Type::Fn(params, ret, _) = from_ty {
                return self.emit_fn_to_py_callable(op, params, ret, span);
            }
            let conv = match from_ty {
                Type::Int
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Usize
                | Type::Bool => Some("__olive_py_from_int"),
                Type::Float | Type::F32 => Some("__olive_py_from_float"),
                Type::Str => Some("__olive_py_from_str"),
                Type::Bytes | Type::Any => Some("__olive_to_pyobject"),
                _ => None,
            };
            if let Some(conv) = conv {
                let tmp = self.new_local(Type::PyObject, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(conv.to_string())),
                            args: vec![op],
                        },
                    ),
                    span,
                );
                return Operand::Copy(tmp);
            }
        }

        // A Python value landing in a native-typed slot must realize into that
        // type's own representation: a raw handle under a native static type
        // reads as garbage on any typed access (indexing, drops, formatting).
        if from_ty.is_py_value()
            && let Some((target, nullable)) = py_realize_target(to_ty)
        {
            // A mixed union source (e.g. `PyObject | Error`) may hold a
            // native member at runtime; realize only actual handles.
            if union_mixes_py(from_ty) {
                return self.realize_py_guarded(op, &target, to_ty, nullable, span);
            }
            if nullable {
                return self.realize_py_nullable(op, &target, to_ty, span);
            }
            return self.realize_py_value(op, &target, span);
        }

        // A scalar widening into `Any` is boxed so the slot stays
        // self-describing: a bare word can't distinguish an int from a float's
        // bits, `null` from `0`, or a large odd int from a tagged string.
        if *to_ty == Type::Any {
            return self.box_into_any(op, from_ty, span);
        }

        // Inverse of the widening above: narrowing back to a concrete scalar.
        if *from_ty == Type::Any
            && let Some(unboxed) = self.unbox_from_any(op.clone(), to_ty, span)
        {
            return unboxed;
        }

        // Scalar unions use the Any tag encoding: a raw word cannot tell a
        // real zero from None. Box on entry, unbox on narrowing. An Any or
        // same-encoded union source is already tagged and passes through.
        if to_ty.is_scalar_nullable_union() && !matches!(from_ty, Type::Union(_) | Type::Any) {
            return self.box_into_any(op, from_ty, span);
        }
        if from_ty.is_scalar_nullable_union()
            && let Some(unboxed) = self.unbox_from_any(op.clone(), to_ty, span)
        {
            return unboxed;
        }

        op
    }

    /// Converts a Python value into the Olive representation of `target`.
    fn realize_py_value(&mut self, op: Operand, target: &Type, span: Span) -> Operand {
        match target {
            Type::Str => {
                let tmp = self.new_unscoped_local(Type::Str);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_py_to_str".to_string(),
                            )),
                            args: vec![op],
                        },
                    ),
                    span,
                );
                Operand::Move(tmp)
            }
            // Python bytes-like values materialize into a native buffer; else raises at call site.
            Type::Bytes => {
                self.emit_py_set_loc(span);
                let tmp = self.new_unscoped_local(Type::Bytes);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_py_to_bytes".to_string(),
                            )),
                            args: vec![op],
                        },
                    ),
                    span,
                );
                Operand::Move(tmp)
            }
            // Element-wise: each member keeps its own declared type, so a
            // `PyNamed` member stays a handle while natives convert.
            Type::Tuple(members) => {
                self.emit_py_set_loc(span);
                let mut elems = Vec::with_capacity(members.len());
                for (i, member) in members.iter().enumerate() {
                    let item = self.new_unscoped_local(Type::PyObject);
                    self.push_statement(
                        StatementKind::Assign(
                            item,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_py_getitem_int".to_string(),
                                )),
                                args: vec![op.clone(), Operand::Constant(Constant::Int(i as i64))],
                            },
                        ),
                        span,
                    );
                    let coerced = self.coerce(Operand::Copy(item), &Type::PyObject, member, span);
                    elems.push(match coerced {
                        Operand::Copy(l) => Operand::Move(l),
                        other => other,
                    });
                }
                let tup = self.new_unscoped_local(target.clone());
                self.push_statement(
                    StatementKind::Assign(tup, Rvalue::Aggregate(AggregateKind::Tuple, elems)),
                    span,
                );
                Operand::Move(tup)
            }
            // `[Any]` needs boxed elements (a raw native word collides with
            // the Any inline tag bits); a concrete native element type wants
            // the raw form, matching that type's own runtime representation.
            Type::List(elem) => {
                let tmp = self.new_unscoped_local(target.clone());
                if elem.as_ref() == &Type::Any {
                    self.push_statement(
                        StatementKind::Assign(
                            tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_py_to_any_list".to_string(),
                                )),
                                args: vec![op],
                            },
                        ),
                        span,
                    );
                } else {
                    // R14: the buffer ingest fast path needs to know the
                    // declared element type up front -- must match
                    // `python_buffer.rs`'s `BUF_ELEM_INT`/`BUF_ELEM_FLOAT`.
                    let elem_tag = match elem.as_ref() {
                        Type::Int => 1,
                        Type::Float => 2,
                        _ => 0,
                    };
                    self.push_statement(
                        StatementKind::Assign(
                            tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_py_to_list".to_string(),
                                )),
                                args: vec![op, Operand::Constant(Constant::Int(elem_tag))],
                            },
                        ),
                        span,
                    );
                }
                Operand::Move(tmp)
            }
            Type::Dict(_, val) => {
                let func = if val.as_ref() == &Type::Any {
                    "__olive_py_to_any_dict"
                } else {
                    "__olive_py_to_dict"
                };
                let tmp = self.new_unscoped_local(target.clone());
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(func.to_string())),
                            args: vec![op],
                        },
                    ),
                    span,
                );
                Operand::Move(tmp)
            }
            _ => {
                self.emit_set_fault_loc(span);
                let tmp = self.new_local(target.clone(), None, false);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::Cast(op, target.clone())),
                    span,
                );
                Operand::Move(tmp)
            }
        }
    }

    /// Mixed py/native union source: branch on a live-handle test so only
    /// actual Python values realize; native members pass through raw.
    fn realize_py_guarded(
        &mut self,
        op: Operand,
        target: &Type,
        to_ty: &Type,
        nullable: bool,
        span: Span,
    ) -> Operand {
        let result = self.new_unscoped_local(to_ty.clone());
        let is_handle = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                is_handle,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_py_is_handle".to_string())),
                    args: vec![op.clone()],
                },
            ),
            span,
        );
        let py_bb = self.new_block();
        let pass_bb = self.new_block();
        let merge_bb = self.new_block();
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::SwitchInt {
                    discr: Operand::Copy(is_handle),
                    targets: vec![(1, py_bb)],
                    otherwise: pass_bb,
                },
                span,
            );
        }
        self.current_block = Some(py_bb);
        let converted = if nullable {
            self.realize_py_nullable(op.clone(), target, to_ty, span)
        } else {
            self.realize_py_value(op.clone(), target, span)
        };
        self.push_statement(StatementKind::Assign(result, Rvalue::Use(converted)), span);
        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: merge_bb }, span);
        }
        self.current_block = Some(pass_bb);
        self.push_statement(StatementKind::Assign(result, Rvalue::Use(op)), span);
        self.terminate_block(pass_bb, TerminatorKind::Goto { target: merge_bb }, span);
        self.current_block = Some(merge_bb);
        Operand::Move(result)
    }

    /// `T | None` target: a Python `None` realizes to the 0 sentinel,
    /// anything else to `T`'s own representation.
    fn realize_py_nullable(
        &mut self,
        op: Operand,
        target: &Type,
        union_ty: &Type,
        span: Span,
    ) -> Operand {
        let result = self.new_unscoped_local(union_ty.clone());
        let is_none = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                is_none,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_py_is_none".to_string())),
                    args: vec![op.clone()],
                },
            ),
            span,
        );
        let none_bb = self.new_block();
        let conv_bb = self.new_block();
        let merge_bb = self.new_block();
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::SwitchInt {
                    discr: Operand::Copy(is_none),
                    targets: vec![(1, none_bb)],
                    otherwise: conv_bb,
                },
                span,
            );
        }
        self.current_block = Some(none_bb);
        self.push_statement(
            StatementKind::Assign(result, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            span,
        );
        self.terminate_block(none_bb, TerminatorKind::Goto { target: merge_bb }, span);
        self.current_block = Some(conv_bb);
        let converted = self.realize_py_value(op, target, span);
        self.push_statement(StatementKind::Assign(result, Rvalue::Use(converted)), span);
        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: merge_bb }, span);
        }
        self.current_block = Some(merge_bb);
        Operand::Move(result)
    }

    /// Synthesizes (or reuses) `<Struct>::__drop_shim(v: Struct) -> None`, a
    /// one-statement function that just drops its owning param. A trait
    /// object's own free (`Drop` on a `TraitObject` local) only knows the
    /// fat pointer's own two/three words, not the concrete struct's field
    /// layout underneath -- the concrete type is erased, that's the point
    /// of dynamic dispatch. This shim's address, stored as the fat
    /// pointer's third word, is how `Drop` on the trait object frees the
    /// real struct (and any heap fields it owns) correctly regardless of
    /// which concrete type was boxed.
    fn build_trait_drop_shim(
        &mut self,
        struct_name: &str,
        type_args: &[Type],
        is_ffi: bool,
    ) -> String {
        let base = format!("{struct_name}::__drop_shim");
        let shim_name = if type_args.is_empty() {
            base
        } else {
            self.monomorphize(&base, type_args)
        };

        let saved_name = std::mem::take(&mut self.current_name);
        let saved_locals = std::mem::take(&mut self.current_locals);
        let saved_blocks = std::mem::take(&mut self.current_blocks);
        let saved_block = self.current_block.take();
        let saved_var_map = std::mem::take(&mut self.var_map);
        let saved_loop_stack = std::mem::take(&mut self.loop_stack);
        let saved_scope_locals = std::mem::take(&mut self.scope_locals);
        let saved_arg_count = self.current_arg_count;
        let saved_is_async = self.current_is_async;
        self.current_is_async = false;

        self.start_function(shim_name.clone(), 1, Type::Null);
        let param_ty = Type::Struct(struct_name.to_string(), type_args.to_vec(), is_ffi);
        let param_local = self.declare_var("v".to_string(), param_ty, false);
        self.push_statement(StatementKind::Drop(param_local), Span::default());
        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Return, Span::default());
        }
        self.current_block = Some(self.new_block());
        self.finish_function();

        self.current_name = saved_name;
        self.current_locals = saved_locals;
        self.current_blocks = saved_blocks;
        self.current_block = saved_block;
        self.var_map = saved_var_map;
        self.loop_stack = saved_loop_stack;
        self.scope_locals = saved_scope_locals;
        self.current_arg_count = saved_arg_count;
        self.current_is_async = saved_is_async;

        shim_name
    }

    /// Unboxes an `Any` into a concrete scalar local; returns `None` for
    /// non-scalar targets (pointers stay as-is).
    pub(super) fn unbox_from_any(
        &mut self,
        op: Operand,
        to_ty: &Type,
        span: Span,
    ) -> Option<Operand> {
        let unboxer = match to_ty {
            Type::Int
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize
            | Type::Bool => "__olive_unbox_int",
            Type::Float | Type::F32 => "__olive_unbox_float",
            _ => return None,
        };
        let tmp = self.new_local(to_ty.clone(), None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(unboxer.to_string())),
                    args: vec![op],
                },
            ),
            span,
        );
        Some(Operand::Copy(tmp))
    }

    pub(super) fn lower_expr(&mut self, expr: &Expr) -> Operand {
        match &expr.kind {
            // The checker unifies an int literal against a `float`-expected
            // position (`let x: float = 5`); the literal must then lower to
            // the float's own bit pattern, not the int's bits reinterpreted.
            ExprKind::Integer(i) if matches!(self.get_type(expr.id), Type::Float | Type::F32) => {
                Operand::Constant(Constant::Float((*i as f64).to_bits()))
            }
            ExprKind::Integer(i) => Operand::Constant(Constant::Int(*i)),
            ExprKind::Null => Operand::Constant(Constant::None),
            ExprKind::Range {
                start,
                end,
                inclusive,
                step,
            } => {
                let start_op = self.lower_expr_as_copy(start);
                let end_op = self.lower_expr_as_copy(end);
                let step_op = match step {
                    Some(step_expr) => {
                        let raw = self.lower_expr_as_copy(step_expr);
                        let checked = self.new_local(Type::Int, None, false);
                        self.push_statement(
                            StatementKind::Assign(
                                checked,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_check_nonzero_step".to_string(),
                                    )),
                                    args: vec![raw, self.index_loc_operand(step_expr.span)],
                                },
                            ),
                            step_expr.span,
                        );
                        Operand::Copy(checked)
                    }
                    None => Operand::Constant(Constant::Int(1)),
                };
                let tmp = self.new_local(Type::List(Box::new(Type::Int)), None, true);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_range_list".to_string(),
                            )),
                            args: vec![
                                start_op,
                                end_op,
                                Operand::Constant(Constant::Int(*inclusive as i64)),
                                step_op,
                            ],
                        },
                    ),
                    expr.span,
                );
                Operand::Copy(tmp)
            }
            ExprKind::Float(f) => Operand::Constant(Constant::Float((*f).to_bits())),
            ExprKind::Str(s) => Operand::Constant(Constant::Str(s.clone())),
            ExprKind::FStr(parts) => self.lower_fstr_expr(parts, expr.span),
            ExprKind::Bool(b) => Operand::Constant(Constant::Bool(*b)),
            ExprKind::Try(inner) => self.lower_try_expr(inner, expr.span, expr.id),
            ExprKind::Await(inner) => self.lower_await_expr(inner, expr.id),
            ExprKind::AsyncBlock(body) => self.lower_async_block_expr(body, expr.span),
            ExprKind::Deref(inner) => self.lower_deref_expr(inner, expr.id),
            ExprKind::Borrow(inner) => self.lower_borrow_expr(inner, expr.id, expr.span),
            ExprKind::Starred(_) => unreachable!(
                "Starred only appears inside an Assign/MultiLet target, lowered by lower_stmt"
            ),
            ExprKind::MutBorrow(inner) => self.lower_mut_borrow_expr(inner, expr.id, expr.span),
            ExprKind::Identifier(name) => self.lower_identifier_expr(name, expr.id),
            ExprKind::BinOp { left, op, right } => {
                self.lower_binop_expr(left, op, right, expr.span, expr.id)
            }
            ExprKind::UnaryOp { op, operand } => {
                self.lower_unary_op_expr(op, operand, expr.span, expr.id)
            }
            ExprKind::Cast(operand, _ty) => self.lower_cast_expr(operand, expr.span, expr.id),
            ExprKind::Call { callee, args } => self.lower_call_expr(callee, args, expr),
            ExprKind::List(elems) | ExprKind::Tuple(elems) => {
                let is_tuple = matches!(expr.kind, ExprKind::Tuple(_));
                self.lower_list_or_tuple_expr(elems, is_tuple, expr.span, expr.id)
            }
            ExprKind::Set(elems) => self.lower_set_expr(elems, expr.span, expr.id),
            ExprKind::Dict(pairs) => self.lower_dict_expr(pairs, expr.span, expr.id),
            ExprKind::Attr { obj, attr } => self.lower_attr_expr(obj, attr, expr.span, expr.id),
            ExprKind::OptAttr { obj, attr } => {
                self.lower_opt_attr_expr(obj, attr, expr.span, expr.id)
            }
            ExprKind::Index { obj, index } => self.lower_index_expr(obj, index, expr.span, expr.id),
            ExprKind::Slice { .. } => {
                panic!(
                    "standalone slice expression not supported at expr.span={:?}",
                    expr.span
                );
            }
            ExprKind::ListComp { elt, clauses } => {
                self.lower_list_comp_expr(elt, clauses, expr.span, expr.id)
            }
            ExprKind::SetComp { elt, clauses } => {
                self.lower_set_comp_expr(elt, clauses, expr.span, expr.id)
            }
            ExprKind::DictComp {
                key,
                value,
                clauses,
            } => self.lower_dict_comp_expr(key, value, clauses, expr.span, expr.id),
            ExprKind::Match {
                expr: match_expr,
                cases,
            } => self.lower_match_expr(match_expr, cases, expr.span, expr.id),
            ExprKind::Ternary {
                cond,
                then,
                otherwise,
            } => self.lower_ternary_expr(cond, then, otherwise, expr.span, expr.id),

            ExprKind::Lambda {
                params: l_params,
                body: l_body,
            } => {
                // A lambda literal reached here is used as a plain value
                // (returned, stored, passed as an argument) -- the IIFE and
                // direct-call-through-a-bound-name shapes are intercepted
                // earlier in `lower_call_expr`. Captures resolved against
                // the still-live scope before `lower_lambda_expr` lowers
                // the body and swaps it away. Always builds the uniform
                // closure record, capturing or not -- see the nested-fn
                // case in `lower_identifier_expr` for why.
                let raw_captures =
                    crate::semantic::free_vars::free_variables_expr(l_params, l_body);
                let func_op = self.lower_lambda_expr(l_params, l_body, expr.id, expr.span);
                let Operand::Constant(Constant::Function(mangled)) = &func_op else {
                    unreachable!("lower_lambda_expr always returns Constant::Function")
                };
                let checked_param_tys = match self.get_type(expr.id) {
                    Type::Fn(p, _, _) => p,
                    _ => Vec::new(),
                };
                let param_tys = l_params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        p.type_ann
                            .as_ref()
                            .map(|ann| self.resolve_type_expr(ann))
                            .or_else(|| checked_param_tys.get(i).cloned())
                            .unwrap_or(Type::Any)
                    })
                    .collect();
                let info = NestedFnInfo {
                    mangled: mangled.clone(),
                    raw_captures,
                    param_tys,
                };
                self.build_closure_value(&info, expr.id, expr.span)
            }
        }
    }

    pub(super) fn lower_call_expr(
        &mut self,
        callee: &Expr,
        args: &[CallArg],
        expr: &Expr,
    ) -> Operand {
        let callee_name = if let ExprKind::Identifier(name) = &callee.kind {
            Some(name.as_str())
        } else {
            None
        };

        let (mut arg_ops, mut arg_kw_names, mut arg_tys) =
            self.lower_call_args(args, callee, expr.span);

        if let Some(kwarg_map) = self.expr_kwarg_maps.get(&expr.id) {
            let mut new_arg_ops = vec![Operand::Constant(Constant::Int(0)); kwarg_map.len()];
            let mut new_arg_tys = vec![Type::Any; kwarg_map.len()];
            for (i, op) in arg_ops.iter().enumerate() {
                if let Some(target_idx) = kwarg_map.iter().position(|&x| x == i) {
                    new_arg_ops[target_idx] = op.clone();
                    if let Some(ty) = arg_tys.get(i) {
                        new_arg_tys[target_idx] = ty.clone();
                    }
                }
            }
            arg_ops = new_arg_ops;
            arg_tys = new_arg_tys;
            arg_kw_names = vec![None; arg_ops.len()];
        }

        let callee_ty = self.get_type(callee.id);

        if let ExprKind::Attr { obj, attr } = &callee.kind
            && let ExprKind::Identifier(name) = &obj.kind
            && self.has_native_module_fn(name, attr)
        {
            return self.lower_attr_method_call_section(
                callee,
                obj,
                attr,
                args,
                arg_ops,
                arg_kw_names,
                arg_tys,
                expr.span,
                expr.id,
            );
        }

        // `obj.attr(...)` where `obj` itself is a raw Python value: route
        // through the method-call fusion path before ever lowering `attr`
        // as a value. The generic `callee_ty.is_py_value()` branch just
        // below would otherwise win first almost every time (a bound
        // attribute of a dynamic Python object types as `PyObject`/`Any`
        // too, same as the callee itself), lowering `callee` as a plain
        // value first -- which unconditionally emits a separate getattr --
        // before this method call ever gets a chance to fuse it away.
        // `PyObject_VectorcallMethod`'s own semantics (getattr the first
        // arg, then call the result with the rest) make this correct
        // whether `obj` is a class instance or a plain module: Python's
        // descriptor protocol binds (or doesn't) at the getattr step
        // either way.
        if let ExprKind::Attr { obj, attr } = &callee.kind
            && self.get_type(obj.id).is_py_value()
        {
            return self.lower_attr_method_call_section(
                callee,
                obj,
                attr,
                args,
                arg_ops,
                arg_kw_names,
                arg_tys,
                expr.span,
                expr.id,
            );
        }

        if callee_ty.is_py_value() {
            let callee_op = self.lower_expr_as_copy(callee);
            let py_result = self.lower_pyobject_call(
                callee_op,
                args,
                arg_ops,
                arg_kw_names,
                expr.span,
                expr.id,
            );
            return self.coerce_pyobj_if_needed(py_result, expr.id, expr.span);
        }

        if let Some(name) = callee_name {
            let base = name.rsplit("::").next().unwrap_or(name);
            if matches!(base, "panic" | "unwrap" | "unwrap_err") {
                self.emit_set_fault_loc(expr.span);
            }

            if name == "print" {
                return self
                    .lower_print_builtin(callee, args, &arg_ops, &arg_tys, expr.span, expr.id);
            }

            // E6.2: `str(p)` on a struct defining `__str__` calls it
            // directly; everything else (including a struct without one)
            // falls through to the normal `str` builtin dispatch below.
            if name == "str"
                && arg_ops.len() == 1
                && let Some(op) =
                    self.lower_struct_str_call(arg_ops[0].clone(), &arg_tys[0].clone(), expr.span)
            {
                return op;
            }

            // `str(x)` on a `u64` must name `__olive_str_u64` here, at
            // build time: codegen's generic dispatch re-derives the arg's
            // type from the MIR operand, and a release-only constant fold
            // can replace a `Copy(local)` with a bare `Constant::Int` that
            // carries no u64 tag, silently falling back to signed `str`.
            if name == "str" && arg_tys.first() == Some(&Type::U64) {
                let tmp = self.new_local(Type::Str, None, true);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_str_u64".to_string(),
                            )),
                            args: vec![arg_ops[0].clone()],
                        },
                    ),
                    expr.span,
                );
                return self.operand_for_local(tmp);
            }

            if name == "type"
                && !args.is_empty()
                && let Some(op) = self.lower_type_builtin(args, expr.span)
            {
                return op;
            }

            if name == "len"
                && !args.is_empty()
                && let Some(op) = self.lower_len_builtin(args, expr.span)
            {
                return op;
            }

            if (name == "max" || name == "min")
                && args.len() == 2
                && let Some(op) = self.lower_maxmin_builtin(name, args, expr.span, expr.id)
            {
                return op;
            }

            if let Some(op) = self.lower_sequence_builtin(name, args, expr.span, expr.id) {
                return op;
            }

            if let Some(op) =
                self.lower_enum_variant_call(name, arg_ops.clone(), expr.span, expr.id)
            {
                return op;
            }

            if name == "list_new"
                && !args.is_empty()
                && let Some(op) = self.lower_list_new_builtin(args, expr.span, expr.id)
            {
                return op;
            }

            if let Some(op) = self.lower_bytes_builtin(name, args, expr.span) {
                return op;
            }
        }

        if let ExprKind::Attr { obj, attr } = &callee.kind {
            return self.lower_attr_method_call_section(
                callee,
                obj,
                attr,
                args,
                arg_ops,
                arg_kw_names,
                arg_tys,
                expr.span,
                expr.id,
            );
        }

        if let Type::Struct(struct_name, type_args, _) = callee_ty {
            return self.lower_struct_construct_call(
                &struct_name,
                &type_args,
                arg_ops,
                arg_tys,
                expr.span,
                expr.id,
            );
        }

        if let Some(name) = callee_name
            && self.lookup_var(name).is_none()
            && let Some(info) = self.lookup_nested_fn(name)
        {
            return self.lower_nested_fn_call(
                &info,
                &arg_ops,
                &arg_tys,
                &arg_kw_names,
                expr.span,
                expr.id,
            );
        }

        // A name bound directly to a lambda (`let g = lambda: ...`) is
        // always a local, so it never reaches the `lookup_nested_fn` branch
        // above (that lookup intentionally defers to a same-named local).
        if let Some(name) = callee_name
            && let Some(info) = self.lookup_bound_lambda(name)
        {
            return self.lower_nested_fn_call(
                &info,
                &arg_ops,
                &arg_tys,
                &arg_kw_names,
                expr.span,
                expr.id,
            );
        }

        // Immediately-invoked lambda (`(lambda x: x + n)(5)`): captures
        // resolve against this scope directly, no name needed.
        if let ExprKind::Lambda {
            params: l_params,
            body: l_body,
        } = &callee.kind
        {
            // Checked types read before `lower_lambda_expr` swaps scope, same
            // reasoning as `collect_bound_lambdas`: an unannotated param's
            // real type is the checker's inference, not a blind `Any`.
            let checked_param_tys = match self.get_type(callee.id) {
                Type::Fn(p, _, _) => p,
                _ => Vec::new(),
            };
            let func_op = self.lower_lambda_expr(l_params, l_body, callee.id, callee.span);
            let Operand::Constant(Constant::Function(mangled)) = &func_op else {
                unreachable!("lower_lambda_expr always returns Constant::Function")
            };
            let param_tys = l_params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    p.type_ann
                        .as_ref()
                        .map(|ann| self.resolve_type_expr(ann))
                        .or_else(|| checked_param_tys.get(i).cloned())
                        .unwrap_or(Type::Any)
                })
                .collect();
            let info = NestedFnInfo {
                mangled: mangled.clone(),
                raw_captures: crate::semantic::free_vars::free_variables_expr(l_params, l_body),
                param_tys,
            };
            return self.lower_nested_fn_call(
                &info,
                &arg_ops,
                &arg_tys,
                &arg_kw_names,
                expr.span,
                expr.id,
            );
        }

        // A direct call to a plain global fn name (not a local, not a nested
        // fn/bound lambda -- those are all handled above): the callee is
        // never lowered as a value here, or every ordinary top-level call
        // would pay for a wasted closure record only to have
        // `lower_general_call_path` re-derive this exact same name anyway.
        // Must match that function's own `call_fn_name` derivation exactly.
        // A name found in `self.globals` instead (a module-scope `let`,
        // possibly holding a lambda/closure value) is never a real function
        // symbol under its own bare name -- fall through to `lower_expr`,
        // which resolves it the same way any other read of that global does.
        // `f[int](..)`/`mod.f[int](..)`: an explicit generic type argument
        // wraps the real callee in an `Index`. The bracketed type has
        // already done its job during type-checking (pinning the checked
        // type this call resolves to); by codegen all that's left is the
        // same bare/module-qualified function name `lower_general_call_path`
        // would derive from the unwrapped shape, which is what lets its
        // existing monomorphize step below key off `self.generic_fns`
        // correctly instead of falling through to `lower_expr` and losing
        // the callee's identity as a function reference entirely.
        let index_inner = match &callee.kind {
            ExprKind::Index { obj, .. } => Some(obj.as_ref()),
            _ => None,
        };
        let name_callee = index_inner.unwrap_or(callee);
        let func = if let ExprKind::Identifier(name) = &name_callee.kind
            && self.lookup_var(name).is_none()
            && !self.globals.contains_key(name)
        {
            Operand::Constant(Constant::Function(name.clone()))
        } else if let ExprKind::Attr { obj, attr } = &name_callee.kind
            && let ExprKind::Identifier(obj_name) = &obj.kind
            && self.lookup_var(obj_name).is_none()
        {
            Operand::Constant(Constant::Function(format!("{obj_name}::{attr}")))
        } else {
            self.lower_expr(callee)
        };
        self.lower_general_call_path(
            callee,
            func,
            arg_ops,
            arg_kw_names,
            arg_tys,
            expr.span,
            expr.id,
        )
    }

    pub(super) fn lower_expr_as_copy(&mut self, expr: &Expr) -> Operand {
        let op = self.lower_expr(expr);
        match op {
            Operand::Move(l) => Operand::Copy(l),
            _ => op,
        }
    }

    pub(super) fn coerce_pyobj_if_needed(
        &mut self,
        op: Operand,
        expr_id: usize,
        span: crate::span::Span,
    ) -> Operand {
        let declared_ty = self.get_type(expr_id);
        // Already the declared type -- an R10-fused scalar result, say --
        // so there is nothing left to convert; inserting a Cast here would
        // be a pure no-op the codegen has to notice and elide on its own.
        if let Operand::Copy(l) | Operand::Move(l) = &op
            && self.current_locals[l.0].ty == declared_ty
        {
            return op;
        }
        // The general `coerce`'s `PyObject -> native` realize branch already
        // covers every target `py_realize_target` names (str, bytes, tuple,
        // list, dict, every numeric width, bool) through the one shared,
        // fault-location-tracked path -- delegate instead of re-special-
        // casing only the numeric subset here and silently no-oping on the
        // rest, which left a fused-less `str`/`bytes`/`list`/`dict`/`tuple`
        // result crossing this branch as a raw, unconverted handle.
        self.coerce(op, &Type::PyObject, &declared_ty, span)
    }

    /// Converts a value crossing into Python by its static type. A raw word
    /// is ambiguous at the boundary (a large odd int reads as a tagged str
    /// pointer), so scalars must become real Python objects here.
    pub(super) fn emit_to_py_arg(
        &mut self,
        op: Operand,
        ty: &Type,
        span: crate::span::Span,
    ) -> Operand {
        if ty.is_py_value() {
            return op;
        }
        let resolved = match ty {
            Type::IntegerLiteral(_) => &Type::Int,
            Type::FloatLiteral(_) => &Type::Float,
            other => other,
        };
        self.coerce(op, resolved, &Type::PyObject, span)
    }

    pub(super) fn is_int_ty(ty: &Type) -> bool {
        matches!(
            ty,
            Type::Int
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Usize
        )
    }

    pub(super) fn enum_type_id(enum_name: &str) -> i64 {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        enum_name.hash(&mut h);
        (h.finish() & 0x7FFF_FFFF_FFFF_FFFF) as i64
    }

    pub(super) fn struct_type_id(struct_name: &str) -> i64 {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        struct_name.hash(&mut h);
        (h.finish() & 0x7FFF_FFFF_FFFF_FFFF) as i64
    }

    /// The `file:line:col` string for `span`, shared by every call-site
    /// location mechanism: the legacy `emit_py_set_loc` statement pair and
    /// the R17 fast-path entry points, which take it as a plain trailing
    /// call argument instead.
    pub(super) fn call_loc_str(&self, span: Span) -> String {
        match self.file_names.get(&span.file_id) {
            Some(file) => format!("{}:{}:{}", file, span.line, span.col),
            None => format!("{}:{}", span.line, span.col),
        }
    }

    /// Records the Olive call site just before a Python call so an uncaught
    /// Python exception can be reported against the exact source line.
    pub(super) fn emit_py_set_loc(&mut self, span: Span) {
        let loc = self.call_loc_str(span);
        let loc_local = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                loc_local,
                Rvalue::Use(Operand::Constant(Constant::Str(loc))),
            ),
            span,
        );
        let sink = self.new_local(Type::Null, None, false);
        self.push_statement(
            StatementKind::Assign(
                sink,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_py_set_loc".to_string())),
                    args: vec![Operand::Copy(loc_local)],
                },
            ),
            span,
        );
    }

    /// Records the user's call site just before a fault-prone prelude call
    /// (`panic`, `unwrap`, `unwrap_err`) so the abort points at the caller, not
    /// the one-line library wrapper. Mirrors Rust's `#[track_caller]`.
    pub(super) fn emit_set_fault_loc(&mut self, span: Span) {
        let loc = match self.file_names.get(&span.file_id) {
            Some(file) => format!("{}:{}:{}", file, span.line, span.col),
            None => format!("{}:{}", span.line, span.col),
        };
        let loc_local = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                loc_local,
                Rvalue::Use(Operand::Constant(Constant::Str(loc))),
            ),
            span,
        );
        let sink = self.new_local(Type::Null, None, false);
        self.push_statement(
            StatementKind::Assign(
                sink,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(
                        "__olive_set_fault_loc".to_string(),
                    )),
                    args: vec![Operand::Copy(loc_local)],
                },
            ),
            span,
        );
    }

    /// Prepends `<file>:<line>:<col>: ` to a runtime error-message string local,
    /// so a `try`-caught Python exception carries the same call-site prefix as an
    /// uncaught one.
    pub(super) fn prepend_call_loc(&mut self, msg_local: Local, span: Span) -> Local {
        let loc = match self.file_names.get(&span.file_id) {
            Some(file) => format!("{}:{}:{}: ", file, span.line, span.col),
            None => format!("{}:{}: ", span.line, span.col),
        };
        let loc_local = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                loc_local,
                Rvalue::Use(Operand::Constant(Constant::Str(loc))),
            ),
            span,
        );
        let out = self.new_local(Type::Str, None, false);
        self.push_statement(
            StatementKind::Assign(
                out,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_str_concat".to_string())),
                    args: vec![Operand::Copy(loc_local), Operand::Copy(msg_local)],
                },
            ),
            span,
        );
        out
    }

    pub(super) fn lower_lambda_expr(
        &mut self,
        params: &[crate::parser::Param],
        body: &Expr,
        expr_id: usize,
        _span: Span,
    ) -> Operand {
        // Named by the lambda expr's own node id rather than a running
        // counter, so a caller can compute the same mangled name up front
        // (`collect_bound_lambdas`) without lowering the lambda first.
        let lambda_name = format!("{}$lambda_{}", self.current_name, expr_id);

        // Captures resolved against the still-live enclosing scope before
        // `start_function` clears it, exactly like a named nested fn.
        let raw_captures = crate::semantic::free_vars::free_variables_expr(params, body);
        let captures = self.resolve_captures(&raw_captures);

        let saved_name = std::mem::take(&mut self.current_name);
        let saved_locals = std::mem::take(&mut self.current_locals);
        let saved_blocks = std::mem::take(&mut self.current_blocks);
        let saved_block = self.current_block.take();
        let saved_var_map = std::mem::take(&mut self.var_map);
        let saved_loop_stack = std::mem::take(&mut self.loop_stack);
        let saved_scope_locals = std::mem::take(&mut self.scope_locals);
        let saved_arg_count = self.current_arg_count;
        let saved_is_async = self.current_is_async;

        // An unannotated param's real type came from the checker's
        // inference (call-site hint or body usage, see `check_expr`'s
        // `Lambda` arm); defaulting straight to `Any` here would silently
        // re-box an already-concrete value and corrupt it (a `Var` that
        // resolved to e.g. `Int` must stay `Int`, not fall back to `Any`).
        let (checked_param_tys, ret_ty) = match self.get_type(expr_id) {
            Type::Fn(p, ret, _) => (p, *ret),
            _ => (Vec::new(), Type::Any),
        };
        self.start_function(lambda_name.clone(), params.len(), ret_ty);

        for (i, p) in params.iter().enumerate() {
            let p_ty = p
                .type_ann
                .as_ref()
                .map(|ann| self.resolve_type_expr(ann))
                .or_else(|| checked_param_tys.get(i).cloned())
                .unwrap_or(Type::Any);
            let local = self.declare_var(p.name.clone(), p_ty, p.is_mut);
            self.current_locals[local.0].is_owning = false;
        }

        // Captures are trailing params aliasing the caller's value (copied
        // in); the lambda never owns or drops them.
        for cap in &captures {
            let local = self.declare_var(cap.name.clone(), cap.ty.clone(), false);
            self.current_locals[local.0].is_owning = false;
        }
        self.current_arg_count += captures.len();

        let return_stmt = Stmt::new(StmtKind::Return(Some(body.clone())), body.span);
        self.lower_stmt(&return_stmt);
        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Return, Span::default());
        }

        self.finish_function();

        self.current_name = saved_name;
        self.current_locals = saved_locals;
        self.current_blocks = saved_blocks;
        self.current_block = saved_block;
        self.var_map = saved_var_map;
        self.loop_stack = saved_loop_stack;
        self.scope_locals = saved_scope_locals;
        self.current_arg_count = saved_arg_count;
        self.current_is_async = saved_is_async;

        Operand::Constant(Constant::Function(lambda_name))
    }
}

/// Whether a union carries both Python-typed and native members, so its
/// runtime value needs a handle test before any realize.
fn union_mixes_py(ty: &Type) -> bool {
    matches!(ty, Type::Union(members)
        if members.iter().any(|m| m.is_py_value())
            && members
                .iter()
                .any(|m| !m.is_py_value() && !matches!(m, Type::Null)))
}

/// The single Olive-native type a Python value can realize into for `to_ty`,
/// plus whether the target union also admits `None` (0 sentinel). `Error`ish
/// members only arise from Olive's own error paths, never a Python payload,
/// so a `T | Error` target realizes to `T`.
fn py_realize_target(to_ty: &Type) -> Option<(Type, bool)> {
    let realizable = |t: &Type| {
        matches!(
            t,
            Type::Str
                | Type::Bytes
                | Type::Tuple(_)
                | Type::List(_)
                | Type::Dict(_, _)
                | Type::Float
                | Type::F32
                | Type::Int
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Usize
                | Type::Bool
        )
    };
    match to_ty {
        Type::Union(members) => {
            let is_error = |t: &Type| {
                matches!(t, Type::Struct(n, _, _) | Type::Enum(n, _)
                    if n == "Error" || n.ends_with("Error"))
            };
            let nullable = members.iter().any(|m| matches!(m, Type::Null));
            let candidates: Vec<&Type> = members
                .iter()
                .filter(|m| !matches!(m, Type::Null) && !m.is_py_value() && !is_error(m))
                .collect();
            match candidates.as_slice() {
                [single] if realizable(single) => Some(((*single).clone(), nullable)),
                _ => None,
            }
        }
        t if realizable(t) => Some((t.clone(), false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::MirBuilder;
    use crate::lexer::Lexer;
    use crate::mir::ir::{Constant, Operand, Rvalue, StatementKind};
    use crate::parser::Parser;
    use crate::semantic::{Resolver, TypeChecker};
    use rustc_hash::FxHashSet;

    fn build(src: &str) -> Vec<super::super::super::ir::MirFunction> {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        let mut tc = TypeChecker::new();
        tc.check_program(&prog);
        let mut builder = MirBuilder::new(
            &tc.expr_types,
            &tc.expr_kwarg_maps,
            &tc.type_env[0],
            tc.struct_fields.clone(),
            &tc.traits,
            FxHashSet::default(),
            tc.enum_defs.clone(),
        );
        builder.build_program(&prog);
        builder.functions
    }

    #[test]
    fn integer_literal_produces_constant() {
        let fns = build("fn f() -> i64:\n    return 42\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_const = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Int(42))))
                )
            })
        });
        assert!(has_const);
    }

    #[test]
    fn bool_literal_produces_constant() {
        let fns = build("fn f() -> bool:\n    return true\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_const = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Bool(_))))
                )
            })
        });
        assert!(has_const, "expected a Bool constant in MIR");
    }

    #[test]
    fn binary_op_produces_binop_rvalue() {
        let fns = build("fn f() -> i64:\n    return 1 + 2\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_binop = f.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(&s.kind, StatementKind::Assign(_, Rvalue::BinaryOp(_, _, _))))
        });
        assert!(has_binop);
    }

    #[test]
    fn unary_op_produces_unaryop_rvalue() {
        let fns = build("fn f(x: i64) -> i64:\n    return -x\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_unary = f.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(&s.kind, StatementKind::Assign(_, Rvalue::UnaryOp(_, _))))
        });
        assert!(has_unary);
    }

    #[test]
    fn function_call_produces_call_rvalue() {
        let fns = build("fn g() -> i64:\n    return 1\n\nfn f() -> i64:\n    return g()\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_call = f.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(&s.kind, StatementKind::Assign(_, Rvalue::Call { .. })))
        });
        assert!(has_call);
    }

    #[test]
    fn string_literal_produces_constant() {
        let fns = build("fn f() -> str:\n    return \"hello\"\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_str = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(&s.kind, StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Str(h)))) if h == "hello")
            })
        });
        assert!(has_str);
    }

    #[test]
    fn cast_produces_cast_rvalue() {
        let fns = build("fn f(x: i64) -> float:\n    return x as float\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_cast = f.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(&s.kind, StatementKind::Assign(_, Rvalue::Cast(_, _))))
        });
        assert!(has_cast);
    }

    // A method call that omits a trailing default must coerce each positional
    // arg against its own param, not against a later one. Skipping more than the
    // receiver `self` aligned the `int` arg to the omitted `PyObject` default,
    // coercing it through `py_from_int` into a Python object the callee then read
    // as a native int.
    #[test]
    fn method_omitting_trailing_default_keeps_arg_alignment() {
        let src = "import py \"math\" as math\n\nstruct S:\n    n: int\n\nimpl S:\n    fn pick(self, count: int, ctx: PyObject = math) -> int:\n        count\n\nfn f() -> int:\n    let s = S(0)\n    return s.pick(7)\n";
        let fns = build(src);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let coerces_arg = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, Rvalue::Call { func, .. })
                        if matches!(func, Operand::Constant(Constant::Function(name)) if name == "__olive_py_from_int")
                )
            })
        });
        assert!(
            !coerces_arg,
            "int arg was misaligned onto the omitted PyObject default"
        );
    }
}
