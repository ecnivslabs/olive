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
use rustc_hash::FxHashMap as HashMap;

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
            }

            for stmt in &bb.statements {
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
                    &mut builder,
                    stmt,
                    &vars,
                    &self.loc_ids,
                );
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
        builder: &mut FunctionBuilder,
        stmt: &Statement,
        vars: &HashMap<Local, Variable>,
        loc_ids: &HashMap<Span, DataId>,
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
                    builder,
                    rval,
                    vars,
                    loc_id,
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

                builder.def_var(*var, val);
            }
            StatementKind::SetAttr(obj, attr, val_op) => {
                let mut obj_ty = if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    &func_mir.locals[loc.0].ty
                } else {
                    &OliveType::Any
                };
                while let OliveType::Ref(inner) | OliveType::MutRef(inner) = obj_ty {
                    obj_ty = inner;
                }
                if let OliveType::Struct(struct_name, _) = obj_ty {
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
                            && matches!(func_mir.locals[src.0].ty, OliveType::PyObject)
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
                    let mut oty = &func_mir.locals[loc.0].ty;
                    while let OliveType::Ref(inner) | OliveType::MutRef(inner) = oty {
                        oty = inner;
                    }
                    matches!(oty, OliveType::PyObject)
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
                let mut ty = if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    &func_mir.locals[loc.0].ty
                } else {
                    &OliveType::Any
                };
                while let OliveType::Ref(inner) | OliveType::MutRef(inner) = ty {
                    ty = inner;
                }

                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module, func_ids);
                let v =
                    Self::translate_operand(builder, val_op, vars, string_ids, module, func_ids);

                let v = if builder.func.dfg.value_type(v) == types::F64 {
                    builder.ins().bitcast(types::I64, MemFlags::new(), v)
                } else {
                    v
                };

                let loc = loc_value(builder, module, loc_id);

                match ty {
                    OliveType::Dict(_, _) | OliveType::Struct(_, _) | OliveType::PyObject => {
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
                        if !unchecked {
                            emit_bounds_check(builder, module, func_ids, i, len, loc);
                        }
                        let data_ptr = builder.ins().load(
                            types::I64,
                            MemFlags::trusted().with_readonly(),
                            o,
                            8,
                        );
                        let addr = builder.ins().iadd(data_ptr, i);
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
                        if !unchecked {
                            emit_bounds_check(builder, module, func_ids, i, len, loc);
                        }
                        let data_ptr = builder.ins().load(
                            types::I64,
                            MemFlags::trusted().with_readonly(),
                            o,
                            8,
                        );
                        let offset = builder.ins().imul_imm(i, 8);
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
                if let OliveType::Struct(name, _) = ty
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

                let free_func_name = match ty {
                    OliveType::Str => "__olive_free_str",
                    OliveType::Bytes => "__olive_buf_free",
                    OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_) => {
                        "__olive_free_list"
                    }
                    OliveType::Struct(name, _) if struct_fields.contains_key(name) => {
                        "__olive_free_struct"
                    }
                    OliveType::Dict(_, _) | OliveType::Struct(_, _) => "__olive_free_obj",
                    OliveType::Enum(_, _) => "__olive_free_enum",
                    OliveType::Any => "__olive_free_any",
                    OliveType::Union(_) => "__olive_free_any",
                    OliveType::PyObject => "__olive_py_decref",
                    _ => "__olive_free",
                };

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
                    let zero = if var_ty == types::F64 {
                        builder.ins().f64const(0.0)
                    } else if var_ty == types::F32 {
                        builder.ins().f32const(0.0)
                    } else if var_ty.is_int() {
                        builder.ins().iconst(var_ty, 0)
                    } else if var_ty.is_vector() {
                        let lane = var_ty.lane_type();
                        let scalar = if lane == types::F64 {
                            builder.ins().f64const(0.0)
                        } else if lane == types::F32 {
                            builder.ins().f32const(0.0)
                        } else {
                            builder.ins().iconst(lane, 0)
                        };
                        builder.ins().splat(var_ty, scalar)
                    } else {
                        builder.ins().iconst(types::I64, 0)
                    };
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
