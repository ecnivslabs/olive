use super::CraneliftCodegen;
use crate::mir::ir::AggregateKind;
use crate::mir::{Local, Operand};
use cranelift::prelude::*;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::FxHashMap as HashMap;

impl<M: Module> CraneliftCodegen<M> {
    pub(super) fn translate_aggregate(
        builder: &mut FunctionBuilder,
        vars: &HashMap<Local, Variable>,
        string_ids: &HashMap<String, DataId>,
        module: &mut M,
        func_ids: &HashMap<String, FuncId>,
        kind: &AggregateKind,
        ops: &[Operand],
    ) -> Value {
        match kind {
            AggregateKind::Dict => {
                let new_id = func_ids
                    .get("__olive_obj_new")
                    .expect("missing __olive_obj_new");
                let new_func = module.declare_func_in_func(*new_id, builder.func);
                let inst = builder.ins().call(new_func, &[]);
                let dict_ptr = builder.inst_results(inst)[0];

                let set_id = func_ids
                    .get("__olive_obj_set")
                    .expect("missing __olive_obj_set");
                let set_func = module.declare_func_in_func(*set_id, builder.func);

                for i in (0..ops.len()).step_by(2) {
                    let key = Self::translate_operand(
                        builder, &ops[i], vars, string_ids, module, func_ids,
                    );
                    let mut val = Self::translate_operand(
                        builder,
                        &ops[i + 1],
                        vars,
                        string_ids,
                        module,
                        func_ids,
                    );
                    if builder.func.dfg.value_type(val) == types::F64 {
                        val = builder.ins().bitcast(types::I64, MemFlags::new(), val);
                    }
                    builder.ins().call(set_func, &[dict_ptr, key, val]);
                }
                dict_ptr
            }
            AggregateKind::EnumVariant(type_id, tag) => {
                let type_id_val = builder.ins().iconst(types::I64, *type_id);
                let tag_val = builder.ins().iconst(types::I64, *tag as i64);
                let count = builder.ins().iconst(types::I64, ops.len() as i64);
                let new_id = func_ids
                    .get("__olive_enum_new")
                    .expect("missing __olive_enum_new");
                let new_func = module.declare_func_in_func(*new_id, builder.func);
                let inst = builder.ins().call(new_func, &[type_id_val, tag_val, count]);
                let enum_ptr = builder.inst_results(inst)[0];

                let set_id = func_ids
                    .get("__olive_enum_set")
                    .expect("missing __olive_enum_set");
                let set_func = module.declare_func_in_func(*set_id, builder.func);

                for (i, op) in ops.iter().enumerate() {
                    let idx = builder.ins().iconst(types::I64, i as i64);
                    let val =
                        Self::translate_operand(builder, op, vars, string_ids, module, func_ids);
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
                let new_id = func_ids
                    .get("__olive_set_new")
                    .expect("missing __olive_set_new");
                let new_func = module.declare_func_in_func(*new_id, builder.func);
                let inst = builder.ins().call(new_func, &[count]);
                let set_ptr = builder.inst_results(inst)[0];

                let add_id = func_ids
                    .get("__olive_set_add")
                    .expect("missing __olive_set_add");
                let add_func = module.declare_func_in_func(*add_id, builder.func);

                for op in ops {
                    let mut val =
                        Self::translate_operand(builder, op, vars, string_ids, module, func_ids);
                    if builder.func.dfg.value_type(val) == types::F64 {
                        val = builder.ins().bitcast(types::I64, MemFlags::new(), val);
                    }
                    builder.ins().call(add_func, &[set_ptr, val]);
                }
                set_ptr
            }
            AggregateKind::FatPtr => {
                let alloc_id = func_ids
                    .get("__olive_alloc")
                    .expect("missing __olive_alloc");
                let alloc_func = module.declare_func_in_func(*alloc_id, builder.func);
                let size = builder.ins().iconst(types::I64, 16);
                let inst = builder.ins().call(alloc_func, &[size]);
                let ptr = builder.inst_results(inst)[0];

                let data_val =
                    Self::translate_operand(builder, &ops[0], vars, string_ids, module, func_ids);
                let vtable_val =
                    Self::translate_operand(builder, &ops[1], vars, string_ids, module, func_ids);

                builder.ins().store(MemFlags::trusted(), data_val, ptr, 0);
                builder.ins().store(MemFlags::trusted(), vtable_val, ptr, 8);
                ptr
            }
            _ => {
                let n = ops.len() as i64;
                let n_val = builder.ins().iconst(types::I64, n);
                let new_id = func_ids
                    .get("__olive_list_new")
                    .expect("missing __olive_list_new");
                let new_func = module.declare_func_in_func(*new_id, builder.func);
                let inst = builder.ins().call(new_func, &[n_val]);
                let list_ptr = builder.inst_results(inst)[0];

                let data_ptr = builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), list_ptr, 8);
                for (i, op) in ops.iter().enumerate() {
                    let mut val =
                        Self::translate_operand(builder, op, vars, string_ids, module, func_ids);
                    if builder.func.dfg.value_type(val) == types::F64 {
                        val = builder.ins().bitcast(types::I64, MemFlags::new(), val);
                    }
                    builder
                        .ins()
                        .store(MemFlags::trusted(), val, data_ptr, (i * 8) as i32);
                }
                list_ptr
            }
        }
    }
}
