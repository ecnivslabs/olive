use super::CraneliftCodegen;
use super::translate::{attr_symbol, c_struct_field_info};
use crate::mir::{AggregateKind, Constant, Local, MirFunction, Operand, Rvalue};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Materialises a tagged Olive string pointer for an interned source location,
/// or a null pointer when no location was recorded for the site.
pub(super) fn loc_value<M: Module>(
    builder: &mut FunctionBuilder,
    module: &mut M,
    loc_id: Option<DataId>,
) -> Value {
    match loc_id {
        Some(id) => {
            let local = module.declare_data_in_func(id, builder.func);
            let ptr = builder.ins().symbol_value(types::I64, local);
            builder.ins().bor_imm(ptr, 1)
        }
        None => builder.ins().iconst(types::I64, 0),
    }
}

/// Panics with a null-index diagnostic unless `obj` is non-null, then continues
/// in a fresh block. The fault path never returns.
pub(super) fn emit_nil_check<M: Module>(
    builder: &mut FunctionBuilder,
    module: &mut M,
    func_ids: &HashMap<String, FuncId>,
    obj: Value,
    loc: Value,
) {
    let ok = builder.create_block();
    let fail = builder.create_block();
    let nonnull = builder.ins().icmp_imm(IntCC::NotEqual, obj, 0);
    builder.ins().brif(nonnull, ok, &[], fail, &[]);

    builder.seal_block(fail);
    builder.switch_to_block(fail);
    let id = *func_ids
        .get("__olive_nil_index_fail")
        .expect("missing __olive_nil_index_fail");
    let f = module.declare_func_in_func(id, builder.func);
    builder.ins().call(f, &[loc]);
    builder.ins().trap(TrapCode::unwrap_user(1));

    builder.seal_block(ok);
    builder.switch_to_block(ok);
}

/// Panics with an out-of-bounds diagnostic unless `idx` lies in `0..len`, then
/// continues in a fresh block. An unsigned comparison folds the negative-index
/// case into the same check. The fault path never returns.
pub(super) fn emit_bounds_check<M: Module>(
    builder: &mut FunctionBuilder,
    module: &mut M,
    func_ids: &HashMap<String, FuncId>,
    idx: Value,
    len: Value,
    loc: Value,
) -> Value {
    let ok = builder.create_block();
    let fail = builder.create_block();
    // Negative indices wrap: idx = idx < 0 ? idx + len : idx
    let is_neg = builder.ins().icmp_imm(IntCC::SignedLessThan, idx, 0);
    let wrapped = builder.ins().iadd(idx, len);
    let final_idx = builder.ins().select(is_neg, wrapped, idx);
    let in_bounds = builder.ins().icmp(IntCC::UnsignedLessThan, final_idx, len);
    builder.ins().brif(in_bounds, ok, &[], fail, &[]);

    builder.seal_block(fail);
    builder.switch_to_block(fail);
    let id = *func_ids
        .get("__olive_bounds_fail")
        .expect("missing __olive_bounds_fail");
    let f = module.declare_func_in_func(id, builder.func);
    builder.ins().call(f, &[final_idx, len, loc]);
    builder.ins().trap(TrapCode::unwrap_user(1));

    builder.seal_block(ok);
    builder.switch_to_block(ok);
    final_idx
}

/// Panics with a divide-by-zero diagnostic when an integer `/` or `%` has a
/// zero divisor, then continues in a fresh block. `is_mod` selects the message.
/// The fault path never returns.
pub(super) fn emit_div_zero_check<M: Module>(
    builder: &mut FunctionBuilder,
    module: &mut M,
    func_ids: &HashMap<String, FuncId>,
    divisor: Value,
    is_mod: bool,
    loc: Value,
) {
    let ok = builder.create_block();
    let fail = builder.create_block();
    let nonzero = builder.ins().icmp_imm(IntCC::NotEqual, divisor, 0);
    builder.ins().brif(nonzero, ok, &[], fail, &[]);

    builder.seal_block(fail);
    builder.switch_to_block(fail);
    let id = *func_ids
        .get("__olive_div_zero_fail")
        .expect("missing __olive_div_zero_fail");
    let f = module.declare_func_in_func(id, builder.func);
    let flag = builder.ins().iconst(types::I64, is_mod as i64);
    builder.ins().call(f, &[flag, loc]);
    builder.ins().trap(TrapCode::unwrap_user(1));

    builder.seal_block(ok);
    builder.switch_to_block(ok);
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

impl<M: Module> CraneliftCodegen<M> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_rvalue(
        func_mir: &MirFunction,
        module: &mut M,
        func_ids: &HashMap<String, FuncId>,
        string_ids: &HashMap<String, DataId>,
        struct_fields: &HashMap<String, Vec<String>>,
        field_types: &HashMap<(String, String), crate::semantic::types::Type>,
        enum_defs: &HashMap<String, Vec<(String, Vec<crate::semantic::types::Type>)>>,
        c_struct_offsets: &HashMap<String, Vec<super::FfiStructFieldLayout>>,
        c_struct_sizes: &HashMap<String, i64>,
        ffi_vararg_ptrs: &HashMap<String, *const u8>,
        ffi_vararg_ids: &std::collections::HashSet<String>,
        ffi_entries: &[super::FfiFnEntry],
        dispatch_ids: &HashMap<String, DataId>,
        any_add_site_ids: &[DataId],
        any_add_site_cursor: &mut usize,
        specialize_sites: &HashSet<usize>,
        builder: &mut FunctionBuilder,
        rval: &Rvalue,
        vars: &HashMap<Local, Variable>,
        loc_id: Option<DataId>,
        reuse_target: Option<(Local, Value, bool)>,
    ) -> Value {
        let is_reusable = match rval {
            Rvalue::Aggregate(kind, _) => !matches!(kind, AggregateKind::FatPtr),
            _ => false,
        };
        if let Some((reuse_local, reuse_val, _)) = reuse_target
            && !is_reusable
        {
            let ty = &func_mir.locals[reuse_local.0].ty;
            if let Some(desc_ty) = super::imports::drop_descriptor_type(ty, struct_fields) {
                let desc =
                    super::imports::type_descriptor(desc_ty, struct_fields, field_types, enum_defs);
                let data_id = *string_ids.get(&desc).unwrap();
                let local_data = module.declare_data_in_func(data_id, builder.func);
                let desc_ptr = builder.ins().symbol_value(types::I64, local_data);
                let free_id = func_ids["__olive_free_typed"];
                let local_func = module.declare_func_in_func(free_id, builder.func);
                builder.ins().call(local_func, &[reuse_val, desc_ptr]);
            }
        }
        match rval {
            Rvalue::Use(op) => {
                Self::translate_operand(builder, op, vars, string_ids, module, func_ids)
            }
            Rvalue::GenOf(op) => {
                let v = Self::translate_operand(builder, op, vars, string_ids, module, func_ids);
                let is_str = matches!(
                    op,
                    Operand::Copy(l) | Operand::Move(l)
                        if crate::mir::optimizations::gencheck::str_backed(&func_mir.locals[l.0].ty)
                );
                if is_str {
                    let id = func_ids
                        .get("__olive_str_gen_of")
                        .expect("missing __olive_str_gen_of");
                    let local_func = module.declare_func_in_func(*id, builder.func);
                    let inst = builder.ins().call(local_func, &[v]);
                    return builder.inst_results(inst)[0];
                }
                let is_struct = matches!(
                    op,
                    Operand::Copy(l) | Operand::Move(l)
                        if crate::mir::optimizations::gencheck::struct_backed(&func_mir.locals[l.0].ty)
                );
                if is_struct {
                    let id = func_ids
                        .get("__olive_struct_gen_of")
                        .expect("missing __olive_struct_gen_of");
                    let local_func = module.declare_func_in_func(*id, builder.func);
                    let inst = builder.ins().call(local_func, &[v]);
                    return builder.inst_results(inst)[0];
                }
                // Branchless null guard: a null borrow reads a scratch slot;
                // the check that consumes the word skips null anyway.
                let slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let scratch = builder.ins().stack_addr(types::I64, slot, 0);
                let header = builder.ins().iadd_imm(v, -8);
                let is_null = builder.ins().icmp_imm(IntCC::Equal, v, 0);
                let addr = builder.ins().select(is_null, scratch, header);
                builder.ins().load(types::I64, MemFlags::trusted(), addr, 0)
            }
            Rvalue::Call { func, args } => Self::translate_call(
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
                builder,
                vars,
                func,
                args,
            ),
            Rvalue::BinaryOp(op, lhs, rhs) => Self::translate_binop(
                func_mir,
                module,
                func_ids,
                string_ids,
                any_add_site_ids,
                any_add_site_cursor,
                specialize_sites,
                builder,
                vars,
                op,
                lhs,
                rhs,
                loc_id,
            ),
            Rvalue::UnaryOp(op, operand) => {
                let operand_ty = match operand {
                    crate::mir::Operand::Copy(l) | crate::mir::Operand::Move(l) => {
                        func_mir.locals[l.0].ty.clone()
                    }
                    _ => crate::semantic::types::Type::Any,
                };
                Self::translate_unaryop(
                    builder,
                    vars,
                    string_ids,
                    module,
                    func_ids,
                    op,
                    operand,
                    &operand_ty,
                )
            }
            Rvalue::Ref(local) | Rvalue::MutRef(local) => {
                let var = vars.get(local).unwrap();
                builder.use_var(*var)
            }
            Rvalue::GetAttr(obj, attr) => {
                let loc = loc_value(builder, module, loc_id);
                if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    let mut obj_ty = &func_mir.locals[loc.0].ty;
                    while let OliveType::Ref(inner) | OliveType::MutRef(inner) = obj_ty {
                        obj_ty = inner;
                    }
                    if let OliveType::Struct(struct_name, _, _) = obj_ty {
                        if let Some((offset, ty_name, bits)) =
                            c_struct_field_info(c_struct_offsets, struct_name, attr)
                        {
                            let o = Self::translate_operand(
                                builder, obj, vars, string_ids, module, func_ids,
                            );
                            return load_and_extend(builder, o, offset, ty_name, bits);
                        }
                        if let Some(fields) = struct_fields.get(struct_name.as_str())
                            && let Some(idx) = fields.iter().position(|f| f == attr)
                        {
                            let offset = 8 + (idx as i32) * 8;
                            let o = Self::translate_operand(
                                builder, obj, vars, string_ids, module, func_ids,
                            );
                            return builder
                                .ins()
                                .load(types::I64, MemFlags::trusted(), o, offset);
                        }
                    }
                    if matches!(obj_ty, OliveType::PyObject) {
                        let o = Self::translate_operand(
                            builder, obj, vars, string_ids, module, func_ids,
                        );
                        let attr_val = attr_symbol(builder, module, string_ids, attr);
                        let get_id = func_ids
                            .get("__olive_py_getattr")
                            .expect("missing __olive_py_getattr");
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, attr_val]);
                        return builder.inst_results(inst)[0];
                    }
                }
                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let attr_val = attr_symbol(builder, module, string_ids, attr);

                let get_id = func_ids
                    .get("__olive_obj_get_checked")
                    .expect("missing __olive_obj_get_checked");
                let local_func = module.declare_func_in_func(*get_id, builder.func);
                let inst = builder.ins().call(local_func, &[o, attr_val, loc]);
                builder.inst_results(inst)[0]
            }
            Rvalue::GetTag(obj) => {
                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let tag_id = func_ids
                    .get("__olive_enum_tag")
                    .expect("missing __olive_enum_tag");
                let local_func = module.declare_func_in_func(*tag_id, builder.func);
                let inst = builder.ins().call(local_func, &[o]);
                builder.inst_results(inst)[0]
            }
            Rvalue::GetTypeId(obj) => {
                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let fn_id = func_ids
                    .get("__olive_enum_type_id")
                    .expect("missing __olive_enum_type_id");
                let local_func = module.declare_func_in_func(*fn_id, builder.func);
                let inst = builder.ins().call(local_func, &[o]);
                builder.inst_results(inst)[0]
            }
            Rvalue::GetIndex(obj, idx, unchecked) => {
                let unchecked = *unchecked;
                let mut ty = match obj {
                    Operand::Copy(loc) | Operand::Move(loc) => &func_mir.locals[loc.0].ty,
                    Operand::Constant(Constant::Str(_)) => &OliveType::Str,
                    _ => &OliveType::Any,
                };
                while let OliveType::Ref(inner) | OliveType::MutRef(inner) = ty {
                    ty = inner;
                }

                let o = Self::translate_operand(builder, obj, vars, string_ids, module, func_ids);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module, func_ids);
                let loc = loc_value(builder, module, loc_id);

                match ty {
                    OliveType::PyObject => {
                        let get_id = func_ids
                            .get("__olive_py_getitem")
                            .expect("missing __olive_py_getitem");
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        builder.inst_results(inst)[0]
                    }
                    OliveType::Enum(_, _) => {
                        let get_id = func_ids
                            .get("__olive_enum_get")
                            .expect("missing __olive_enum_get");
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        builder.inst_results(inst)[0]
                    }
                    OliveType::Dict(_, _) | OliveType::Struct(_, _, _) => {
                        let get_id = func_ids
                            .get("__olive_obj_get_checked")
                            .expect("missing __olive_obj_get_checked");
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i, loc]);
                        builder.inst_results(inst)[0]
                    }
                    OliveType::Any => {
                        emit_nil_check(builder, module, func_ids, o, loc);
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
                        let len = builder.ins().load(
                            types::I64,
                            MemFlags::trusted().with_readonly(),
                            o,
                            24,
                        );
                        let fast_idx = if !unchecked {
                            emit_bounds_check(builder, module, func_ids, i, len, loc)
                        } else {
                            i
                        };
                        let offset = builder.ins().imul_imm(fast_idx, 8);
                        let addr = builder.ins().iadd(data_ptr, offset);
                        let fast_val = builder.ins().load(types::I64, MemFlags::trusted(), addr, 0);
                        builder.def_var(result_var, fast_val);
                        builder.ins().jump(merge_block, &[]);

                        builder.seal_block(slow_block);
                        builder.switch_to_block(slow_block);
                        let get_id = func_ids
                            .get("__olive_get_index_any")
                            .expect("missing __olive_get_index_any");
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i, loc]);
                        let slow_val = builder.inst_results(inst)[0];
                        builder.def_var(result_var, slow_val);
                        builder.ins().jump(merge_block, &[]);

                        builder.seal_block(merge_block);
                        builder.switch_to_block(merge_block);
                        builder.use_var(result_var)
                    }
                    OliveType::Str => {
                        let get_id = func_ids
                            .get("__olive_str_get_checked")
                            .expect("missing __olive_str_get_checked");
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i, loc]);
                        builder.inst_results(inst)[0]
                    }
                    OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_) => {
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
                        builder.ins().load(types::I64, MemFlags::trusted(), addr, 0)
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
                        let byte = builder.ins().load(types::I8, MemFlags::trusted(), addr, 0);
                        builder.ins().uextend(types::I64, byte)
                    }
                    _ => {
                        let get_id = func_ids
                            .get("__olive_get_index_any")
                            .expect("missing __olive_get_index_any");
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i, loc]);
                        builder.inst_results(inst)[0]
                    }
                }
            }
            Rvalue::Cast(op, ty) => {
                let val = Self::translate_operand(builder, op, vars, string_ids, module, func_ids);
                let current_ty = builder.func.dfg.value_type(val);
                let target_cl_ty = match ty {
                    OliveType::F32 => types::F32,
                    OliveType::Float => types::F64,
                    _ => types::I64,
                };

                // Scalar-to-str: int/float/bool formatted as string.
                if *ty == OliveType::Str {
                    let src_is_int = match op {
                        crate::mir::Operand::Copy(l) | crate::mir::Operand::Move(l) => {
                            matches!(
                                func_mir.locals[l.0].ty,
                                OliveType::Int
                                    | OliveType::I8
                                    | OliveType::I16
                                    | OliveType::I32
                                    | OliveType::U8
                                    | OliveType::U16
                                    | OliveType::U32
                                    | OliveType::U64
                                    | OliveType::Usize
                            )
                        }
                        _ => false,
                    };
                    let src_is_float = match op {
                        crate::mir::Operand::Copy(l) | crate::mir::Operand::Move(l) => {
                            matches!(func_mir.locals[l.0].ty, OliveType::Float | OliveType::F32)
                        }
                        _ => false,
                    };
                    let src_is_bool = match op {
                        crate::mir::Operand::Copy(l) | crate::mir::Operand::Move(l) => {
                            func_mir.locals[l.0].ty == OliveType::Bool
                        }
                        _ => false,
                    };
                    if src_is_int {
                        let id = func_ids.get("__olive_str").expect("missing __olive_str");
                        let f = module.declare_func_in_func(*id, builder.func);
                        let inst = builder.ins().call(f, &[val]);
                        return builder.inst_results(inst)[0];
                    }
                    if src_is_float {
                        let id = func_ids
                            .get("__olive_float_to_str")
                            .expect("missing __olive_float_to_str");
                        let f = module.declare_func_in_func(*id, builder.func);
                        let inst = builder.ins().call(f, &[val]);
                        return builder.inst_results(inst)[0];
                    }
                    if src_is_bool {
                        let id = func_ids
                            .get("__olive_bool_to_str")
                            .expect("missing __olive_bool_to_str");
                        let f = module.declare_func_in_func(*id, builder.func);
                        let inst = builder.ins().call(f, &[val]);
                        return builder.inst_results(inst)[0];
                    }
                }

                let src_is_pyobj = match op {
                    crate::mir::Operand::Copy(l) | crate::mir::Operand::Move(l) => {
                        matches!(func_mir.locals[l.0].ty, OliveType::PyObject)
                    }
                    _ => false,
                };

                if src_is_pyobj
                    && target_cl_ty == types::I64
                    && !matches!(ty, OliveType::PyObject | OliveType::Float | OliveType::F32)
                {
                    let to_int_id = func_ids
                        .get("__olive_py_to_int")
                        .expect("missing __olive_py_to_int");
                    let local_func = module.declare_func_in_func(*to_int_id, builder.func);
                    let inst = builder.ins().call(local_func, &[val]);
                    builder.inst_results(inst)[0]
                } else if current_ty == target_cl_ty {
                    val
                } else if current_ty.is_float() && target_cl_ty.is_float() {
                    if current_ty == types::F64 {
                        builder.ins().fdemote(target_cl_ty, val)
                    } else {
                        builder.ins().fpromote(target_cl_ty, val)
                    }
                } else if current_ty.is_int() && target_cl_ty.is_float() {
                    if src_is_pyobj {
                        let float_id = func_ids
                            .get("__olive_py_to_float")
                            .expect("missing __olive_py_to_float");
                        let local_func = module.declare_func_in_func(*float_id, builder.func);
                        let inst = builder.ins().call(local_func, &[val]);
                        let f64_val = builder.inst_results(inst)[0];
                        if target_cl_ty == types::F32 {
                            builder.ins().fdemote(types::F32, f64_val)
                        } else {
                            f64_val
                        }
                    } else {
                        builder.ins().fcvt_from_sint(target_cl_ty, val)
                    }
                } else if current_ty.is_float() && target_cl_ty.is_int() {
                    builder.ins().fcvt_to_sint(target_cl_ty, val)
                } else if current_ty.is_int() && target_cl_ty.is_int() {
                    if current_ty.bits() < target_cl_ty.bits() {
                        let src_signed = match op {
                            crate::mir::Operand::Copy(l) | crate::mir::Operand::Move(l) => {
                                matches!(
                                    func_mir.locals[l.0].ty,
                                    OliveType::Int
                                        | OliveType::I32
                                        | OliveType::I16
                                        | OliveType::I8
                                )
                            }
                            crate::mir::Operand::Constant(crate::mir::Constant::Int(_)) => true,
                            _ => true,
                        };
                        if src_signed {
                            builder.ins().sextend(target_cl_ty, val)
                        } else {
                            builder.ins().uextend(target_cl_ty, val)
                        }
                    } else {
                        builder.ins().ireduce(target_cl_ty, val)
                    }
                } else {
                    val
                }
            }
            Rvalue::Aggregate(kind, ops) => Self::translate_aggregate(
                func_mir,
                builder,
                vars,
                string_ids,
                module,
                func_ids,
                kind,
                ops,
                reuse_target,
            ),
            Rvalue::VTableLoad { vtable, method_idx } => {
                let fat_ptr_val =
                    Self::translate_operand(builder, vtable, vars, string_ids, module, func_ids);
                let vtable_ptr =
                    builder
                        .ins()
                        .load(types::I64, MemFlags::trusted(), fat_ptr_val, 8);
                let offset = (*method_idx * 8) as i32;
                builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), vtable_ptr, offset)
            }
            Rvalue::FatPtrData(fat_ptr) => {
                let fat_ptr_val =
                    Self::translate_operand(builder, fat_ptr, vars, string_ids, module, func_ids);
                builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), fat_ptr_val, 0)
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
}

#[cfg(test)]
mod tests {
    use crate::test_utils::{call_i64, call_i64_1, call_i64_2, compile};

    #[test]
    fn test_translate_rvalue_use() {
        let mut cg = compile("fn f(x: i64) -> i64:\n    return x\n");
        assert_eq!(call_i64_1(&mut cg, "f", 42), 42);
    }

    #[test]
    fn test_translate_rvalue_ref() {
        let mut cg = compile("fn f(x: i64) -> i64:\n    return x\n");
        assert_eq!(call_i64_1(&mut cg, "f", 99), 99);
    }

    #[test]
    fn test_translate_rvalue_cast_int_to_int() {
        let mut cg = compile("fn f(x: i64) -> i64:\n    return x\n");
        assert_eq!(call_i64_1(&mut cg, "f", 7), 7);
    }

    #[test]
    fn test_translate_rvalue_get_index_list() {
        let code = "fn f() -> i64:\n    let xs = [10, 20, 30]\n    return xs[1]\n";
        let _cg = compile(code);
    }

    #[test]
    fn test_translate_rvalue_get_attr_struct() {
        let code = "struct P:\n    x: i64\n    y: i64\n\nfn f() -> i64:\n    let p = P(10, 32)\n    return p.x + p.y\n";
        let mut cg = compile(code);
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn test_translate_rvalue_aggregate_tuple() {
        let mut cg =
            compile("fn f(a: i64, b: i64) -> i64:\n    let t = (a, b)\n    return t[0] + t[1]\n");
        assert_eq!(call_i64_2(&mut cg, "f", 10, 32), 42);
    }

    #[test]
    fn test_translate_rvalue_constant_int() {
        let mut cg = compile("fn f() -> i64:\n    return 42\n");
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn test_translate_rvalue_move() {
        let mut cg = compile("fn f(x: i64) -> i64:\n    return x\n");
        assert_eq!(call_i64_1(&mut cg, "f", 42), 42);
    }
}
