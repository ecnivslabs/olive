use super::CraneliftCodegen;
use super::imports::cl_type;
use super::translate_rvalue::{emit_bounds_check, emit_nil_check, loc_value};
use crate::mir::{
    Constant, Local, MirFunction, Operand, Statement, StatementKind, Terminator, TerminatorKind,
};
use crate::semantic::types::Type as OliveType;
use crate::span::Span;
use cranelift::prelude::*;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(super) type FieldInfo<'a> = (i32, &'a str, Option<(u8, u8)>);

pub(super) fn c_struct_field_info<'a>(
    c_struct_offsets: &'a HashMap<String, Vec<super::FfiStructFieldLayout>>,
    struct_name: &str,
    attr: &str,
) -> Option<FieldInfo<'a>> {
    c_struct_offsets
        .get(struct_name)
        .and_then(|fields| fields.iter().find(|(n, _, _, _)| n == attr))
        .map(|(_, off, ty, bits)| (*off, ty.as_str(), *bits))
}

pub(super) fn free_func_name_for_type(
    ty: &OliveType,
    struct_fields: &HashMap<String, Vec<String>>,
) -> &'static str {
    match ty {
        OliveType::Str => "__olive_free_str",
        OliveType::Bytes => "__olive_buf_free",
        OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_) => "__olive_free_list",
        OliveType::Struct(name, _, _) if struct_fields.contains_key(name) => "__olive_free_struct",
        OliveType::Dict(_, _) | OliveType::Struct(_, _, _) => "__olive_free_obj",
        OliveType::Enum(_, _) => "__olive_free_enum",
        OliveType::Any => "__olive_free_any",
        // `T | None` stores the raw `T` with `None` as a zero sentinel (no
        // Any-box with kind tag).  `__olive_free_any` would misinterpret T's
        // header word as a kind tag; dispatch to T's concrete free function
        // instead. A multi-member union (no `Any` involved) is the same raw
        // representation, just with more than one possible non-null shape --
        // `__olive_free_union_member` gates on the allocator's live-object
        // table rather than `__olive_free_any`'s inline-tag bit heuristics,
        // which a plain scalar member's untagged bits can coincidentally
        // satisfy (see the runtime doc comment).
        OliveType::Union(members) => {
            let non_null: Vec<&OliveType> = members
                .iter()
                .filter(|m| !matches!(m, OliveType::Null))
                .collect();
            match non_null.as_slice() {
                [single] => free_func_name_for_type(single, struct_fields),
                _ if members.contains(&OliveType::Any) => "__olive_free_any",
                _ => "__olive_free_union_member",
            }
        }
        OliveType::PyObject | OliveType::PyNamed(_, _) => "__olive_py_decref",
        _ => "__olive_free",
    }
}

pub(super) fn truncate_for_store(
    builder: &mut FunctionBuilder,
    val: Value,
    ty_name: &str,
) -> Value {
    let cl_ty = super::ffi_cl_type(ty_name);
    let val_ty = builder.func.dfg.value_type(val);
    if val_ty == cl_ty {
        return val;
    }
    match cl_ty {
        t if t == types::I64 => val,
        t if t == types::F64 => {
            if val_ty == types::I64 {
                builder.ins().bitcast(types::F64, MemFlags::new(), val)
            } else {
                val
            }
        }
        t if t == types::F32 => {
            if val_ty == types::F64 {
                builder.ins().fdemote(types::F32, val)
            } else if val_ty == types::I64 {
                builder.ins().bitcast(types::F32, MemFlags::new(), val)
            } else {
                val
            }
        }
        _ => builder.ins().ireduce(cl_ty, val),
    }
}

pub(super) fn attr_symbol(
    builder: &mut FunctionBuilder,
    module: &mut impl Module,
    string_ids: &HashMap<String, DataId>,
    attr: &str,
) -> Value {
    if let Some(&id) = string_ids.get(attr) {
        let local_id = module.declare_data_in_func(id, builder.func);
        builder.ins().symbol_value(types::I64, local_id)
    } else {
        let c_str = std::ffi::CString::new(attr).unwrap();
        builder.ins().iconst(types::I64, c_str.into_raw() as i64)
    }
}

impl<M: Module> CraneliftCodegen<M> {
    pub(super) fn translate_function(&mut self, func: &MirFunction) {
        let mut ctx = self.module.make_context();

        for i in 0..func.arg_count {
            let ty = &func.locals[i + 1].ty;
            ctx.func.signature.params.push(AbiParam::new(cl_type(ty)));
        }
        let ret_ty = &func.locals[0].ty;
        ctx.func
            .signature
            .returns
            .push(AbiParam::new(cl_type(ret_ty)));

        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);

        let blocks: Vec<Block> = func
            .basic_blocks
            .iter()
            .map(|_| builder.create_block())
            .collect();
        let mut vars = HashMap::default();

        for (i, decl) in func.locals.iter().enumerate() {
            let var = builder.declare_var(cl_type(&decl.ty));
            vars.insert(Local(i), var);
        }

        let mut pred_count = vec![0u32; func.basic_blocks.len()];
        for bb in &func.basic_blocks {
            if let Some(term) = &bb.terminator {
                match &term.kind {
                    TerminatorKind::Goto { target } => {
                        pred_count[target.0] += 1;
                    }
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        for (_, t) in targets {
                            pred_count[t.0] += 1;
                        }
                        pred_count[otherwise.0] += 1;
                    }
                    _ => {}
                }
            }
        }
        let mut sealed = vec![false; func.basic_blocks.len()];
        let mut filled_pred = vec![0u32; func.basic_blocks.len()];

        for (i, bb) in func.basic_blocks.iter().enumerate() {
            builder.switch_to_block(blocks[i]);

            if i == 0 && !sealed[i] {
                builder.seal_block(blocks[i]);
                sealed[i] = true;
            }

            if i == 0 {
                builder.append_block_params_for_function_params(blocks[i]);
                let params: Vec<Value> = builder.block_params(blocks[i]).to_vec();

                for (j, val) in params.iter().enumerate() {
                    let var = vars.get(&Local(j + 1)).unwrap();
                    let var_ty = builder.func.dfg.value_type(*val);
                    let decl_ty = cl_type(&func.locals[j + 1].ty);
                    if var_ty != decl_ty {
                        panic!("param def_var type mismatch");
                    }
                    builder.def_var(*var, *val);
                }

                if let Some(&hotcount_id) = self.hotcount_ids.get(&func.name) {
                    let local_id = self.module.declare_data_in_func(hotcount_id, builder.func);
                    let ptr = builder.ins().symbol_value(types::I64, local_id);
                    let count = builder.ins().load(types::I64, MemFlags::trusted(), ptr, 0);
                    let next = builder.ins().iadd_imm(count, 1);
                    builder.ins().store(MemFlags::trusted(), next, ptr, 0);
                }
            }

            let mut s_idx = 0;
            let mut reuse_state: Option<(Local, Value, bool)> = None;
            while s_idx < bb.statements.len() {
                let stmt = &bb.statements[s_idx];
                if let StatementKind::Drop(local) = &stmt.kind {
                    let has_borrow = crate::mir::optimizations::ownership::REASSIGN_LIVE_BORROWS
                        .with(|map| {
                            map.borrow()
                                .get(&func.name)
                                .and_then(|m| m.get(local))
                                .copied()
                        });
                    if let Some(has_borrow) = has_borrow {
                        let mut next_assign_idx = None;
                        for next_j in (s_idx + 1)..bb.statements.len() {
                            let next_stmt = &bb.statements[next_j];
                            match &next_stmt.kind {
                                StatementKind::Assign(dst, _) if dst == local => {
                                    next_assign_idx = Some(next_j);
                                    break;
                                }
                                StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {
                                    continue;
                                }
                                _ => break,
                            }
                        }
                        if next_assign_idx.is_some() {
                            let var = vars.get(local).unwrap();
                            let val = builder.use_var(*var);
                            if let Some(desc_ty) = super::imports::drop_descriptor_type(
                                &func.locals[local.0].ty,
                                &self.struct_fields,
                            ) {
                                let desc = super::imports::type_descriptor(
                                    desc_ty,
                                    &self.struct_fields,
                                    &self.field_types,
                                    &self.enum_defs,
                                );
                                let data_id = *self
                                    .string_ids
                                    .get(&desc)
                                    .expect("drop descriptor not interned");
                                let local_data =
                                    self.module.declare_data_in_func(data_id, builder.func);
                                let desc_ptr = builder.ins().symbol_value(types::I64, local_data);
                                let clear_id = self.func_ids["__olive_clear_typed"];
                                let local_func =
                                    self.module.declare_func_in_func(clear_id, builder.func);
                                builder.ins().call(local_func, &[val, desc_ptr]);
                                reuse_state = Some((*local, val, has_borrow));
                                s_idx += 1;
                                continue;
                            }
                        }
                    }
                }
                let reuse_arg = if let StatementKind::Assign(local, _) = &stmt.kind
                    && let Some((reuse_local, reuse_val, has_borrow)) = reuse_state
                    && reuse_local == *local
                {
                    Some((reuse_local, reuse_val, has_borrow))
                } else {
                    None
                };
                Self::translate_statement(
                    func,
                    &mut self.module,
                    &self.func_ids,
                    &self.string_ids,
                    &self.struct_fields,
                    &self.field_types,
                    &self.enum_defs,
                    &self.c_struct_offsets,
                    &self.c_struct_names,
                    &self.c_struct_sizes,
                    &self.c_struct_destructors,
                    &self.ffi_vararg_ptrs,
                    &self.ffi_vararg_ids,
                    &self.ffi_entries,
                    &self.dispatch_ids,
                    &self.any_add_site_ids,
                    &mut self.any_add_site_cursor,
                    &self.specialize_sites,
                    &mut builder,
                    stmt,
                    &vars,
                    &self.loc_ids,
                    reuse_arg,
                );
                if reuse_arg.is_some() {
                    reuse_state = None;
                }
                s_idx += 1;
            }

            if let Some(term) = &bb.terminator {
                Self::translate_terminator(
                    &mut builder,
                    func,
                    term,
                    &blocks,
                    &vars,
                    &self.string_ids,
                    &mut self.module,
                    &self.func_ids,
                    &self.struct_fields,
                    func.is_async,
                );
                match &term.kind {
                    TerminatorKind::Goto { target } => {
                        filled_pred[target.0] += 1;
                        if filled_pred[target.0] == pred_count[target.0] && !sealed[target.0] {
                            builder.seal_block(blocks[target.0]);
                            sealed[target.0] = true;
                        }
                    }
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        for (_, t) in targets {
                            filled_pred[t.0] += 1;
                            if filled_pred[t.0] == pred_count[t.0] && !sealed[t.0] {
                                builder.seal_block(blocks[t.0]);
                                sealed[t.0] = true;
                            }
                        }
                        filled_pred[otherwise.0] += 1;
                        if filled_pred[otherwise.0] == pred_count[otherwise.0]
                            && !sealed[otherwise.0]
                        {
                            builder.seal_block(blocks[otherwise.0]);
                            sealed[otherwise.0] = true;
                        }
                    }
                    _ => {}
                }
            } else {
                let zero = builder.ins().iconst(types::I64, 0);
                builder.ins().return_(&[zero]);
            }
        }

        for (i, block) in blocks.iter().enumerate() {
            if !sealed[i] {
                builder.seal_block(*block);
            }
        }

        builder.finalize();

        let func_id = self
            .func_ids
            .get(&func.name)
            .expect("func not declared in func_ids");
        self.module.define_function(*func_id, &mut ctx).unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_statement(
        func_mir: &MirFunction,
        module: &mut M,
        func_ids: &HashMap<String, FuncId>,
        string_ids: &HashMap<String, DataId>,
        struct_fields: &HashMap<String, Vec<String>>,
        field_types: &HashMap<(String, String), crate::semantic::types::Type>,
        enum_defs: &HashMap<String, Vec<(String, Vec<crate::semantic::types::Type>)>>,
        c_struct_offsets: &HashMap<String, Vec<super::FfiStructFieldLayout>>,
        c_struct_names: &std::collections::HashSet<String>,
        c_struct_sizes: &HashMap<String, i64>,
        c_struct_destructors: &HashMap<String, String>,
        ffi_vararg_ptrs: &HashMap<String, *const u8>,
        ffi_vararg_ids: &std::collections::HashSet<String>,
        ffi_entries: &[super::FfiFnEntry],
        dispatch_ids: &HashMap<String, DataId>,
        any_add_site_ids: &[DataId],
        any_add_site_cursor: &mut usize,
        specialize_sites: &HashSet<usize>,
        builder: &mut FunctionBuilder,
        stmt: &Statement,
        vars: &HashMap<Local, Variable>,
        loc_ids: &HashMap<Span, DataId>,
        reuse_target: Option<(Local, Value, bool)>,
    ) {
        let loc_id = loc_ids.get(&stmt.span).copied();
        match &stmt.kind {
            StatementKind::Assign(local, rval) => {
                let val = Self::translate_rvalue(
                    func_mir,
                    module,
                    func_ids,
                    string_ids,
                    struct_fields,
                    field_types,
                    enum_defs,
                    c_struct_offsets,
                    c_struct_sizes,
                    ffi_vararg_ptrs,
                    ffi_vararg_ids,
                    ffi_entries,
                    dispatch_ids,
                    any_add_site_ids,
                    any_add_site_cursor,
                    specialize_sites,
                    builder,
                    rval,
                    vars,
                    loc_id,
                    reuse_target,
                    &func_mir.locals[local.0].ty,
                );
                let var = vars.get(local).unwrap();

                let val_ty = builder.func.dfg.value_type(val);
                let decl_ty = cl_type(&func_mir.locals[local.0].ty);
                let rval_is_pyobj = match rval {
                    crate::mir::ir::Rvalue::Use(
                        crate::mir::ir::Operand::Copy(l) | crate::mir::ir::Operand::Move(l),
                    ) => matches!(func_mir.locals[l.0].ty, OliveType::PyObject),
                    _ => false,
                };
                let rval_is_ptr_load = matches!(rval, crate::mir::ir::Rvalue::PtrLoad(_));
                let val = if val_ty != decl_ty {
                    if rval_is_ptr_load && val_ty == types::I64 && decl_ty == types::F32 {
                        let low = builder.ins().ireduce(types::I32, val);
                        builder.ins().bitcast(types::F32, MemFlags::new(), low)
                    } else if rval_is_pyobj && decl_ty == types::F64 {
                        let float_id = func_ids
                            .get("__olive_py_to_float")
                            .expect("missing __olive_py_to_float");
                        let local_func = module.declare_func_in_func(*float_id, builder.func);
                        let inst = builder.ins().call(local_func, &[val]);
                        builder.inst_results(inst)[0]
                    } else if rval_is_pyobj && decl_ty == types::F32 {
                        let float_id = func_ids
                            .get("__olive_py_to_float")
                            .expect("missing __olive_py_to_float");
                        let local_func = module.declare_func_in_func(*float_id, builder.func);
                        let inst = builder.ins().call(local_func, &[val]);
                        let f64_val = builder.inst_results(inst)[0];
                        builder.ins().fdemote(types::F32, f64_val)
                    } else if val_ty == types::F64 && decl_ty == types::F32 {
                        builder.ins().fdemote(types::F32, val)
                    } else if val_ty == types::F32 && decl_ty == types::F64 {
                        builder.ins().fpromote(types::F64, val)
                    } else if val_ty.bits() == decl_ty.bits() {
                        builder.ins().bitcast(decl_ty, MemFlags::new(), val)
                    } else if val_ty.is_int() && decl_ty.is_int() {
                        if val_ty.bits() < decl_ty.bits() {
                            let ty = &func_mir.locals[local.0].ty;
                            let is_signed = matches!(
                                ty,
                                crate::semantic::types::Type::Int
                                    | crate::semantic::types::Type::I32
                                    | crate::semantic::types::Type::I16
                                    | crate::semantic::types::Type::I8
                            );
                            if is_signed {
                                builder.ins().sextend(decl_ty, val)
                            } else {
                                builder.ins().uextend(decl_ty, val)
                            }
                        } else {
                            builder.ins().ireduce(decl_ty, val)
                        }
                    } else if val_ty.is_int() && decl_ty.is_float() {
                        builder.ins().fcvt_from_sint(decl_ty, val)
                    } else if val_ty.is_float() && decl_ty.is_int() {
                        builder.ins().fcvt_to_sint(decl_ty, val)
                    } else {
                        val
                    }
                } else {
                    val
                };

                // list_new always produces KIND_LIST (1) at runtime, but when
                // the destination is typed [Any] the elements will be boxed -
                // patch to KIND_ANY_LIST (15) so FFI proxies decode correctly.
                if matches!(
                    &func_mir.locals[local.0].ty,
                    OliveType::List(inner) if matches!(inner.as_ref(), OliveType::Any)
                ) && matches!(
                    rval,
                    crate::mir::ir::Rvalue::Call {
                        func: crate::mir::ir::Operand::Constant(
                            crate::mir::Constant::Function(name)
                        ),
                        ..
                    } if name == "__olive_list_new"
                ) {
                    let any_kind = builder.ins().iconst(types::I64, 15_i64);
                    builder.ins().store(MemFlags::trusted(), any_kind, val, 0);
                }

                builder.def_var(*var, val);
            }
            StatementKind::SetAttr(obj, attr, val_op) => {
                let obj_ty = if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    &func_mir.locals[loc.0].ty
                } else {
                    &OliveType::Any
                };
                let obj_ty = super::imports::concrete_ty(obj_ty);
                if let OliveType::Struct(struct_name, _, _) = obj_ty {
                    if let Some((offset, ty_name, bits)) =
                        c_struct_field_info(c_struct_offsets, struct_name, attr)
                    {
                        let o = Self::translate_operand(
                            builder, obj, vars, string_ids, module, func_ids,
                        );
                        let v = Self::translate_operand(
                            builder, val_op, vars, string_ids, module, func_ids,
                        );
                        if let Some((bit_off, bit_count)) = bits {
                            let word_ty = super::ffi_cl_type(ty_name);
                            let word = builder.ins().load(word_ty, MemFlags::trusted(), o, offset);
                            let mask = (1i64 << bit_count) - 1;
                            let positioned_mask = mask << bit_off;
                            let word_i64 = if word_ty == types::I64 {
                                word
                            } else {
                                builder.ins().uextend(types::I64, word)
                            };
                            let cleared = builder.ins().band_imm(word_i64, !positioned_mask);
                            let truncated = builder.ins().band_imm(v, mask);
                            let shifted = if bit_off > 0 {
                                builder.ins().ishl_imm(truncated, bit_off as i64)
                            } else {
                                truncated
                            };
                            let merged = builder.ins().bor(cleared, shifted);
                            let to_store = if word_ty == types::I64 {
                                merged
                            } else {
                                builder.ins().ireduce(word_ty, merged)
                            };
                            builder
                                .ins()
                                .store(MemFlags::trusted(), to_store, o, offset);
                        } else {
                            let v = truncate_for_store(builder, v, ty_name);
                            builder.ins().store(MemFlags::trusted(), v, o, offset);
                        }
                        return;
                    }
                    if let Some(fields) = struct_fields.get(struct_name.as_str())
                        && let Some(idx) = fields.iter().position(|f| f == attr)
                    {
                        let offset = 8 + (idx as i32) * 8;
                        let o = Self::translate_operand(
                            builder, obj, vars, string_ids, module, func_ids,
                        );
                        let v = Self::translate_operand(
                            builder, val_op, vars, string_ids, module, func_ids,
                        );
                        let v = if let Operand::Copy(src) = val_op
                            && func_mir.locals[src.0].ty.is_py_value()
                        {
                            let copy_ref_id = func_ids
                                .get("__olive_py_copy_ref")
                                .expect("missing __olive_py_copy_ref");
                            let local_func =
                                module.declare_func_in_func(*copy_ref_id, builder.func);
                            let inst = builder.ins().call(local_func, &[v]);
                            builder.inst_results(inst)[0]
                        } else if builder.func.dfg.value_type(v) == types::F64 {
                            builder.ins().bitcast(types::I64, MemFlags::new(), v)
                        } else {
                            v
                        };

                        // Overwriting an owning field must release whatever it held;
                        // otherwise every reassignment (e.g. a per-frame camera field) leaks.
                        // Struct storage is zero-initialized, so the first write decrefs a
                        // harmless null.
                        if let Some(field_ty) =
                            field_types.get(&(struct_name.clone(), attr.clone()))
                            && matches!(field_ty, OliveType::PyObject | OliveType::PyNamed(_, _))
                        {
                            let decref_id = func_ids
                                .get("__olive_py_decref")
                                .expect("missing __olive_py_decref");
                            let decref_func = module.declare_func_in_func(*decref_id, builder.func);
                            let old =
                                builder
                                    .ins()
                                    .load(types::I64, MemFlags::trusted(), o, offset);
                            builder.ins().call(decref_func, &[old]);
                        }

                        builder.ins().store(MemFlags::trusted(), v, o, offset);
                        return;
                    }
                }
                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let v =
                    Self::translate_operand(builder, val_op, vars, string_ids, module, func_ids);

                let attr_val = attr_symbol(builder, module, string_ids, attr);

                let v = if builder.func.dfg.value_type(v) == types::F64 {
                    builder.ins().bitcast(types::I64, MemFlags::new(), v)
                } else {
                    v
                };

                let obj_is_pyobj = if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    matches!(
                        super::imports::concrete_ty(&func_mir.locals[loc.0].ty),
                        OliveType::PyObject
                    )
                } else {
                    false
                };

                let func_name = if obj_is_pyobj {
                    "__olive_py_setattr"
                } else {
                    "__olive_obj_set"
                };
                let set_id = func_ids
                    .get(func_name)
                    .expect("missing obj_set or py_setattr");
                let local_func = module.declare_func_in_func(*set_id, builder.func);
                builder.ins().call(local_func, &[o, attr_val, v]);
            }
            StatementKind::SetIndex(obj, idx, val_op, unchecked) => {
                let unchecked = *unchecked;
                let ty = if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    &func_mir.locals[loc.0].ty
                } else {
                    &OliveType::Any
                };
                let ty = super::imports::concrete_ty(ty);

                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module, func_ids);
                let v =
                    Self::translate_operand(builder, val_op, vars, string_ids, module, func_ids);

                // Incref a borrowed PyObject; the container decrefs it on drop.
                let v = if let Operand::Copy(src) = val_op
                    && func_mir.locals[src.0].ty.is_py_value()
                {
                    let copy_ref_id = func_ids
                        .get("__olive_py_copy_ref")
                        .expect("missing __olive_py_copy_ref");
                    let local_func = module.declare_func_in_func(*copy_ref_id, builder.func);
                    let inst = builder.ins().call(local_func, &[v]);
                    builder.inst_results(inst)[0]
                } else if builder.func.dfg.value_type(v) == types::F64 {
                    builder.ins().bitcast(types::I64, MemFlags::new(), v)
                } else {
                    v
                };

                let loc = loc_value(builder, module, loc_id);

                match ty {
                    OliveType::Dict(k, _) if super::imports::needs_structural_key(k) => {
                        let desc = super::imports::type_descriptor(
                            k,
                            struct_fields,
                            field_types,
                            enum_defs,
                        );
                        let data_id = *string_ids
                            .get(&desc)
                            .expect("dict key descriptor not interned during collection");
                        let local_data = module.declare_data_in_func(data_id, builder.func);
                        let desc_ptr = builder.ins().symbol_value(types::I64, local_data);
                        let set_id = func_ids
                            .get("__olive_obj_set_typed")
                            .expect("missing __olive_obj_set_typed");
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v, desc_ptr]);
                    }
                    OliveType::Dict(_, _) | OliveType::Struct(_, _, _) | OliveType::PyObject => {
                        let set_id = func_ids
                            .get("__olive_obj_set")
                            .expect("missing __olive_obj_set");
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v]);
                    }
                    OliveType::Any => {
                        let set_id = func_ids
                            .get("__olive_set_index_any")
                            .expect("missing __olive_set_index_any");
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v, loc]);
                    }
                    OliveType::Enum(_, _) => {
                        let set_id = func_ids
                            .get("__olive_enum_set")
                            .expect("missing __olive_enum_set");
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v]);
                    }
                    OliveType::Bytes => {
                        emit_nil_check(builder, module, func_ids, o, loc);
                        let len = builder.ins().load(
                            types::I64,
                            MemFlags::trusted().with_readonly(),
                            o,
                            16,
                        );
                        let idx = if !unchecked {
                            emit_bounds_check(builder, module, func_ids, i, len, loc)
                        } else {
                            i
                        };
                        let data_ptr = builder.ins().load(
                            types::I64,
                            MemFlags::trusted().with_readonly(),
                            o,
                            8,
                        );
                        let addr = builder.ins().iadd(data_ptr, idx);
                        let byte = builder.ins().ireduce(types::I8, v);
                        builder.ins().store(MemFlags::trusted(), byte, addr, 0);
                    }
                    _ => {
                        emit_nil_check(builder, module, func_ids, o, loc);
                        let len = builder.ins().load(
                            types::I64,
                            MemFlags::trusted().with_readonly(),
                            o,
                            24,
                        );
                        let idx = if !unchecked {
                            emit_bounds_check(builder, module, func_ids, i, len, loc)
                        } else {
                            i
                        };
                        let data_ptr = builder.ins().load(
                            types::I64,
                            MemFlags::trusted().with_readonly(),
                            o,
                            8,
                        );
                        let offset = builder.ins().imul_imm(idx, 8);
                        let addr = builder.ins().iadd(data_ptr, offset);
                        builder.ins().store(MemFlags::trusted(), v, addr, 0);
                    }
                }
            }
            StatementKind::Drop(local) => {
                let ty = &func_mir.locals[local.0].ty;
                if !ty.is_move_type() {
                    return;
                }
                // A closure record's layout is per-instance, not per-type
                // (two closures sharing one `Type::Fn` signature can capture
                // different variables), so there's no static descriptor to
                // look up the way an ordinary struct/list/enum has -- the
                // descriptor was embedded in the record itself at
                // construction (`closures.rs::build_closure_value`) and is
                // loaded back here at runtime, then handed to the unmodified
                // struct free path.
                if let OliveType::Fn(..) = super::imports::concrete_ty(ty) {
                    let var = vars.get(local).unwrap();
                    let val = builder.use_var(*var);

                    let nonnull_bb = builder.create_block();
                    let done_bb = builder.create_block();
                    let nonnull = builder.ins().icmp_imm(IntCC::NotEqual, val, 0);
                    builder.ins().brif(nonnull, nonnull_bb, &[], done_bb, &[]);

                    builder.seal_block(nonnull_bb);
                    builder.switch_to_block(nonnull_bb);
                    // The record stores `__desc` via an ordinary `Constant::Str`
                    // (so the generic string-interning collector picks it up),
                    // which tags the low bit; strip it back to the raw
                    // descriptor pointer `olive_free_typed` expects.
                    let desc_tagged = builder.ins().load(types::I64, MemFlags::trusted(), val, 16);
                    let desc_ptr = builder.ins().band_imm(desc_tagged, -2);
                    let free_id = func_ids["__olive_free_typed"];
                    let local_func = module.declare_func_in_func(free_id, builder.func);
                    builder.ins().call(local_func, &[val, desc_ptr]);
                    builder.ins().jump(done_bb, &[]);

                    builder.seal_block(done_bb);
                    builder.switch_to_block(done_bb);
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.def_var(*var, zero);
                    return;
                }
                if let OliveType::Struct(name, _, _) = ty
                    && c_struct_names.contains(name.as_str())
                {
                    let var = vars.get(local).unwrap();
                    let val = builder.use_var(*var);

                    if let Some(dtor_name) = c_struct_destructors.get(name.as_str()) {
                        if let Some(&dtor_id) = func_ids.get(dtor_name.as_str()) {
                            let local_dtor = module.declare_func_in_func(dtor_id, builder.func);
                            builder.ins().call(local_dtor, &[val]);
                        }
                    } else {
                        let size = c_struct_sizes.get(name.as_str()).unwrap();
                        let size_val = builder.ins().iconst(types::I64, *size);
                        let free_id = func_ids
                            .get("__olive_free_c_struct")
                            .expect("missing __olive_free_c_struct");
                        let local_func = module.declare_func_in_func(*free_id, builder.func);
                        builder.ins().call(local_func, &[val, size_val]);
                    }

                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.def_var(*var, zero);
                    return;
                }

                let var = vars.get(local).unwrap();
                let val = builder.use_var(*var);

                // A trait object's own two/three words say nothing about the
                // concrete struct underneath (that's erased -- the point of
                // dynamic dispatch), so there is no static descriptor to look
                // up here. The fat pointer's third word is the drop shim
                // synthesized at the coercion site (`build_trait_drop_shim`),
                // which knows the real type and frees it correctly (including
                // any heap fields it owns) before the fat pointer block itself
                // is freed.
                if let OliveType::TraitObject(..) = ty {
                    let nonnull_bb = builder.create_block();
                    let done_bb = builder.create_block();
                    let nonnull = builder.ins().icmp_imm(IntCC::NotEqual, val, 0);
                    builder.ins().brif(nonnull, nonnull_bb, &[], done_bb, &[]);

                    builder.seal_block(nonnull_bb);
                    builder.switch_to_block(nonnull_bb);
                    let data_ptr = builder.ins().load(types::I64, MemFlags::trusted(), val, 0);
                    let shim_ptr = builder.ins().load(types::I64, MemFlags::trusted(), val, 16);
                    let mut sig = module.make_signature();
                    sig.call_conv = module.isa().default_call_conv();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let sig_ref = builder.import_signature(sig);
                    builder.ins().call_indirect(sig_ref, shim_ptr, &[data_ptr]);
                    let free_id = func_ids["__olive_free"];
                    let local_func = module.declare_func_in_func(free_id, builder.func);
                    builder.ins().call(local_func, &[val]);
                    builder.ins().jump(done_bb, &[]);

                    builder.seal_block(done_bb);
                    builder.switch_to_block(done_bb);
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.def_var(*var, zero);
                    return;
                }

                // A container type carries a descriptor so elements, fields,
                // and payloads are freed by their static types instead of
                // runtime guessing.
                if let Some(desc_ty) = super::imports::drop_descriptor_type(ty, struct_fields) {
                    let desc = super::imports::type_descriptor(
                        desc_ty,
                        struct_fields,
                        field_types,
                        enum_defs,
                    );
                    let data_id = *string_ids
                        .get(&desc)
                        .expect("drop descriptor not interned during collection");
                    let local_data = module.declare_data_in_func(data_id, builder.func);
                    let desc_ptr = builder.ins().symbol_value(types::I64, local_data);
                    let free_id = func_ids["__olive_free_typed"];
                    let local_func = module.declare_func_in_func(free_id, builder.func);
                    builder.ins().call(local_func, &[val, desc_ptr]);
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.def_var(*var, zero);
                    return;
                }

                let free_func_name = free_func_name_for_type(ty, struct_fields);

                let free_id = func_ids
                    .get(free_func_name)
                    .unwrap_or_else(|| panic!("missing runtime function: {}", free_func_name));
                let local_func = module.declare_func_in_func(*free_id, builder.func);
                builder.ins().call(local_func, &[val]);

                let zero = builder.ins().iconst(types::I64, 0);
                builder.def_var(*var, zero);
            }
            StatementKind::PtrStore(ptr_op, val_op) => {
                let ptr =
                    Self::translate_operand(builder, ptr_op, vars, string_ids, module, func_ids);
                let val =
                    Self::translate_operand(builder, val_op, vars, string_ids, module, func_ids);
                builder.ins().store(MemFlags::trusted(), val, ptr, 0);
            }
            StatementKind::GenCheck { value, generation } => {
                let v = builder.use_var(*vars.get(value).unwrap());
                let g = builder.use_var(*vars.get(generation).unwrap());

                let fail = builder.create_block();
                let ok = builder.create_block();
                builder.set_cold_block(fail);

                // Strings and structs use helpers instead of a raw read.
                if crate::mir::optimizations::gencheck::str_backed(&func_mir.locals[value.0].ty) {
                    let id = func_ids
                        .get("__olive_str_gen_stale")
                        .expect("missing __olive_str_gen_stale");
                    let local_func = module.declare_func_in_func(*id, builder.func);
                    let inst = builder.ins().call(local_func, &[v, g]);
                    let stale = builder.inst_results(inst)[0];
                    builder.ins().brif(stale, fail, &[], ok, &[]);
                } else if crate::mir::optimizations::gencheck::struct_backed(
                    &func_mir.locals[value.0].ty,
                ) {
                    let id = func_ids
                        .get("__olive_struct_gen_stale")
                        .expect("missing __olive_struct_gen_stale");
                    let local_func = module.declare_func_in_func(*id, builder.func);
                    let inst = builder.ins().call(local_func, &[v, g]);
                    let stale = builder.inst_results(inst)[0];
                    builder.ins().brif(stale, fail, &[], ok, &[]);
                } else {
                    let probe = builder.create_block();
                    // Null carries no generation; later null handling reports it.
                    let nonnull = builder.ins().icmp_imm(IntCC::NotEqual, v, 0);
                    builder.ins().brif(nonnull, probe, &[], ok, &[]);

                    builder.seal_block(probe);
                    builder.switch_to_block(probe);
                    let cur = builder.ins().load(types::I64, MemFlags::trusted(), v, -8);
                    // Shift drops the shared bit; bit 0 must still read live.
                    let diff = builder.ins().bxor(cur, g);
                    let diff = builder.ins().ishl_imm(diff, 1);
                    let dead = builder.ins().band_imm(cur, 1);
                    let dead = builder.ins().bxor_imm(dead, 1);
                    let stale = builder.ins().bor(diff, dead);
                    builder.ins().brif(stale, fail, &[], ok, &[]);
                }

                builder.seal_block(fail);
                builder.switch_to_block(fail);
                let name = func_mir.locals[value.0]
                    .name
                    .as_deref()
                    .and_then(|n| string_ids.get(n))
                    .map(|id| {
                        let local = module.declare_data_in_func(*id, builder.func);
                        let ptr = builder.ins().symbol_value(types::I64, local);
                        builder.ins().bor_imm(ptr, 1)
                    })
                    .unwrap_or_else(|| builder.ins().iconst(types::I64, 0));
                let loc = loc_value(builder, module, loc_id);
                let fail_id = func_ids
                    .get("__olive_stale_ref_fail")
                    .expect("missing __olive_stale_ref_fail");
                let local_func = module.declare_func_in_func(*fail_id, builder.func);
                builder.ins().call(local_func, &[name, loc]);
                builder.ins().trap(TrapCode::unwrap_user(1));

                builder.seal_block(ok);
                builder.switch_to_block(ok);
            }
            StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {}
            StatementKind::VectorStore(obj, idx, val_op) => {
                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module, func_ids);
                let v =
                    Self::translate_operand(builder, val_op, vars, string_ids, module, func_ids);

                let data_ptr =
                    builder
                        .ins()
                        .load(types::I64, MemFlags::trusted().with_readonly(), o, 8);
                let offset = builder.ins().imul_imm(i, 8);
                let addr = builder.ins().iadd(data_ptr, offset);
                builder.ins().store(MemFlags::trusted(), v, addr, 0);
            }
        }
    }

    pub(super) fn translate_operand(
        builder: &mut FunctionBuilder,
        op: &Operand,
        vars: &HashMap<Local, Variable>,
        string_ids: &HashMap<String, DataId>,
        module: &mut M,
        func_ids: &HashMap<String, FuncId>,
    ) -> Value {
        match op {
            Operand::Copy(l) | Operand::Move(l) => {
                let var = vars.get(l).expect("variable not found");
                let val = builder.use_var(*var);
                if matches!(op, Operand::Move(_)) {
                    let var_ty = builder.func.dfg.value_type(val);
                    let zero = super::imports::typed_zero(builder, var_ty);
                    builder.def_var(*var, zero);
                }
                val
            }
            Operand::Constant(c) => match c {
                Constant::Int(i) => builder.ins().iconst(types::I64, *i),
                Constant::Float(f) => builder.ins().f64const(f64::from_bits(*f)),
                Constant::Bool(b) => {
                    let val = if *b { 1 } else { 0 };
                    builder.ins().iconst(types::I64, val)
                }
                Constant::Str(s) => {
                    let id = *string_ids.get(s).unwrap_or_else(|| {
                        panic!("string constant not found: {:?}", s);
                    });
                    let local_id = module.declare_data_in_func(id, builder.func);
                    let ptr = builder.ins().symbol_value(types::I64, local_id);
                    builder.ins().bor_imm(ptr, 1)
                }
                Constant::Function(name) => {
                    if let Some(&func_id) = func_ids.get(name) {
                        let local_ref = module.declare_func_in_func(func_id, builder.func);
                        builder.ins().func_addr(types::I64, local_ref)
                    } else if name.starts_with("__olive_") {
                        panic!(
                            "internal error: address taken of unregistered runtime function \
                             `{name}`. This is a compiler bug."
                        );
                    } else {
                        builder.ins().iconst(types::I64, 0)
                    }
                }
                Constant::GlobalData(name) => {
                    let id = module
                        .declare_data(name, cranelift_module::Linkage::Export, true, false)
                        .unwrap();
                    let local_id = module.declare_data_in_func(id, builder.func);
                    builder.ins().symbol_value(types::I64, local_id)
                }
                _ => builder.ins().iconst(types::I64, 0),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_terminator(
        builder: &mut FunctionBuilder,
        func_mir: &MirFunction,
        term: &Terminator,
        blocks: &[Block],
        vars: &HashMap<Local, Variable>,
        string_ids: &HashMap<String, DataId>,
        module: &mut M,
        func_ids: &HashMap<String, FuncId>,
        _struct_fields: &HashMap<String, Vec<String>>,
        is_async: bool,
    ) {
        match &term.kind {
            TerminatorKind::Goto { target } => {
                builder.ins().jump(blocks[target.0], &[]);
            }
            TerminatorKind::SwitchInt {
                discr,
                targets,
                otherwise,
            } => {
                let val =
                    Self::translate_operand(builder, discr, vars, string_ids, module, func_ids);
                let is_pyobj = matches!(discr,
                    Operand::Copy(loc) | Operand::Move(loc)
                    if matches!(func_mir.locals[loc.0].ty, OliveType::PyObject)
                );
                let cond_val = if is_pyobj {
                    let to_int_id = func_ids
                        .get("__olive_py_to_int")
                        .expect("missing __olive_py_to_int");
                    let local_func = module.declare_func_in_func(*to_int_id, builder.func);
                    let inst = builder.ins().call(local_func, &[val]);
                    builder.inst_results(inst)[0]
                } else {
                    val
                };
                if is_pyobj && targets.len() == 1 {
                    let target_val = targets[0].0;
                    let target_block = blocks[targets[0].1.0];
                    let else_block = blocks[otherwise.0];
                    let cond = if target_val == 0 {
                        builder.ins().icmp_imm(IntCC::Equal, cond_val, 0)
                    } else {
                        builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, cond_val, target_val)
                    };
                    builder.ins().brif(cond, target_block, &[], else_block, &[]);
                } else {
                    let mut switch = cranelift::frontend::Switch::new();
                    for (v, target_bb) in targets {
                        switch.set_entry(*v as u128, blocks[target_bb.0]);
                    }
                    switch.emit(builder, cond_val, blocks[otherwise.0]);
                }
            }
            TerminatorKind::Return => {
                let var = vars.get(&Local(0)).unwrap();
                let ret_val = builder.use_var(*var);
                if is_async {
                    let make_future_id = func_ids
                        .get("__olive_make_future")
                        .expect("missing __olive_make_future");
                    let local_func = module.declare_func_in_func(*make_future_id, builder.func);
                    let call = builder.ins().call(local_func, &[ret_val]);
                    let future_val = builder.inst_results(call)[0];
                    builder.ins().return_(&[future_val]);
                } else {
                    builder.ins().return_(&[ret_val]);
                }
            }
            TerminatorKind::Unreachable => {
                builder.ins().trap(TrapCode::unwrap_user(1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::{call_i64, call_i64_1, call_i64_2, compile};

    #[test]
    fn test_translate_simple_return() {
        let mut cg = compile("fn f() -> i64:\n    return 99\n");
        assert_eq!(call_i64(&mut cg, "f"), 99);
    }

    #[test]
    fn test_translate_multi_block() {
        let mut cg =
            compile("fn f(x: i64) -> i64:\n    if x > 0:\n        return x\n    return 0 - x\n");
        assert_eq!(call_i64_1(&mut cg, "f", 5), 5);
        assert_eq!(call_i64_1(&mut cg, "f", -3), 3);
    }

    #[test]
    fn test_translate_switch_int() {
        let mut cg = compile(
            "fn f(x: i64) -> i64:\n    if x == 0:\n        return 10\n    elif x == 1:\n        return 20\n    return 30\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 0), 10);
        assert_eq!(call_i64_1(&mut cg, "f", 1), 20);
        assert_eq!(call_i64_1(&mut cg, "f", 2), 30);
    }

    #[test]
    fn test_translate_local_vars() {
        let mut cg = compile("fn f(a: i64, b: i64) -> i64:\n    let c = a + b\n    return c\n");
        assert_eq!(call_i64_2(&mut cg, "f", 10, 32), 42);
    }

    #[test]
    fn test_translate_nested_blocks() {
        let mut cg = compile(
            "fn f(x: i64) -> i64:\n    let mut r = 0\n    let mut i = 0\n    while i < x:\n        r = r + i\n        i = i + 1\n    return r\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 5), 10);
    }
}
