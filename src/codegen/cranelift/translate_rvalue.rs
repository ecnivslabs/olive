use super::CraneliftCodegen;
use super::translate::{attr_symbol, c_struct_field_info};
use crate::mir::{Constant, Local, MirFunction, Operand, Rvalue};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::FxHashMap as HashMap;

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
        c_struct_offsets: &HashMap<String, Vec<super::FfiStructFieldLayout>>,
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
            Rvalue::Call { func, args } => Self::translate_call(
                func_mir,
                module,
                func_ids,
                string_ids,
                c_struct_offsets,
                c_struct_sizes,
                ffi_vararg_ptrs,
                ffi_vararg_ids,
                ffi_entries,
                builder,
                vars,
                func,
                args,
            ),
            Rvalue::BinaryOp(op, lhs, rhs) => Self::translate_binop(
                func_mir, module, func_ids, string_ids, builder, vars, op, lhs, rhs,
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
                if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    let mut obj_ty = &func_mir.locals[loc.0].ty;
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
                    .get("__olive_obj_get")
                    .expect("missing __olive_obj_get");
                let local_func = module.declare_func_in_func(*get_id, builder.func);
                let inst = builder.ins().call(local_func, &[o, attr_val]);
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
            Rvalue::GetIndex(obj, idx) => {
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
                    OliveType::Dict(_, _) | OliveType::Struct(_, _) => {
                        let get_id = func_ids
                            .get("__olive_obj_get")
                            .expect("missing __olive_obj_get");
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
                        let get_id = func_ids
                            .get("__olive_get_index_any")
                            .expect("missing __olive_get_index_any");
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
                        let get_id = func_ids
                            .get("__olive_str_get")
                            .expect("missing __olive_str_get");
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
                        let get_id = func_ids
                            .get("__olive_get_index_any")
                            .expect("missing __olive_get_index_any");
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
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
            Rvalue::Aggregate(kind, ops) => {
                Self::translate_aggregate(builder, vars, string_ids, module, func_ids, kind, ops)
            }
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
