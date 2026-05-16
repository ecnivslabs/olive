use super::CraneliftCodegen;
use super::imports::cl_type;
use crate::mir::{
    Constant, Local, MirFunction, Operand, Statement, StatementKind, Terminator, TerminatorKind,
};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::FxHashMap as HashMap;

pub(super) fn c_struct_field_info<'a>(
    c_struct_offsets: &'a HashMap<String, Vec<(String, i32, String, Option<(u8, u8)>)>>,
    struct_name: &str,
    attr: &str,
) -> Option<(i32, &'a str, Option<(u8, u8)>)> {
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
                );
            }

            if let Some(term) = &bb.terminator {
                Self::translate_terminator(
                    &mut builder,
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

        let func_id = self.func_ids.get(&func.name).unwrap();
        self.module.define_function(*func_id, &mut ctx).unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_statement(
        func_mir: &MirFunction,
        module: &mut M,
        func_ids: &HashMap<String, FuncId>,
        string_ids: &HashMap<String, DataId>,
        struct_fields: &HashMap<String, Vec<String>>,
        c_struct_offsets: &HashMap<String, Vec<(String, i32, String, Option<(u8, u8)>)>>,
        c_struct_names: &std::collections::HashSet<String>,
        c_struct_sizes: &HashMap<String, i64>,
        c_struct_destructors: &HashMap<String, String>,
        ffi_vararg_ptrs: &HashMap<String, *const u8>,
        ffi_vararg_ids: &std::collections::HashSet<String>,
        ffi_entries: &[super::FfiFnEntry],
        builder: &mut FunctionBuilder,
        stmt: &Statement,
        vars: &HashMap<Local, Variable>,
    ) {
        match &stmt.kind {
            StatementKind::Assign(local, rval) => {
                let val = Self::translate_rvalue(
                    func_mir,
                    module,
                    func_ids,
                    string_ids,
                    struct_fields,
                    c_struct_offsets,
                    c_struct_sizes,
                    ffi_vararg_ptrs,
                    ffi_vararg_ids,
                    ffi_entries,
                    builder,
                    rval,
                    vars,
                );
                let var = vars.get(local).unwrap();

                let val_ty = builder.func.dfg.value_type(val);
                let decl_ty = cl_type(&func_mir.locals[local.0].ty);
                let val = if val_ty != decl_ty && val_ty.bits() == decl_ty.bits() {
                    builder.ins().bitcast(decl_ty, MemFlags::new(), val)
                } else {
                    val
                };

                builder.def_var(*var, val);
            }
            StatementKind::SetAttr(obj, attr, val_op) => {
                let obj_ty = if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    &func_mir.locals[loc.0].ty
                } else {
                    &OliveType::Any
                };
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
                    if let Some(fields) = struct_fields.get(struct_name.as_str()) {
                        if let Some(idx) = fields.iter().position(|f| f == attr) {
                            let offset = 8 + (idx as i32) * 8;
                            let o = Self::translate_operand(
                                builder, obj, vars, string_ids, module, func_ids,
                            );
                            let v = Self::translate_operand(
                                builder, val_op, vars, string_ids, module, func_ids,
                            );
                            let v = if builder.func.dfg.value_type(v) == types::F64 {
                                builder.ins().bitcast(types::I64, MemFlags::new(), v)
                            } else {
                                v
                            };
                            builder.ins().store(MemFlags::trusted(), v, o, offset);
                            return;
                        }
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

                let set_id = func_ids.get("__olive_obj_set").unwrap();
                let local_func = module.declare_func_in_func(*set_id, builder.func);
                builder.ins().call(local_func, &[o, attr_val, v]);
            }
            StatementKind::SetIndex(obj, idx, val_op) => {
                let ty = if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    &func_mir.locals[loc.0].ty
                } else {
                    &OliveType::Any
                };

                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module, func_ids);
                let v =
                    Self::translate_operand(builder, val_op, vars, string_ids, module, func_ids);

                let v = if builder.func.dfg.value_type(v) == types::F64 {
                    builder.ins().bitcast(types::I64, MemFlags::new(), v)
                } else {
                    v
                };

                match ty {
                    OliveType::Dict(_, _) | OliveType::Struct(_, _) => {
                        let set_id = func_ids.get("__olive_obj_set").unwrap();
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v]);
                    }
                    OliveType::Any => {
                        let set_id = func_ids.get("__olive_set_index_any").unwrap();
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v]);
                    }
                    OliveType::Enum(_, _) => {
                        let set_id = func_ids.get("__olive_enum_set").unwrap();
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v]);
                    }
                    _ => {
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
                        let free_id = func_ids.get("__olive_free_c_struct").unwrap();
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
                    _ => "__olive_free",
                };

                let free_id = func_ids.get(free_func_name).unwrap();
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
                    let id = *string_ids.get(s).expect("string constant not found");
                    let local_id = module.declare_data_in_func(id, builder.func);
                    let ptr = builder.ins().symbol_value(types::I64, local_id);
                    builder.ins().bor_imm(ptr, 1)
                }
                Constant::Function(name) => {
                    if let Some(&func_id) = func_ids.get(name) {
                        let local_ref = module.declare_func_in_func(func_id, builder.func);
                        builder.ins().func_addr(types::I64, local_ref)
                    } else {
                        builder.ins().iconst(types::I64, 0)
                    }
                }
                _ => builder.ins().iconst(types::I64, 0),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_terminator(
        builder: &mut FunctionBuilder,
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
                if targets.len() == 1 && targets[0].0 == 1 {
                    let target_block = blocks[targets[0].1.0];
                    let else_block = blocks[otherwise.0];
                    let cond = builder.ins().icmp_imm(IntCC::NotEqual, val, 0);
                    builder.ins().brif(cond, target_block, &[], else_block, &[]);
                } else {
                    let mut switch = cranelift::frontend::Switch::new();
                    for (v, target_bb) in targets {
                        switch.set_entry(*v as u128, blocks[target_bb.0]);
                    }
                    switch.emit(builder, val, blocks[otherwise.0]);
                }
            }
            TerminatorKind::Return => {
                let var = vars.get(&Local(0)).unwrap();
                let ret_val = builder.use_var(*var);
                if is_async {
                    let make_future_id = func_ids.get("__olive_make_future").unwrap();
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
