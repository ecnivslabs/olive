use super::CraneliftCodegen;
use super::imports::map_builtin_to_runtime;
use super::translate::truncate_for_store;
use crate::mir::{Constant, Local, MirFunction, Operand};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::FxHashMap as HashMap;

impl<M: Module> CraneliftCodegen<M> {
    /// Whether the callee's declared parameter at `arg_index` is a float, read
    /// from its signature so float args reach float-typed builtins intact.
    fn callee_param_is_float(
        builder: &FunctionBuilder,
        callee: cranelift::codegen::ir::FuncRef,
        arg_index: usize,
        has_sret: bool,
    ) -> bool {
        let sig = builder.func.dfg.ext_funcs[callee].signature;
        let pidx = arg_index + if has_sret { 1 } else { 0 };
        builder.func.dfg.signatures[sig]
            .params
            .get(pidx)
            .is_some_and(|p| p.value_type == types::F64 || p.value_type == types::F32)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_call(
        func_mir: &MirFunction,
        module: &mut M,
        func_ids: &HashMap<String, FuncId>,
        string_ids: &HashMap<String, DataId>,
        struct_fields: &HashMap<String, Vec<String>>,
        field_types: &HashMap<(String, String), OliveType>,
        enum_defs: &HashMap<String, Vec<(String, Vec<OliveType>)>>,
        c_struct_offsets: &HashMap<String, Vec<super::FfiStructFieldLayout>>,
        c_struct_sizes: &HashMap<String, i64>,
        ffi_vararg_ptrs: &HashMap<String, *const u8>,
        ffi_vararg_ids: &std::collections::HashSet<String>,
        ffi_entries: &[super::FfiFnEntry],
        dispatch_ids: &HashMap<String, DataId>,
        builder: &mut FunctionBuilder,
        vars: &HashMap<Local, Variable>,
        func: &Operand,
        args: &[Operand],
    ) -> Value {
        let call_args: Vec<Value> = args
            .iter()
            .map(|a| Self::translate_operand(builder, a, vars, string_ids, module, func_ids))
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

            let arg_type = if !args.is_empty() {
                match &args[0] {
                    Operand::Constant(Constant::Str(_)) => OliveType::Str,
                    Operand::Constant(Constant::Float(_)) => OliveType::Float,
                    Operand::Constant(Constant::Bool(_)) => OliveType::Bool,
                    Operand::Copy(l) | Operand::Move(l) => func_mir.locals[l.0].ty.clone(),
                    _ => OliveType::Int,
                }
            } else {
                OliveType::Int
            };
            // All Python-valued types (PyNamed, Union containing PyNamed/PyObject, etc.) behave
            // identically at the runtime level -- they are opaque CPython object pointers.
            // Normalizing to PyObject here ensures builtin dispatch (list, dict, len, ...) selects
            // the correct __olive_py_* helper regardless of the static named type.
            let arg_type = if arg_type.is_py_value() {
                OliveType::PyObject
            } else {
                arg_type
            };

            if (name == "print" || name == "str")
                && call_args.len() == 1
                && super::imports::needs_type_descriptor(&arg_type)
            {
                let desc = super::imports::type_descriptor(
                    &arg_type,
                    struct_fields,
                    field_types,
                    enum_defs,
                );
                let data_id = *string_ids
                    .get(&desc)
                    .expect("type descriptor not interned during collection");
                let local_data = module.declare_data_in_func(data_id, builder.func);
                let desc_ptr = builder.ins().symbol_value(types::I64, local_data);
                let sym = if name == "print" {
                    "__olive_print_typed"
                } else {
                    "__olive_format_typed"
                };
                let func_id = func_ids[sym];
                let local_func = module.declare_func_in_func(func_id, builder.func);
                let inst = builder.ins().call(local_func, &[call_args[0], desc_ptr]);
                return builder.inst_results(inst)[0];
            }

            // Copy-on-escape: deep-copy a borrowed value so the container it
            // enters owns an independent one. The descriptor comes from the
            // argument's static type, like the typed print/free paths.
            if name == "__olive_copy_typed" && call_args.len() == 1 {
                let desc = super::imports::type_descriptor(
                    &arg_type,
                    struct_fields,
                    field_types,
                    enum_defs,
                );
                let data_id = *string_ids
                    .get(&desc)
                    .expect("copy descriptor not interned during collection");
                let local_data = module.declare_data_in_func(data_id, builder.func);
                let desc_ptr = builder.ins().symbol_value(types::I64, local_data);
                let func_id = func_ids["__olive_copy_typed"];
                let local_func = module.declare_func_in_func(func_id, builder.func);
                let inst = builder.ins().call(local_func, &[call_args[0], desc_ptr]);
                return builder.inst_results(inst)[0];
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
                || name == "len"
                || name == "list"
                || name == "dict"
                || name == "keys"
                || name == "values"
                || (name == "sum" && !func_ids.contains_key("sum"))
                || (name == "min" && !func_ids.contains_key("min"))
                || (name == "max" && !func_ids.contains_key("max"))
                || name == "remove")
                && !args.is_empty()
            {
                map_builtin_to_runtime(name, &arg_type).unwrap_or(name.as_str())
            } else if name == "ffi_errno" {
                "__olive_ffi_errno"
            } else if name == "realize" {
                "__olive_py_realize"
            } else {
                name.as_str()
            };

            if ((resolved_name == "__olive_int" && arg_type == OliveType::Int)
                || (resolved_name == "__olive_copy_float"
                    && (arg_type == OliveType::Float || arg_type == OliveType::F32)))
                && !call_args.is_empty()
            {
                return call_args[0];
            }

            let is_ffi = resolved_name.contains("::") && !resolved_name.starts_with("__olive");

            if let Some(&func_id) = func_ids.get(resolved_name) {
                let is_aot_vararg = ffi_vararg_ids.contains(resolved_name);
                let local_func = module.declare_func_in_func(func_id, builder.func);
                let mut final_args = Vec::new();
                let mut sret_ptr = None;
                let is_builtin = resolved_name.starts_with("__olive") || resolved_name == "print";
                let ffi_entry = ffi_entries.iter().find(|e| e.jit_name == resolved_name);

                if let Some(entry) = ffi_entry
                    && entry.use_sret
                {
                    let ret_name = entry.ret.as_ref().unwrap();
                    let size = *c_struct_sizes.get(ret_name).unwrap();
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        size as u32,
                        3,
                    ));
                    let ptr = builder
                        .ins()
                        .stack_addr(module.isa().pointer_type(), slot, 0);
                    final_args.push(ptr);
                    sret_ptr = Some(ptr);
                }

                for (i, &arg) in call_args.iter().enumerate() {
                    let is_str_arg = args.get(i).is_some_and(|op| match op {
                        Operand::Constant(Constant::Str(_)) => true,
                        Operand::Copy(l) | Operand::Move(l) => {
                            matches!(func_mir.locals[l.0].ty, OliveType::Str)
                        }
                        _ => false,
                    });

                    if is_ffi
                        && let Some(entry) = ffi_entry
                        && i < entry.params.len()
                    {
                        let p_type = &entry.params[i];
                        if let Some(layout) = c_struct_offsets.get(p_type) {
                            let size = *c_struct_sizes.get(p_type).unwrap_or(&8);
                            let is_windows = cfg!(target_os = "windows");
                            if is_windows {
                                if size == 1 || size == 2 || size == 4 || size == 8 {
                                    let ty = match size {
                                        1 => types::I8,
                                        2 => types::I16,
                                        4 => types::I32,
                                        _ => types::I64,
                                    };
                                    let val = builder.ins().load(ty, MemFlags::trusted(), arg, 0);
                                    final_args.push(val);
                                } else {
                                    let slot = builder.create_sized_stack_slot(StackSlotData::new(
                                        StackSlotKind::ExplicitSlot,
                                        size as u32,
                                        3,
                                    ));
                                    let stack_ptr = builder.ins().stack_addr(
                                        module.isa().pointer_type(),
                                        slot,
                                        0,
                                    );
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
                                        builder.ins().store(
                                            MemFlags::trusted(),
                                            val,
                                            stack_ptr,
                                            *offset,
                                        );
                                    }
                                    final_args.push(stack_ptr);
                                }
                            } else {
                                if size <= 8 {
                                    let has_float = layout.iter().any(|(_, _, ty_name, _)| {
                                        ty_name == "float" || ty_name == "f32" || ty_name == "f64"
                                    });
                                    let ty = if has_float {
                                        if size <= 4 { types::F32 } else { types::F64 }
                                    } else {
                                        if size <= 1 {
                                            types::I8
                                        } else if size <= 2 {
                                            types::I16
                                        } else if size <= 4 {
                                            types::I32
                                        } else {
                                            types::I64
                                        }
                                    };
                                    let val = builder.ins().load(ty, MemFlags::trusted(), arg, 0);
                                    final_args.push(val);
                                } else if size <= 16 {
                                    let first_has_float =
                                        layout.iter().any(|(_, offset, ty_name, _)| {
                                            *offset < 8
                                                && (ty_name == "float"
                                                    || ty_name == "f32"
                                                    || ty_name == "f64")
                                        });
                                    let second_has_float =
                                        layout.iter().any(|(_, offset, ty_name, _)| {
                                            *offset >= 8
                                                && (ty_name == "float"
                                                    || ty_name == "f32"
                                                    || ty_name == "f64")
                                        });

                                    let first_ty = if first_has_float {
                                        types::F64
                                    } else {
                                        types::I64
                                    };
                                    let second_ty = if second_has_float {
                                        types::F64
                                    } else {
                                        types::I64
                                    };

                                    let val1 =
                                        builder.ins().load(first_ty, MemFlags::trusted(), arg, 0);
                                    let val2 =
                                        builder.ins().load(second_ty, MemFlags::trusted(), arg, 8);

                                    final_args.push(val1);
                                    final_args.push(val2);
                                } else {
                                    let slot = builder.create_sized_stack_slot(StackSlotData::new(
                                        StackSlotKind::ExplicitSlot,
                                        size as u32,
                                        3,
                                    ));
                                    let stack_ptr = builder.ins().stack_addr(
                                        module.isa().pointer_type(),
                                        slot,
                                        0,
                                    );
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
                                        builder.ins().store(
                                            MemFlags::trusted(),
                                            val,
                                            stack_ptr,
                                            *offset,
                                        );
                                    }
                                    final_args.push(stack_ptr);
                                }
                            }
                            continue;
                        }
                    }

                    if (ffi_entry.is_some() || is_aot_vararg) && is_str_arg {
                        final_args.push(builder.ins().band_imm(arg, -2));
                    } else if is_builtin && builder.func.dfg.value_type(arg) == types::F64 {
                        let expects_float = if let Some(entry) = ffi_entry {
                            if i < entry.params.len() {
                                entry.params[i] == "float"
                                    || entry.params[i] == "f64"
                                    || entry.params[i] == "f32"
                            } else {
                                false
                            }
                        } else {
                            Self::callee_param_is_float(builder, local_func, i, sret_ptr.is_some())
                        };
                        if !expects_float {
                            final_args.push(builder.ins().bitcast(
                                types::I64,
                                MemFlags::new(),
                                arg,
                            ));
                        } else {
                            final_args.push(arg);
                        }
                    } else if is_builtin && builder.func.dfg.value_type(arg) == types::F32 {
                        let expects_float = if let Some(entry) = ffi_entry {
                            if i < entry.params.len() {
                                entry.params[i] == "float"
                                    || entry.params[i] == "f64"
                                    || entry.params[i] == "f32"
                            } else {
                                false
                            }
                        } else {
                            Self::callee_param_is_float(builder, local_func, i, sret_ptr.is_some())
                        };

                        if !expects_float {
                            let bitcast_val =
                                builder.ins().bitcast(types::I32, MemFlags::new(), arg);
                            final_args.push(builder.ins().uextend(types::I64, bitcast_val));
                        } else {
                            final_args.push(arg);
                        }
                    } else {
                        let arg_ty = builder.func.dfg.value_type(arg);
                        if arg_ty == types::F64 || arg_ty == types::F32 {
                            let local_sig = builder.func.dfg.ext_funcs[local_func].signature;
                            let params = &builder.func.dfg.signatures[local_sig].params;
                            let param_idx = final_args.len();
                            let expected = params.get(param_idx).map(|p| p.value_type);
                            if expected == Some(types::I64) {
                                if arg_ty == types::F64 {
                                    final_args.push(builder.ins().bitcast(
                                        types::I64,
                                        MemFlags::new(),
                                        arg,
                                    ));
                                } else {
                                    let i32_val =
                                        builder.ins().bitcast(types::I32, MemFlags::new(), arg);
                                    final_args.push(builder.ins().uextend(types::I64, i32_val));
                                }
                            } else {
                                final_args.push(arg);
                            }
                        } else {
                            final_args.push(arg);
                        }
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
                } else if let Some(&cell_id) = dispatch_ids.get(resolved_name) {
                    // Route through the function's dispatch cell instead of calling
                    // `local_func` directly, so a future tier-up recompiler can retarget
                    // the cell without touching this call site. Reuses `local_func`'s
                    // already-registered signature; only the callee address changes.
                    let local_cell = module.declare_data_in_func(cell_id, builder.func);
                    let cell_ptr = builder.ins().symbol_value(types::I64, local_cell);
                    let target = builder
                        .ins()
                        .load(types::I64, MemFlags::trusted(), cell_ptr, 0);
                    let sig_ref = builder.func.dfg.ext_funcs[local_func].signature;
                    builder.ins().call_indirect(sig_ref, target, &final_args)
                } else {
                    builder.ins().call(local_func, &final_args)
                };

                // Capture errno before any runtime allocation below can clobber
                // it, but only when the program actually reads ffi_errno.
                if is_ffi && let Some(&snap_id) = func_ids.get("__olive_ffi_snapshot_errno") {
                    let snap = module.declare_func_in_func(snap_id, builder.func);
                    builder.ins().call(snap, &[]);
                }

                let mut ret_val = if let Some(ptr) = sret_ptr {
                    ptr
                } else {
                    let results = builder.inst_results(inst).to_vec();
                    if is_ffi
                        && let Some(entry) = ffi_entry
                        && let Some(ref r) = entry.ret
                        && c_struct_sizes.contains_key(r)
                    {
                        let size = *c_struct_sizes.get(r).unwrap_or(&8);
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            size as u32,
                            3,
                        ));
                        let stack_ptr =
                            builder
                                .ins()
                                .stack_addr(module.isa().pointer_type(), slot, 0);
                        if results.len() == 1 {
                            builder
                                .ins()
                                .store(MemFlags::trusted(), results[0], stack_ptr, 0);
                        } else if results.len() == 2 {
                            builder
                                .ins()
                                .store(MemFlags::trusted(), results[0], stack_ptr, 0);
                            builder
                                .ins()
                                .store(MemFlags::trusted(), results[1], stack_ptr, 8);
                        }
                        stack_ptr
                    } else {
                        if results.is_empty() {
                            builder.ins().iconst(types::I64, 0)
                        } else {
                            results[0]
                        }
                    }
                };

                if is_ffi
                    && let Some(entry) = ffi_entry
                    && let Some(ref r) = entry.ret
                    && r == "str"
                {
                    ret_val = builder.ins().bor_imm(ret_val, 1);
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
                    if i < n_fixed
                        && let Some(e) = entry
                    {
                        let declared_ty = super::ffi_cl_type(&e.params[i]);
                        let cooked = truncate_for_store(builder, cooked, &e.params[i]);
                        sig.params.push(AbiParam::new(declared_ty));
                        vararg_args.push(cooked);
                        continue;
                    }

                    sig.params
                        .push(AbiParam::new(builder.func.dfg.value_type(cooked)));
                    vararg_args.push(cooked);
                }

                if let Some(e) = entry {
                    if let Some(ref r) = e.ret
                        && r != "void"
                    {
                        sig.returns.push(AbiParam::new(super::ffi_cl_type(r)));
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
                if let Some(e) = entry
                    && e.ret.as_deref() == Some("str")
                {
                    ret_val = builder.ins().bor_imm(ret_val, 1);
                }
                return ret_val;
            }

            if is_ffi {
                panic!(
                    "FFI symbol `{resolved_name}` could not be resolved: the native function was \
                     declared but its library exported no such symbol. Check the `native import` \
                     library path and the exact symbol name."
                );
            }
            panic!(
                "internal error: call to unregistered runtime function `{resolved_name}` \
                 (no matching builtin, FFI entry, or compiled function). This is a compiler bug."
            );
        } else {
            let fn_ptr_val =
                Self::translate_operand(builder, func, vars, string_ids, module, func_ids);

            let mut sig = module.make_signature();
            sig.call_conv = module.isa().default_call_conv();

            for &a in &call_args {
                sig.params
                    .push(AbiParam::new(builder.func.dfg.value_type(a)));
            }
            sig.returns.push(AbiParam::new(types::I64));

            let sig_ref = builder.import_signature(sig);
            let inst = builder.ins().call_indirect(sig_ref, fn_ptr_val, &call_args);
            let results = builder.inst_results(inst);

            if results.is_empty() {
                builder.ins().iconst(types::I64, 0)
            } else {
                results[0]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::{call_i64, call_i64_1, call_i64_2, compile};

    #[test]
    fn test_translate_call_zero_arg() {
        let mut cg = compile("fn f() -> i64:\n    return 42\n");
        assert_eq!(call_i64(&mut cg, "f"), 42);
    }

    #[test]
    fn test_translate_call_one_arg() {
        let mut cg = compile("fn double(x: i64) -> i64:\n    return x * 2\n");
        assert_eq!(call_i64_1(&mut cg, "double", 21), 42);
    }

    #[test]
    fn test_translate_call_multi_arg() {
        let mut cg = compile("fn add(a: i64, b: i64) -> i64:\n    return a + b\n");
        assert_eq!(call_i64_2(&mut cg, "add", 10, 32), 42);
    }

    #[test]
    fn test_translate_call_nested() {
        let mut cg = compile(
            "fn add(a: i64, b: i64) -> i64:\n    return a + b\n\nfn f(x: i64) -> i64:\n    return add(x, add(x, x))\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 14), 42);
    }

    #[test]
    fn test_translate_call_multiple_functions() {
        let mut cg = compile(
            "fn inc(x: i64) -> i64:\n    return x + 1\n\nfn dec(x: i64) -> i64:\n    return x - 1\n\nfn f(x: i64) -> i64:\n    return inc(dec(x))\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 42), 42);
    }

    #[test]
    fn test_translate_call_recursive() {
        let mut cg = compile(
            "fn fact(n: i64) -> i64:\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n",
        );
        assert_eq!(call_i64_1(&mut cg, "fact", 5), 120);
    }
}
