use super::CraneliftCodegen;
use crate::mir::ir::AggregateKind;
use crate::mir::{Local, MirFunction, Operand};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::FxHashMap as HashMap;

impl<M: Module> CraneliftCodegen<M> {
    /// Increfs a borrowed `PyObject` element; the container decrefs it on drop.
    fn translate_aggregate_elem(
        func_mir: &MirFunction,
        builder: &mut FunctionBuilder,
        vars: &HashMap<Local, Variable>,
        string_ids: &HashMap<String, DataId>,
        module: &mut M,
        func_ids: &HashMap<String, FuncId>,
        op: &Operand,
    ) -> Value {
        let val = Self::translate_operand(builder, op, vars, string_ids, module, func_ids);
        if let Operand::Copy(src) = op
            && matches!(func_mir.locals[src.0].ty, OliveType::PyObject)
        {
            let copy_ref_id = func_ids
                .get("__olive_py_copy_ref")
                .expect("missing __olive_py_copy_ref");
            let local_func = module.declare_func_in_func(*copy_ref_id, builder.func);
            let inst = builder.ins().call(local_func, &[val]);
            return builder.inst_results(inst)[0];
        }
        if builder.func.dfg.value_type(val) == types::F64 {
            return builder.ins().bitcast(types::I64, MemFlags::new(), val);
        }
        val
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_aggregate(
        func_mir: &MirFunction,
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
                    let key = Self::translate_aggregate_elem(
                        func_mir, builder, vars, string_ids, module, func_ids, &ops[i],
                    );
                    let val = Self::translate_aggregate_elem(
                        func_mir,
                        builder,
                        vars,
                        string_ids,
                        module,
                        func_ids,
                        &ops[i + 1],
                    );
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
                    let val = Self::translate_aggregate_elem(
                        func_mir, builder, vars, string_ids, module, func_ids, op,
                    );
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
                    let val = Self::translate_aggregate_elem(
                        func_mir, builder, vars, string_ids, module, func_ids, op,
                    );
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
                    let val = Self::translate_aggregate_elem(
                        func_mir, builder, vars, string_ids, module, func_ids, op,
                    );
                    builder
                        .ins()
                        .store(MemFlags::trusted(), val, data_ptr, (i * 8) as i32);
                }
                list_ptr
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::{call_i64, call_i64_2, compile};

    #[test]
    fn test_translate_aggregate_tuple() {
        let mut cg =
            compile("fn f(a: i64, b: i64) -> i64:\n    let t = (a, b)\n    return t[0] + t[1]\n");
        assert_eq!(call_i64_2(&mut cg, "f", 10, 32), 42);
    }

    #[test]
    fn test_translate_aggregate_list() {
        let _cg = compile("fn f() -> i64:\n    let xs = [1, 2, 3]\n    return len(xs)\n");
    }

    #[test]
    fn test_translate_aggregate_empty_list() {
        let _cg = compile("fn f() -> i64:\n    let xs = []\n    return len(xs)\n");
    }

    #[test]
    fn test_translate_aggregate_struct_construction() {
        let mut cg = compile(
            "struct Point:\n    x: i64\n    y: i64\n\nfn f() -> i64:\n    let p = Point(10, 32)\n    return p.x + p.y\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn test_heterogeneous_any_list_ints() {
        let mut cg = compile(
            "fn f() -> i64:\n    let xs: [Any] = [10, 2.5, 32]\n    return int(xs[0]) + int(xs[2])\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn test_heterogeneous_any_list_float_roundtrip() {
        // A float boxed into `[Any]` unboxes back to its value.
        let mut cg = compile(
            "fn f() -> i64:\n    let xs: [Any] = [1.5, 7]\n    return int(float(xs[0]) + 40.5)\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn test_any_null_distinct_from_zero() {
        // A boxed `null` in an `Any` slot is not equal to the integer zero.
        let mut cg = compile(
            "fn f() -> i64:\n    let xs: [Any] = [None, 0]\n    let mut r = 0\n    if xs[0] == None:\n        r = r + 1\n    if xs[1] == None:\n        r = r + 10\n    return r\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 1);
    }

    #[test]
    fn test_any_boxed_bool_truthiness_in_and() {
        // `and`/`or` branch on the truthiness of a boxed `Any` bool, not its
        // raw pointer word.
        let mut cg = compile(
            "fn f() -> i64:\n    let xs: [Any] = [True, False]\n    if xs[0]:\n        return 42\n    return 0\n",
        );
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }
}
