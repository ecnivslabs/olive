use super::CraneliftCodegen;
use crate::mir::ir::AggregateKind;
use crate::mir::{Local, MirFunction, Operand};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::FxHashMap as HashMap;

impl<M: Module> CraneliftCodegen<M> {
    /// Increfs a borrowed `PyObject` element; the container decrefs it on drop.
    pub(super) fn translate_aggregate_elem(
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
            && func_mir.locals[src.0].ty.is_py_value()
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
        struct_fields: &HashMap<String, Vec<String>>,
        field_types: &HashMap<(String, String), OliveType>,
        enum_defs: &HashMap<String, Vec<(String, Vec<OliveType>)>>,
        kind: &AggregateKind,
        ops: &[Operand],
        reuse: Option<(Local, Value, bool)>,
    ) -> Value {
        match kind {
            AggregateKind::Dict => {
                let dict_ptr = if let Some((_, reuse_val, has_borrow)) = reuse {
                    let new_id = func_ids.get("__olive_dict_new_reuse").unwrap();
                    let new_func = module.declare_func_in_func(*new_id, builder.func);
                    let bump_val = builder
                        .ins()
                        .iconst(types::I64, if has_borrow { 1 } else { 0 });
                    let inst = builder.ins().call(new_func, &[reuse_val, bump_val]);
                    builder.inst_results(inst)[0]
                } else {
                    let new_id = func_ids
                        .get("__olive_obj_new")
                        .expect("missing __olive_obj_new");
                    let new_func = module.declare_func_in_func(*new_id, builder.func);
                    let inst = builder.ins().call(new_func, &[]);
                    builder.inst_results(inst)[0]
                };

                // A struct/enum/tuple/collection key needs the same
                // structural hash+eq `==` derives; every key literal shares
                // one static type, so the descriptor is built once.
                let key_desc_ptr = ops.first().and_then(|first_key| {
                    let key_ty = super::imports::operand_static_type(first_key, func_mir);
                    super::imports::needs_structural_key(&key_ty).then(|| {
                        let desc = super::imports::type_descriptor(
                            &key_ty,
                            struct_fields,
                            field_types,
                            enum_defs,
                        );
                        let data_id = *string_ids
                            .get(&desc)
                            .expect("dict key descriptor not interned during collection");
                        let local_data = module.declare_data_in_func(data_id, builder.func);
                        builder.ins().symbol_value(types::I64, local_data)
                    })
                });
                let set_id = func_ids
                    .get(if key_desc_ptr.is_some() {
                        "__olive_obj_set_typed"
                    } else {
                        "__olive_obj_set"
                    })
                    .expect("missing obj_set variant");
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
                    match key_desc_ptr {
                        Some(desc_ptr) => {
                            builder
                                .ins()
                                .call(set_func, &[dict_ptr, key, val, desc_ptr]);
                        }
                        None => {
                            builder.ins().call(set_func, &[dict_ptr, key, val]);
                        }
                    }
                }
                dict_ptr
            }
            AggregateKind::EnumVariant(type_id, tag) => {
                let type_id_val = builder.ins().iconst(types::I64, *type_id);
                let tag_val = builder.ins().iconst(types::I64, *tag as i64);
                let count = builder.ins().iconst(types::I64, ops.len() as i64);
                let enum_ptr = if let Some((_, reuse_val, has_borrow)) = reuse {
                    let new_id = func_ids.get("__olive_enum_new_reuse").unwrap();
                    let new_func = module.declare_func_in_func(*new_id, builder.func);
                    let bump_val = builder
                        .ins()
                        .iconst(types::I64, if has_borrow { 1 } else { 0 });
                    let inst = builder.ins().call(
                        new_func,
                        &[reuse_val, type_id_val, tag_val, count, bump_val],
                    );
                    builder.inst_results(inst)[0]
                } else {
                    let new_id = func_ids
                        .get("__olive_enum_new")
                        .expect("missing __olive_enum_new");
                    let new_func = module.declare_func_in_func(*new_id, builder.func);
                    let inst = builder.ins().call(new_func, &[type_id_val, tag_val, count]);
                    builder.inst_results(inst)[0]
                };

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
                let set_ptr = if let Some((_, reuse_val, has_borrow)) = reuse {
                    let new_id = func_ids.get("__olive_set_new_reuse").unwrap();
                    let new_func = module.declare_func_in_func(*new_id, builder.func);
                    let bump_val = builder
                        .ins()
                        .iconst(types::I64, if has_borrow { 1 } else { 0 });
                    let inst = builder.ins().call(new_func, &[reuse_val, count, bump_val]);
                    builder.inst_results(inst)[0]
                } else {
                    let new_id = func_ids
                        .get("__olive_set_new")
                        .expect("missing __olive_set_new");
                    let new_func = module.declare_func_in_func(*new_id, builder.func);
                    let inst = builder.ins().call(new_func, &[count]);
                    builder.inst_results(inst)[0]
                };

                // Same reasoning as the dict key descriptor above, keyed off
                // the element type instead.
                let elem_desc_ptr = ops.first().and_then(|first_elem| {
                    let elem_ty = super::imports::operand_static_type(first_elem, func_mir);
                    super::imports::needs_structural_key(&elem_ty).then(|| {
                        let desc = super::imports::type_descriptor(
                            &elem_ty,
                            struct_fields,
                            field_types,
                            enum_defs,
                        );
                        let data_id = *string_ids
                            .get(&desc)
                            .expect("set element descriptor not interned during collection");
                        let local_data = module.declare_data_in_func(data_id, builder.func);
                        builder.ins().symbol_value(types::I64, local_data)
                    })
                });
                let add_id = func_ids
                    .get(if elem_desc_ptr.is_some() {
                        "__olive_set_add_typed"
                    } else {
                        "__olive_set_add"
                    })
                    .expect("missing set_add variant");
                let add_func = module.declare_func_in_func(*add_id, builder.func);

                for op in ops {
                    let val = Self::translate_aggregate_elem(
                        func_mir, builder, vars, string_ids, module, func_ids, op,
                    );
                    match elem_desc_ptr {
                        Some(desc_ptr) => {
                            builder.ins().call(add_func, &[set_ptr, val, desc_ptr]);
                        }
                        None => {
                            builder.ins().call(add_func, &[set_ptr, val]);
                        }
                    }
                }
                set_ptr
            }
            AggregateKind::FatPtr => {
                let alloc_id = func_ids
                    .get("__olive_fatptr_alloc")
                    .expect("missing __olive_fatptr_alloc");
                let alloc_func = module.declare_func_in_func(*alloc_id, builder.func);
                // Slab record: [kind, data ptr, vtable ptr, drop-shim ptr,
                // concrete descriptor ptr]; the runtime writes the kind word so
                // free paths classify it, and the descriptor lets the copy path
                // deep-copy the erased value.
                let inst = builder.ins().call(alloc_func, &[]);
                let ptr = builder.inst_results(inst)[0];

                let data_val =
                    Self::translate_operand(builder, &ops[0], vars, string_ids, module, func_ids);
                let vtable_val =
                    Self::translate_operand(builder, &ops[1], vars, string_ids, module, func_ids);
                let drop_shim_val =
                    Self::translate_operand(builder, &ops[2], vars, string_ids, module, func_ids);
                let desc_val =
                    Self::translate_operand(builder, &ops[3], vars, string_ids, module, func_ids);

                builder.ins().store(MemFlags::trusted(), data_val, ptr, 8);
                builder
                    .ins()
                    .store(MemFlags::trusted(), vtable_val, ptr, 16);
                builder
                    .ins()
                    .store(MemFlags::trusted(), drop_shim_val, ptr, 24);
                builder.ins().store(MemFlags::trusted(), desc_val, ptr, 32);
                ptr
            }
            _ => {
                let n = ops.len() as i64;
                let n_val = builder.ins().iconst(types::I64, n);
                let list_ptr = if let Some((_, reuse_val, has_borrow)) = reuse {
                    let new_id = func_ids.get("__olive_list_new_reuse").unwrap();
                    let new_func = module.declare_func_in_func(*new_id, builder.func);
                    let bump_val = builder
                        .ins()
                        .iconst(types::I64, if has_borrow { 1 } else { 0 });
                    let inst = builder.ins().call(new_func, &[reuse_val, n_val, bump_val]);
                    builder.inst_results(inst)[0]
                } else {
                    let new_id = func_ids
                        .get("__olive_list_new")
                        .expect("missing __olive_list_new");
                    let new_func = module.declare_func_in_func(*new_id, builder.func);
                    let inst = builder.ins().call(new_func, &[n_val]);
                    builder.inst_results(inst)[0]
                };

                // KIND_LIST=1 (raw concrete elements) vs KIND_ANY_LIST=15 (inline Any-tagged).
                // The Python proxy reads this to choose the right element decoder.
                let is_any_list = matches!(kind, AggregateKind::List)
                    && ops.iter().any(|op| {
                        matches!(
                            op,
                            Operand::Copy(l) | Operand::Move(l)
                                if matches!(func_mir.locals[l.0].ty, OliveType::Any)
                        )
                    });
                if is_any_list {
                    let any_kind = builder.ins().iconst(types::I64, 15_i64);
                    builder
                        .ins()
                        .store(MemFlags::trusted(), any_kind, list_ptr, 0);
                }

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
