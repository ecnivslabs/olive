use super::CraneliftCodegen;
use super::imports::{
    cl_type, is_float_op, is_list_op, is_str_op, is_u64_op, map_builtin_to_runtime,
};
use crate::mir::{
    Constant, Local, MirFunction, Operand, Rvalue, Statement, StatementKind, Terminator,
    TerminatorKind,
};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::FxHashMap as HashMap;

fn c_struct_field_info<'a>(
    c_struct_offsets: &'a HashMap<String, Vec<(String, i32, String, Option<(u8, u8)>)>>,
    struct_name: &str,
    attr: &str,
) -> Option<(i32, &'a str, Option<(u8, u8)>)> {
    c_struct_offsets
        .get(struct_name)
        .and_then(|fields| fields.iter().find(|(n, _, _, _)| n == attr))
        .map(|(_, off, ty, bits)| (*off, ty.as_str(), *bits))
}

fn load_and_extend(
    builder: &mut FunctionBuilder,
    ptr: Value,
    offset: i32,
    ty_name: &str,
    bits: Option<(u8, u8)>,
) -> Value {
    if let Some((bit_off, bit_count)) = bits {
        let word_ty = super::ffi_cl_type(ty_name);
        let word = builder
            .ins()
            .load(word_ty, MemFlags::trusted(), ptr, offset);

        let unsigned = matches!(ty_name, "u8" | "u16" | "u32" | "bool");
        let extended = if word_ty == types::I64 {
            word
        } else if unsigned {
            builder.ins().uextend(types::I64, word)
        } else {
            builder.ins().sextend(types::I64, word)
        };

        let shifted = if bit_off > 0 {
            builder.ins().ushr_imm(extended, bit_off as i64)
        } else {
            extended
        };
        let mask = (1i64 << bit_count) - 1;
        let masked = builder.ins().band_imm(shifted, mask);
        if unsigned {
            return masked;
        }

        let shift = (64 - bit_count) as i64;
        let shl = builder.ins().ishl_imm(masked, shift);
        builder.ins().sshr_imm(shl, shift)
    } else {
        let cl_ty = super::ffi_cl_type(ty_name);
        let raw = builder.ins().load(cl_ty, MemFlags::trusted(), ptr, offset);
        if cl_ty == types::I64 || cl_ty == types::F64 || cl_ty == types::F32 {
            return raw;
        }
        let unsigned = matches!(ty_name, "u8" | "u16" | "u32" | "bool");
        if unsigned {
            builder.ins().uextend(types::I64, raw)
        } else {
            builder.ins().sextend(types::I64, raw)
        }
    }
}

fn truncate_for_store(builder: &mut FunctionBuilder, val: Value, ty_name: &str) -> Value {
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

fn attr_symbol(
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

impl<'a, M: Module> CraneliftCodegen<'a, M> {
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
                            // Bitfield write (RMW)
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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_rvalue(
        func_mir: &MirFunction,
        module: &mut M,
        func_ids: &HashMap<String, FuncId>,
        string_ids: &HashMap<String, DataId>,
        struct_fields: &HashMap<String, Vec<String>>,
        c_struct_offsets: &HashMap<String, Vec<(String, i32, String, Option<(u8, u8)>)>>,
        c_struct_sizes: &HashMap<String, i64>,
        ffi_vararg_ptrs: &HashMap<String, *const u8>,
        ffi_vararg_ids: &std::collections::HashSet<String>,
        ffi_entries: &[super::FfiFnEntry],
        builder: &mut FunctionBuilder,
        rval: &Rvalue,
        vars: &HashMap<Local, Variable>,
    ) -> Value {
        match rval {
            Rvalue::Use(op) => {
                Self::translate_operand(builder, op, vars, string_ids, module, func_ids)
            }
            Rvalue::Call { func, args } => {
                let call_args: Vec<Value> = args
                    .iter()
                    .map(|a| {
                        Self::translate_operand(builder, a, vars, string_ids, module, func_ids)
                    })
                    .collect();

                if let Operand::Constant(Constant::Function(name)) = func {
                    if let Some(&size) = c_struct_sizes.get(name.as_str()) {
                        if let Some(&alloc_id) = func_ids.get("__olive_alloc") {
                            let local_fn = module.declare_func_in_func(alloc_id, builder.func);
                            let size_val = builder.ins().iconst(types::I64, size);
                            let inst = builder.ins().call(local_fn, &[size_val]);
                            return builder.inst_results(inst)[0];
                        }
                        return builder.ins().iconst(types::I64, 0);
                    }

                    let resolved_name = if (name == "print"
                        || name == "str"
                        || name == "int"
                        || name == "float"
                        || name == "bool"
                        || name == "iter"
                        || name == "next"
                        || name == "has_next"
                        || name == "slice"
                        || name == "len")
                        && !args.is_empty()
                    {
                        let arg_type = match &args[0] {
                            Operand::Constant(Constant::Str(_)) => OliveType::Str,
                            Operand::Constant(Constant::Float(_)) => OliveType::Float,
                            Operand::Copy(l) | Operand::Move(l) => func_mir.locals[l.0].ty.clone(),
                            _ => OliveType::Int,
                        };

                        map_builtin_to_runtime(name, &arg_type).unwrap_or(name.as_str())
                    } else if name == "ffi_errno" {
                        "__olive_ffi_errno"
                    } else {
                        name.as_str()
                    };

                    if (resolved_name == "__olive_int" || resolved_name == "__olive_copy_float")
                        && !call_args.is_empty()
                    {
                        return call_args[0];
                    }

                    let is_ffi =
                        resolved_name.contains("::") && !resolved_name.starts_with("__olive");

                    if let Some(&func_id) = func_ids.get(resolved_name) {
                        let is_aot_vararg = ffi_vararg_ids.contains(resolved_name);
                        let local_func = module.declare_func_in_func(func_id, builder.func);
                        let mut final_args = Vec::new();
                        let mut sret_ptr = None;
                        let is_builtin =
                            resolved_name.starts_with("__olive") || resolved_name == "print";
                        let accepts_float = resolved_name == "__olive_print_float"
                            || resolved_name == "__olive_float_to_str"
                            || resolved_name == "__olive_float_to_int"
                            || resolved_name == "__olive_bool_from_float"
                            || resolved_name == "__olive_pow_float"
                            || resolved_name == "__olive_copy_float";
                        let ffi_entry = ffi_entries.iter().find(|e| e.jit_name == resolved_name);

                        if let Some(entry) = ffi_entry {
                            if entry.use_sret {
                                let ret_name = entry.ret.as_ref().unwrap();
                                let size = *c_struct_sizes.get(ret_name).unwrap();
                                let slot = builder.create_sized_stack_slot(StackSlotData::new(
                                    StackSlotKind::ExplicitSlot,
                                    size as u32,
                                    3,
                                ));
                                let ptr =
                                    builder
                                        .ins()
                                        .stack_addr(module.isa().pointer_type(), slot, 0);
                                final_args.push(ptr);
                                sret_ptr = Some(ptr);
                            }
                        }

                        for (i, &arg) in call_args.iter().enumerate() {
                            let is_str_arg = args.get(i).is_some_and(|op| match op {
                                Operand::Constant(Constant::Str(_)) => true,
                                Operand::Copy(l) | Operand::Move(l) => {
                                    matches!(func_mir.locals[l.0].ty, OliveType::Str)
                                }
                                _ => false,
                            });

                            if is_ffi {
                                if let Some(entry) = ffi_entry {
                                    if i < entry.params.len() {
                                        let p_type = &entry.params[i];
                                        if let Some(layout) = c_struct_offsets.get(p_type) {
                                            // Unroll struct fields (by value)
                                            for (_, offset, ty_name, bits) in layout {
                                                if bits.is_some() {
                                                    continue;
                                                }
                                                let cl_ty = super::ffi_cl_type(ty_name);
                                                let val = builder.ins().load(
                                                    cl_ty,
                                                    MemFlags::trusted(),
                                                    arg,
                                                    *offset,
                                                );
                                                final_args.push(val);
                                            }
                                            continue;
                                        }
                                    }
                                }
                            }

                            if (is_ffi || is_aot_vararg) && is_str_arg {
                                final_args.push(builder.ins().band_imm(arg, -2));
                            } else if is_builtin
                                && !accepts_float
                                && builder.func.dfg.value_type(arg) == types::F64
                            {
                                final_args.push(builder.ins().bitcast(
                                    types::I64,
                                    MemFlags::new(),
                                    arg,
                                ));
                            } else {
                                final_args.push(arg);
                            }
                        }
                        let inst = if is_aot_vararg {
                            let mut sig = module.make_signature();
                            sig.call_conv = module.isa().default_call_conv();
                            for &a in &final_args {
                                sig.params
                                    .push(AbiParam::new(builder.func.dfg.value_type(a)));
                            }
                            sig.returns.push(AbiParam::new(types::I64));
                            let sig_ref = builder.import_signature(sig);
                            let fn_addr = builder.ins().func_addr(types::I64, local_func);
                            builder.ins().call_indirect(sig_ref, fn_addr, &final_args)
                        } else {
                            builder.ins().call(local_func, &final_args)
                        };

                        let mut ret_val = if let Some(ptr) = sret_ptr {
                            ptr
                        } else {
                            let results = builder.inst_results(inst);
                            if results.is_empty() {
                                builder.ins().iconst(types::I64, 0)
                            } else {
                                results[0]
                            }
                        };

                        if is_ffi {
                            if let Some(entry) = ffi_entry {
                                if let Some(ref r) = entry.ret {
                                    if r == "str" {
                                        ret_val = builder.ins().bor_imm(ret_val, 1);
                                    }
                                }
                            }
                        }
                        return ret_val;
                    }

                    if let Some(&fn_ptr) = ffi_vararg_ptrs.get(resolved_name) {
                        let entry = ffi_entries.iter().find(|e| e.jit_name == resolved_name);
                        let n_fixed = entry.map(|e| e.n_fixed).unwrap_or(0);

                        let mut sig = module.make_signature();
                        sig.call_conv = match entry.and_then(|e| e.call_conv.as_deref()) {
                            #[cfg(target_os = "windows")]
                            Some("stdcall") | Some("fastcall") => {
                                cranelift::prelude::isa::CallConv::WindowsFastcall
                            }
                            _ => module.isa().default_call_conv(),
                        };

                        let mut vararg_args: Vec<Value> = Vec::with_capacity(call_args.len());
                        for (i, &arg_val) in call_args.iter().enumerate() {
                            let is_str_arg = args.get(i).is_some_and(|op| match op {
                                Operand::Constant(Constant::Str(_)) => true,
                                Operand::Copy(l) | Operand::Move(l) => {
                                    matches!(func_mir.locals[l.0].ty, OliveType::Str)
                                }
                                _ => false,
                            });
                            let cooked = if is_str_arg {
                                builder.ins().band_imm(arg_val, -2)
                            } else {
                                arg_val
                            };
                            if i < n_fixed {
                                if let Some(e) = entry {
                                    let declared_ty = super::ffi_cl_type(&e.params[i]);
                                    let cooked = truncate_for_store(builder, cooked, &e.params[i]);
                                    sig.params.push(AbiParam::new(declared_ty));
                                    vararg_args.push(cooked);
                                    continue;
                                }
                            }

                            sig.params
                                .push(AbiParam::new(builder.func.dfg.value_type(cooked)));
                            vararg_args.push(cooked);
                        }

                        if let Some(e) = entry {
                            if let Some(ref r) = e.ret {
                                if r != "void" {
                                    sig.returns.push(AbiParam::new(super::ffi_cl_type(r)));
                                }
                            }
                        } else {
                            sig.returns.push(AbiParam::new(types::I64));
                        }

                        let sig_ref = builder.import_signature(sig);
                        let fn_ptr_val = builder.ins().iconst(types::I64, fn_ptr as i64);
                        let inst = builder
                            .ins()
                            .call_indirect(sig_ref, fn_ptr_val, &vararg_args);
                        let results = builder.inst_results(inst);
                        let mut ret_val = if results.is_empty() {
                            builder.ins().iconst(types::I64, 0)
                        } else {
                            results[0]
                        };
                        if let Some(e) = entry {
                            if e.ret.as_deref() == Some("str") {
                                ret_val = builder.ins().bor_imm(ret_val, 1);
                            }
                        }
                        return ret_val;
                    }
                }
                builder.ins().iconst(types::I64, 0)
            }
            Rvalue::BinaryOp(op, lhs, rhs) => {
                let l = Self::translate_operand(builder, lhs, vars, string_ids, module, func_ids);
                let r = Self::translate_operand(builder, rhs, vars, string_ids, module, func_ids);
                use crate::parser::BinOp::*;
                match op {
                    Add => {
                        let is_str = is_str_op(func_mir, lhs);
                        let is_float = is_float_op(func_mir, lhs);
                        let is_list = is_list_op(func_mir, lhs);

                        if is_str {
                            let concat_func_id = func_ids.get("__olive_str_concat").unwrap();
                            let local_func =
                                module.declare_func_in_func(*concat_func_id, builder.func);
                            let inst = builder.ins().call(local_func, &[l, r]);
                            builder.inst_results(inst)[0]
                        } else if is_list {
                            let concat_func_id = func_ids.get("__olive_list_concat").unwrap();
                            let local_func =
                                module.declare_func_in_func(*concat_func_id, builder.func);
                            let inst = builder.ins().call(local_func, &[l, r]);
                            builder.inst_results(inst)[0]
                        } else if is_float {
                            builder.ins().fadd(l, r)
                        } else {
                            builder.ins().iadd(l, r)
                        }
                    }
                    Sub => {
                        if is_float_op(func_mir, lhs) {
                            builder.ins().fsub(l, r)
                        } else {
                            builder.ins().isub(l, r)
                        }
                    }
                    Mul => {
                        if is_float_op(func_mir, lhs) {
                            builder.ins().fmul(l, r)
                        } else {
                            builder.ins().imul(l, r)
                        }
                    }
                    Div => {
                        if is_float_op(func_mir, lhs) {
                            builder.ins().fdiv(l, r)
                        } else if is_u64_op(func_mir, lhs) || is_u64_op(func_mir, rhs) {
                            builder.ins().udiv(l, r)
                        } else {
                            builder.ins().sdiv(l, r)
                        }
                    }
                    Mod => {
                        if is_u64_op(func_mir, lhs) || is_u64_op(func_mir, rhs) {
                            builder.ins().urem(l, r)
                        } else {
                            builder.ins().srem(l, r)
                        }
                    }
                    Eq => {
                        let mut is_str = false;
                        let mut is_float = false;
                        match lhs {
                            Operand::Constant(Constant::Str(_)) => is_str = true,
                            Operand::Constant(Constant::Float(_)) => is_float = true,
                            Operand::Copy(loc) | Operand::Move(loc) => {
                                let ty = &func_mir.locals[loc.0].ty;
                                if *ty == OliveType::Str {
                                    is_str = true;
                                }
                                if *ty == OliveType::Float {
                                    is_float = true;
                                }
                            }
                            _ => {}
                        }

                        if is_str {
                            let eq_func_id = func_ids.get("__olive_str_eq").unwrap();
                            let local_func = module.declare_func_in_func(*eq_func_id, builder.func);
                            let inst = builder.ins().call(local_func, &[l, r]);
                            builder.inst_results(inst)[0]
                        } else if is_float {
                            let res = builder.ins().fcmp(FloatCC::Equal, l, r);
                            builder.ins().uextend(types::I64, res)
                        } else {
                            let res = builder.ins().icmp(IntCC::Equal, l, r);
                            builder.ins().uextend(types::I64, res)
                        }
                    }

                    Lt | LtEq | Gt | GtEq | NotEq => {
                        let mut is_float = false;
                        if let Operand::Copy(loc) | Operand::Move(loc) = lhs {
                            if func_mir.locals[loc.0].ty == OliveType::Float {
                                is_float = true;
                            }
                        } else if let Operand::Constant(Constant::Float(_)) = lhs {
                            is_float = true;
                        }
                        let is_u64 = is_u64_op(func_mir, lhs) || is_u64_op(func_mir, rhs);

                        if is_float {
                            let cc = match op {
                                Lt => FloatCC::LessThan,
                                LtEq => FloatCC::LessThanOrEqual,
                                Gt => FloatCC::GreaterThan,
                                GtEq => FloatCC::GreaterThanOrEqual,
                                NotEq => FloatCC::NotEqual,
                                _ => unreachable!(),
                            };
                            let res = builder.ins().fcmp(cc, l, r);
                            builder.ins().uextend(types::I64, res)
                        } else if is_u64 {
                            let cc = match op {
                                Lt => IntCC::UnsignedLessThan,
                                LtEq => IntCC::UnsignedLessThanOrEqual,
                                Gt => IntCC::UnsignedGreaterThan,
                                GtEq => IntCC::UnsignedGreaterThanOrEqual,
                                NotEq => IntCC::NotEqual,
                                _ => unreachable!(),
                            };
                            let res = builder.ins().icmp(cc, l, r);
                            builder.ins().uextend(types::I64, res)
                        } else {
                            let cc = match op {
                                Lt => IntCC::SignedLessThan,
                                LtEq => IntCC::SignedLessThanOrEqual,
                                Gt => IntCC::SignedGreaterThan,
                                GtEq => IntCC::SignedGreaterThanOrEqual,
                                NotEq => IntCC::NotEqual,
                                _ => unreachable!(),
                            };
                            let res = builder.ins().icmp(cc, l, r);
                            builder.ins().uextend(types::I64, res)
                        }
                    }
                    Shl => builder.ins().ishl(l, r),
                    Shr => {
                        if is_u64_op(func_mir, lhs) {
                            builder.ins().ushr(l, r)
                        } else {
                            builder.ins().sshr(l, r)
                        }
                    }
                    And => builder.ins().band(l, r),
                    Or => builder.ins().bor(l, r),
                    Pow => {
                        let is_float = is_float_op(func_mir, lhs);
                        let func_name = if is_float {
                            "__olive_pow_float"
                        } else {
                            "__olive_pow"
                        };
                        let pow_id = func_ids.get(func_name).unwrap();
                        let local_func = module.declare_func_in_func(*pow_id, builder.func);
                        let inst = builder.ins().call(local_func, &[l, r]);
                        builder.inst_results(inst)[0]
                    }
                    In | NotIn => {
                        let is_obj = if let Operand::Copy(loc) | Operand::Move(loc) = rhs {
                            let mut ty = &func_mir.locals[loc.0].ty;
                            while let OliveType::Ref(inner) | OliveType::MutRef(inner) = ty {
                                ty = inner;
                            }
                            matches!(ty, OliveType::Dict(_, _) | OliveType::Struct(_, _))
                        } else {
                            false
                        };
                        let func_name = if is_obj {
                            "__olive_in_obj"
                        } else {
                            "__olive_in_list"
                        };
                        let in_id = func_ids.get(func_name).unwrap();
                        let local_func = module.declare_func_in_func(*in_id, builder.func);
                        let inst = builder.ins().call(local_func, &[l, r]);
                        let res = builder.inst_results(inst)[0];
                        if matches!(op, NotIn) {
                            let is_zero = builder.ins().icmp_imm(IntCC::Equal, res, 0);
                            builder.ins().uextend(types::I64, is_zero)
                        } else {
                            res
                        }
                    }
                }
            }
            Rvalue::UnaryOp(op, operand) => {
                let o =
                    Self::translate_operand(builder, operand, vars, string_ids, module, func_ids);
                use crate::parser::UnaryOp::*;
                match op {
                    Neg => {
                        let is_float = builder.func.dfg.value_type(o) == types::F64;
                        if is_float {
                            builder.ins().fneg(o)
                        } else {
                            builder.ins().ineg(o)
                        }
                    }
                    Not => {
                        let res = builder.ins().icmp_imm(IntCC::Equal, o, 0);
                        builder.ins().uextend(types::I64, res)
                    }
                    Pos => o,
                }
            }
            Rvalue::Ref(local) | Rvalue::MutRef(local) => {
                let var = vars.get(local).unwrap();
                builder.use_var(*var)
            }
            Rvalue::GetAttr(obj, attr) => {
                if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    let obj_ty = &func_mir.locals[loc.0].ty;
                    if let OliveType::Struct(struct_name, _) = obj_ty {
                        if let Some((offset, ty_name, bits)) =
                            c_struct_field_info(c_struct_offsets, struct_name, attr)
                        {
                            let o = Self::translate_operand(
                                builder, obj, vars, string_ids, module, func_ids,
                            );
                            return load_and_extend(builder, o, offset, ty_name, bits);
                        }
                        if let Some(fields) = struct_fields.get(struct_name.as_str()) {
                            if let Some(idx) = fields.iter().position(|f| f == attr) {
                                let offset = 8 + (idx as i32) * 8;
                                let o = Self::translate_operand(
                                    builder, obj, vars, string_ids, module, func_ids,
                                );
                                return builder.ins().load(
                                    types::I64,
                                    MemFlags::trusted(),
                                    o,
                                    offset,
                                );
                            }
                        }
                    }
                }
                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let attr_val = attr_symbol(builder, module, string_ids, attr);

                let get_id = func_ids.get("__olive_obj_get").unwrap();
                let local_func = module.declare_func_in_func(*get_id, builder.func);
                let inst = builder.ins().call(local_func, &[o, attr_val]);
                builder.inst_results(inst)[0]
            }
            Rvalue::GetTag(obj) => {
                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let tag_id = func_ids.get("__olive_enum_tag").unwrap();
                let local_func = module.declare_func_in_func(*tag_id, builder.func);
                let inst = builder.ins().call(local_func, &[o]);
                builder.inst_results(inst)[0]
            }
            Rvalue::GetTypeId(obj) => {
                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let fn_id = func_ids.get("__olive_enum_type_id").unwrap();
                let local_func = module.declare_func_in_func(*fn_id, builder.func);
                let inst = builder.ins().call(local_func, &[o]);
                builder.inst_results(inst)[0]
            }
            Rvalue::GetIndex(obj, idx) => {
                let ty = match obj {
                    Operand::Copy(loc) | Operand::Move(loc) => &func_mir.locals[loc.0].ty,
                    Operand::Constant(Constant::Str(_)) => &OliveType::Str,
                    _ => &OliveType::Any,
                };

                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module, func_ids);

                match ty {
                    OliveType::Enum(_, _) => {
                        let get_id = func_ids.get("__olive_enum_get").unwrap();
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        builder.inst_results(inst)[0]
                    }
                    OliveType::Dict(_, _) | OliveType::Struct(_, _) => {
                        let get_id = func_ids.get("__olive_obj_get").unwrap();
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        builder.inst_results(inst)[0]
                    }
                    OliveType::Any => {
                        let result_var = builder.declare_var(types::I64);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        let merge_block = builder.create_block();

                        let data_ptr = builder.ins().load(
                            types::I64,
                            MemFlags::trusted().with_readonly(),
                            o,
                            8,
                        );
                        let kind = builder.ins().load(
                            types::I64,
                            MemFlags::trusted().with_readonly(),
                            o,
                            0,
                        );
                        let is_list = builder.ins().icmp_imm(IntCC::Equal, kind, 1);
                        builder
                            .ins()
                            .brif(is_list, fast_block, &[], slow_block, &[]);

                        builder.seal_block(fast_block);
                        builder.switch_to_block(fast_block);
                        let offset = builder.ins().imul_imm(i, 8);
                        let addr = builder.ins().iadd(data_ptr, offset);
                        let fast_val = builder.ins().load(types::I64, MemFlags::trusted(), addr, 0);
                        builder.def_var(result_var, fast_val);
                        builder.ins().jump(merge_block, &[]);

                        builder.seal_block(slow_block);
                        builder.switch_to_block(slow_block);
                        let get_id = func_ids.get("__olive_get_index_any").unwrap();
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        let slow_val = builder.inst_results(inst)[0];
                        builder.def_var(result_var, slow_val);
                        builder.ins().jump(merge_block, &[]);

                        builder.seal_block(merge_block);
                        builder.switch_to_block(merge_block);
                        builder.use_var(result_var)
                    }
                    OliveType::Str => {
                        let get_id = func_ids.get("__olive_str_get").unwrap();
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        builder.inst_results(inst)[0]
                    }
                    OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_) => {
                        let data_ptr = builder.ins().load(
                            types::I64,
                            MemFlags::trusted().with_readonly(),
                            o,
                            8,
                        );
                        let offset = builder.ins().imul_imm(i, 8);
                        let addr = builder.ins().iadd(data_ptr, offset);
                        builder.ins().load(types::I64, MemFlags::trusted(), addr, 0)
                    }
                    _ => {
                        let get_id = func_ids.get("__olive_get_index_any").unwrap();
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        builder.inst_results(inst)[0]
                    }
                }
            }
            Rvalue::Aggregate(kind, ops) => {
                use crate::mir::ir::AggregateKind;
                match kind {
                    AggregateKind::Dict => {
                        let new_id = func_ids.get("__olive_obj_new").unwrap();
                        let new_func = module.declare_func_in_func(*new_id, builder.func);
                        let inst = builder.ins().call(new_func, &[]);
                        let dict_ptr = builder.inst_results(inst)[0];

                        let set_id = func_ids.get("__olive_obj_set").unwrap();
                        let set_func = module.declare_func_in_func(*set_id, builder.func);

                        for i in (0..ops.len()).step_by(2) {
                            let key = Self::translate_operand(
                                builder, &ops[i], vars, string_ids, module, func_ids,
                            );
                            let val = Self::translate_operand(
                                builder,
                                &ops[i + 1],
                                vars,
                                string_ids,
                                module,
                                func_ids,
                            );
                            builder.ins().call(set_func, &[dict_ptr, key, val]);
                        }
                        dict_ptr
                    }
                    AggregateKind::EnumVariant(type_id, tag) => {
                        let type_id_val = builder.ins().iconst(types::I64, *type_id);
                        let tag_val = builder.ins().iconst(types::I64, *tag as i64);
                        let count = builder.ins().iconst(types::I64, ops.len() as i64);
                        let new_id = func_ids.get("__olive_enum_new").unwrap();
                        let new_func = module.declare_func_in_func(*new_id, builder.func);
                        let inst = builder.ins().call(new_func, &[type_id_val, tag_val, count]);
                        let enum_ptr = builder.inst_results(inst)[0];

                        let set_id = func_ids.get("__olive_enum_set").unwrap();
                        let set_func = module.declare_func_in_func(*set_id, builder.func);

                        for (i, op) in ops.iter().enumerate() {
                            let idx = builder.ins().iconst(types::I64, i as i64);
                            let val = Self::translate_operand(
                                builder, op, vars, string_ids, module, func_ids,
                            );
                            let val = if builder.func.dfg.value_type(val) == types::F64 {
                                builder.ins().bitcast(types::I64, MemFlags::new(), val)
                            } else {
                                val
                            };
                            builder.ins().call(set_func, &[enum_ptr, idx, val]);
                        }
                        enum_ptr
                    }
                    AggregateKind::Set => {
                        let count = builder.ins().iconst(types::I64, ops.len() as i64);
                        let new_id = func_ids.get("__olive_set_new").unwrap();
                        let new_func = module.declare_func_in_func(*new_id, builder.func);
                        let inst = builder.ins().call(new_func, &[count]);
                        let set_ptr = builder.inst_results(inst)[0];

                        let add_id = func_ids.get("__olive_set_add").unwrap();
                        let add_func = module.declare_func_in_func(*add_id, builder.func);

                        for op in ops {
                            let val = Self::translate_operand(
                                builder, op, vars, string_ids, module, func_ids,
                            );
                            builder.ins().call(add_func, &[set_ptr, val]);
                        }
                        set_ptr
                    }
                    _ => {
                        let n = ops.len() as i64;
                        let n_val = builder.ins().iconst(types::I64, n);
                        let new_id = func_ids.get("__olive_list_new").unwrap();
                        let new_func = module.declare_func_in_func(*new_id, builder.func);
                        let inst = builder.ins().call(new_func, &[n_val]);
                        let list_ptr = builder.inst_results(inst)[0];

                        let data_ptr = builder.ins().iadd_imm(list_ptr, 32);
                        for (i, op) in ops.iter().enumerate() {
                            let val = Self::translate_operand(
                                builder, op, vars, string_ids, module, func_ids,
                            );
                            builder
                                .ins()
                                .store(MemFlags::trusted(), val, data_ptr, (i * 8) as i32);
                        }
                        list_ptr
                    }
                }
            }
            Rvalue::PtrLoad(ptr_op) => {
                let ptr =
                    Self::translate_operand(builder, ptr_op, vars, string_ids, module, func_ids);
                builder.ins().load(types::I64, MemFlags::trusted(), ptr, 0)
            }
            Rvalue::VectorSplat(op, width) => {
                let val = Self::translate_operand(builder, op, vars, string_ids, module, func_ids);
                let inner_ty = builder.func.dfg.value_type(val);
                let vec_ty = inner_ty.by(*width as u32).expect("invalid splat width");
                builder.ins().splat(vec_ty, val)
            }
            Rvalue::VectorLoad(obj, idx, width) => {
                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module, func_ids);
                let data_ptr = builder.ins().load(types::I64, MemFlags::trusted(), o, 8);
                let offset = builder.ins().imul_imm(i, 8);
                let addr = builder.ins().iadd(data_ptr, offset);
                let vec_ty = types::I64.by(*width as u32).unwrap();
                builder.ins().load(vec_ty, MemFlags::trusted(), addr, 0)
            }
            Rvalue::VectorFMA(a_op, b_op, c_op) => {
                let a = Self::translate_operand(builder, a_op, vars, string_ids, module, func_ids);
                let b = Self::translate_operand(builder, b_op, vars, string_ids, module, func_ids);
                let c = Self::translate_operand(builder, c_op, vars, string_ids, module, func_ids);
                let ty = builder.func.dfg.value_type(a);
                if ty.is_int() || ty.lane_type().is_int() {
                    let prod = builder.ins().imul(a, b);
                    builder.ins().iadd(prod, c)
                } else {
                    builder.ins().fma(a, b, c)
                }
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
