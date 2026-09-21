use super::CraneliftCodegen;
use super::imports::map_builtin_to_runtime;
use crate::mir::{Constant, Local, MirFunction, Operand};
use crate::semantic::types::Type as OliveType;
use cranelift::codegen::ir::BlockArg;
use cranelift::prelude::*;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::FxHashMap as HashMap;

/// Both low tag bits of an Olive string word (`STR_TAG | STR_HEAP`, see
/// `str_body` in the runtime and `STR_TAG_BITS` in `translate_rvalue`).
/// A foreign `char*` argument must clear both: clearing only bit 0 hands a
/// heap string to C two bytes past its body.
const STR_TAG_BITS: i64 = 3;

fn vararg_code(declared: Option<&str>, static_ty: Option<&OliveType>, fixed: bool) -> i64 {
    if fixed && let Some(name) = declared {
        return match name {
            "f32" => 2,
            "float" | "f64" => 1,
            "i8" | "u8" | "bool" => 5,
            "i16" | "u16" => 4,
            "i32" | "u32" => 3,
            "str" | "ptr" => 6,
            "void" => 7,
            _ => 0,
        };
    }
    match static_ty {
        Some(OliveType::F32 | OliveType::Float) => 1,
        Some(OliveType::I8 | OliveType::U8 | OliveType::Bool) => 3,
        Some(OliveType::I16 | OliveType::U16) => 3,
        Some(OliveType::I32 | OliveType::U32) => 3,
        Some(OliveType::Str | OliveType::Ptr(_)) => 6,
        _ => 0,
    }
}

fn vararg_word(
    builder: &mut FunctionBuilder,
    value: Value,
    code: i64,
    static_ty: Option<&OliveType>,
) -> Value {
    let value_ty = builder.func.dfg.value_type(value);
    match code {
        1 => {
            let f64_value = if value_ty == types::I64 && matches!(static_ty, Some(OliveType::F32)) {
                let low = builder.ins().ireduce(types::I32, value);
                let f32_value = builder.ins().bitcast(types::F32, MemFlags::new(), low);
                builder.ins().fpromote(types::F64, f32_value)
            } else if value_ty == types::F32 {
                builder.ins().fpromote(types::F64, value)
            } else if value_ty == types::I64 {
                builder.ins().bitcast(types::F64, MemFlags::new(), value)
            } else {
                value
            };
            if builder.func.dfg.value_type(f64_value) == types::F64 {
                builder
                    .ins()
                    .bitcast(types::I64, MemFlags::new(), f64_value)
            } else {
                value
            }
        }
        2 => {
            let f32_value = if value_ty == types::I64 {
                let low = builder.ins().ireduce(types::I32, value);
                builder.ins().bitcast(types::F32, MemFlags::new(), low)
            } else if value_ty == types::F64 {
                builder.ins().fdemote(types::F32, value)
            } else {
                value
            };
            let bits = builder
                .ins()
                .bitcast(types::I32, MemFlags::new(), f32_value);
            builder.ins().uextend(types::I64, bits)
        }
        3..=5 => {
            let reduced = match code {
                3 => builder.ins().ireduce(types::I32, value),
                4 => builder.ins().ireduce(types::I16, value),
                _ => builder.ins().ireduce(types::I8, value),
            };
            builder.ins().sextend(types::I64, reduced)
        }
        _ => value,
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_vararg_runtime_call<M: Module>(
    builder: &mut FunctionBuilder,
    module: &mut M,
    func_ids: &HashMap<String, FuncId>,
    fn_ptr: Value,
    entry: Option<&super::FfiFnEntry>,
    args: &[Operand],
    values: &[Value],
    func_mir: &MirFunction,
) -> Value {
    let n = values.len();
    let slot_size = (n * 8).max(8) as u32;
    let type_slot = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        slot_size,
        3,
    ));
    let value_slot = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        slot_size,
        3,
    ));
    let type_ptr = builder
        .ins()
        .stack_addr(module.isa().pointer_type(), type_slot, 0);
    let value_ptr = builder
        .ins()
        .stack_addr(module.isa().pointer_type(), value_slot, 0);
    for (i, &value) in values.iter().enumerate() {
        let static_ty = args.get(i).map(|op| {
            super::imports::concrete_ty(&super::imports::operand_static_type(op, func_mir)).clone()
        });
        let fixed = entry.is_some_and(|e| i < e.params.len());
        let declared = entry.and_then(|e| e.params.get(i).map(String::as_str));
        let code = vararg_code(declared, static_ty.as_ref(), fixed);
        let value = if code == 6 && matches!(static_ty, Some(OliveType::Str)) {
            builder.ins().band_imm(value, !STR_TAG_BITS)
        } else {
            value
        };
        let word = vararg_word(builder, value, code, static_ty.as_ref());
        let code_value = builder.ins().iconst(types::I64, code);
        builder
            .ins()
            .store(MemFlags::trusted(), code_value, type_ptr, (i * 8) as i32);
        builder
            .ins()
            .store(MemFlags::trusted(), word, value_ptr, (i * 8) as i32);
    }
    let n_fixed = entry.map(|e| e.n_fixed).unwrap_or(0);
    let n_total = builder.ins().iconst(types::I64, n as i64);
    let n_fixed_value = builder.ins().iconst(types::I64, n_fixed as i64);
    let ret_code = entry
        .and_then(|e| e.ret.as_deref())
        .map(|name| vararg_code(Some(name), None, true))
        .unwrap_or(0);
    let ret_code_value = builder.ins().iconst(types::I64, ret_code);
    let helper_id = func_ids
        .get("__olive_vararg_call_ex")
        .expect("missing __olive_vararg_call_ex");
    let helper = module.declare_func_in_func(*helper_id, builder.func);
    let call = builder.ins().call(
        helper,
        &[
            fn_ptr,
            n_fixed_value,
            n_total,
            type_ptr,
            value_ptr,
            ret_code_value,
        ],
    );
    let mut result = builder.inst_results(call)[0];
    match entry.and_then(|e| e.ret.as_deref()) {
        Some("f32") => {
            let bits = builder.ins().ireduce(types::I32, result);
            result = builder.ins().bitcast(types::F32, MemFlags::new(), bits);
        }
        Some("float") | Some("f64") => {
            result = builder.ins().bitcast(types::F64, MemFlags::new(), result);
        }
        Some("str") => result = builder.ins().bor_imm(result, 1),
        _ => {}
    }
    result
}

impl<M: Module> CraneliftCodegen<M> {
    /// Exact float width the callee's declared parameter expects, if any.
    /// Both f32 and f64 args land in the same XMM register class, so
    /// cranelift's call lowering never widens/narrows on its own -- passing
    /// an f32 value where the callee's signature declares an f64 param (or
    /// vice versa) used to reach the call unconverted, and the callee reads
    /// the wrong bit width straight out of the register.
    fn callee_param_float_ty(
        builder: &FunctionBuilder,
        callee: cranelift::codegen::ir::FuncRef,
        arg_index: usize,
        has_sret: bool,
    ) -> Option<types::Type> {
        let sig = builder.func.dfg.ext_funcs[callee].signature;
        let pidx = arg_index + if has_sret { 1 } else { 0 };
        builder.func.dfg.signatures[sig]
            .params
            .get(pidx)
            .map(|p| p.value_type)
            .filter(|t| *t == types::F64 || *t == types::F32)
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
        dest_ty: &OliveType,
    ) -> Value {
        // The pushed element lands in a list slot, so it needs the same incref
        // and float bitcast a list literal's element would have gotten before
        // `ListAppend` folded the literal away. Translating an operand consumes
        // it (a `Move` zeroes its source), so each one is translated once.
        let push_elem = matches!(func, Operand::Constant(Constant::Function(n))
            if n == "__olive_list_push")
        .then_some(1);
        let call_args: Vec<Value> = args
            .iter()
            .enumerate()
            .map(|(i, a)| {
                if push_elem == Some(i) {
                    Self::translate_aggregate_elem(
                        func_mir, builder, vars, string_ids, module, func_ids, a,
                    )
                } else {
                    Self::translate_operand(builder, a, vars, string_ids, module, func_ids)
                }
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

            let arg_type = if !args.is_empty() {
                super::imports::operand_static_type(&args[0], func_mir)
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

            if call_args.len() == 1 && super::imports::needs_type_descriptor(&arg_type) {
                let use_typed = name == "print" || name == "__olive_write_any";
                if use_typed || name == "str" {
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
                    let sym = if name == "print" || name == "__olive_write_any" {
                        if name == "print" {
                            "__olive_print_typed"
                        } else {
                            "__olive_write_typed"
                        }
                    } else {
                        "__olive_format_typed"
                    };
                    let func_id = func_ids[sym];
                    let local_func = module.declare_func_in_func(func_id, builder.func);
                    let inst = builder.ins().call(local_func, &[call_args[0], desc_ptr]);
                    return builder.inst_results(inst)[0];
                }
            }

            // Copy-on-escape: deep-copy a borrowed value so the container it
            // enters owns an independent one. `__olive_relocate_typed`
            // (E5.6) is the same shape one level up: the same descriptor,
            // wrapped in the shared escape arena so the copy survives the
            // sending task completing. The descriptor comes from the
            // argument's static type, like the typed print/free paths --
            // except a closure record, whose descriptor is per-instance, not
            // per-type (see the `Drop` arm in translate.rs for why); it's
            // loaded back from the record itself at runtime instead.
            let is_copy_intrinsic =
                name == "__olive_copy_typed" || name == "__olive_relocate_typed";
            if is_copy_intrinsic
                && call_args.len() == 1
                && let OliveType::Fn(..) = super::imports::concrete_ty(&arg_type)
            {
                let val = call_args[0];
                let nz_bb = builder.create_block();
                let done_bb = builder.create_block();
                builder.append_block_param(done_bb, types::I64);
                let nonnull = builder.ins().icmp_imm(IntCC::NotEqual, val, 0);
                builder
                    .ins()
                    .brif(nonnull, nz_bb, &[], done_bb, &[BlockArg::Value(val)]);

                builder.seal_block(nz_bb);
                builder.switch_to_block(nz_bb);
                // See the analogous strip in translate.rs's `Drop` arm: the
                // record's `__desc` field is tagged like any other
                // `Constant::Str`, and must be untagged before use as a raw
                // descriptor pointer.
                let desc_tagged = builder.ins().load(types::I64, MemFlags::trusted(), val, 16);
                let desc_ptr = builder.ins().band_imm(desc_tagged, -2);
                let func_id = func_ids[name.as_str()];
                let local_func = module.declare_func_in_func(func_id, builder.func);
                let inst = builder.ins().call(local_func, &[val, desc_ptr]);
                let copied = builder.inst_results(inst)[0];
                builder.ins().jump(done_bb, &[BlockArg::Value(copied)]);

                builder.seal_block(done_bb);
                builder.switch_to_block(done_bb);
                return builder.block_params(done_bb)[0];
            }

            if is_copy_intrinsic && call_args.len() == 1 {
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
                let func_id = func_ids[name.as_str()];
                let local_func = module.declare_func_in_func(func_id, builder.func);
                let inst = builder.ins().call(local_func, &[call_args[0], desc_ptr]);
                return builder.inst_results(inst)[0];
            }

            // Derived structural `==`: descriptor comes from the left
            // operand's static type -- the checker already required both
            // sides to match before emitting this call.
            if name == "__olive_eq_typed" && call_args.len() == 2 {
                let desc = super::imports::type_descriptor(
                    &arg_type,
                    struct_fields,
                    field_types,
                    enum_defs,
                );
                let data_id = *string_ids
                    .get(&desc)
                    .expect("eq descriptor not interned during collection");
                let local_data = module.declare_data_in_func(data_id, builder.func);
                let desc_ptr = builder.ins().symbol_value(types::I64, local_data);
                let func_id = func_ids["__olive_eq_typed"];
                let local_func = module.declare_func_in_func(func_id, builder.func);
                let inst = builder
                    .ins()
                    .call(local_func, &[call_args[0], call_args[1], desc_ptr]);
                return builder.inst_results(inst)[0];
            }

            // Descriptor comes from the list arg's static type; needed to deep-copy elements.
            // `__olive_chan_send`/`__olive_mutex_unlock` take the sent value's
            // descriptor so a rejected (closed/null) value is released instead
            // of stranded.
            let desc_arg = match name.as_str() {
                "__olive_list_concat_typed" if call_args.len() == 2 => Some(0usize),
                "__olive_list_getslice_typed" if call_args.len() == 5 => Some(0usize),
                "__olive_list_repeat_typed" if call_args.len() == 2 => Some(0usize),
                "__olive_list_extend_typed" if call_args.len() == 2 => Some(1usize),
                "__olive_obj_update_typed" | "__olive_obj_update_from_any"
                    if call_args.len() == 2 =>
                {
                    Some(0usize)
                }
                "__olive_chan_send" | "__olive_mutex_unlock" if call_args.len() == 2 => {
                    Some(1usize)
                }
                "__olive_obj_values_typed" | "__olive_obj_items_typed" if call_args.len() == 1 => {
                    Some(0usize)
                }
                "__olive_obj_keys_typed" if call_args.len() == 1 => Some(0usize),
                "__olive_iter_typed" if call_args.len() == 1 => Some(0usize),
                "__olive_list_clear_typed"
                | "__olive_set_clear_typed"
                | "__olive_obj_clear_typed"
                    if call_args.len() == 1 =>
                {
                    Some(0usize)
                }
                // `__olive_mutex_new` stores its initial value for a
                // descriptor-driven teardown drain; the descriptor comes
                // from the value's static type, same as the send path.
                "__olive_mutex_new" if call_args.len() == 1 => Some(0usize),
                "__olive_set_add_typed"
                | "__olive_set_remove_typed"
                | "__olive_set_contains_typed"
                | "__olive_obj_get_typed"
                | "__olive_obj_remove_typed"
                | "__olive_list_count_typed"
                    if call_args.len() == 2 =>
                {
                    Some(1usize)
                }
                // Set-`in` on a statically-`Any` haystack with a concrete
                // scalar needle: the descriptor comes from the needle
                // (arg 0), the haystack's own type is `Any`.
                "__olive_in_list_typed" if call_args.len() == 2 => Some(0usize),
                "__olive_obj_get_default_typed"
                | "__olive_obj_get_default_boxed_typed"
                | "__olive_list_index_typed"
                | "__olive_set_remove_checked_typed"
                | "__olive_obj_pop_checked_typed"
                | "__olive_obj_pop_default_typed"
                | "__olive_obj_setdefault_typed"
                    if call_args.len() == 3 =>
                {
                    Some(1usize)
                }
                "__olive_set_union_typed"
                | "__olive_set_intersection_typed"
                | "__olive_set_diff_typed"
                | "__olive_set_sym_diff_typed"
                    if call_args.len() == 2 =>
                {
                    Some(0usize)
                }
                _ => None,
            };
            if let Some(pos) = desc_arg {
                // `operand_static_type`, not a Copy/Move-only match falling back to
                // `arg_type` (args[0]'s type): a folded `Constant` operand at `pos`
                // (e.g. `xs.count(2)`) needs its OWN type, not the receiver's, or the
                // wrong descriptor gets looked up and a raw int is misread as a
                // container pointer. Must match `collect_type_descriptor`'s
                // interning exactly, which already uses this function.
                let arg_static_ty = super::imports::operand_static_type(&args[pos], func_mir);
                let list_ty = &arg_static_ty;
                // Set combinators hash elements, not sets: describe the
                // element type. Mirrors the interning below exactly.
                let elem_ty = match (name.as_str(), list_ty) {
                    (
                        "__olive_set_union_typed"
                        | "__olive_set_intersection_typed"
                        | "__olive_set_diff_typed"
                        | "__olive_set_sym_diff_typed",
                        OliveType::Set(e),
                    ) => (**e).clone(),
                    _ => list_ty.clone(),
                };
                let desc = super::imports::type_descriptor(
                    &elem_ty,
                    struct_fields,
                    field_types,
                    enum_defs,
                );
                let data_id = *string_ids
                    .get(&desc)
                    .expect("typed list op descriptor not interned during collection");
                let local_data = module.declare_data_in_func(data_id, builder.func);
                let desc_ptr = builder.ins().symbol_value(types::I64, local_data);
                let func_id = func_ids[name.as_str()];
                let local_func = module.declare_func_in_func(func_id, builder.func);
                // These callees all declare i64-only parameters, but the
                // generic per-argument conversion below is skipped by this
                // early return -- so float keys, needles, and values (e.g. a
                // float dict key or a float sent over a channel) would reach
                // an integer parameter unconverted and abort compilation.
                // Convert them here with the same convention instead. Floats
                // are the only values that can arrive mistyped: every other
                // argument shape is either already a 64-bit word (ints,
                // strings, pointers, boxed words, descs appended below) or
                // provably absent (the checker rejects non-int positions
                // like list indices).
                let mut full_args: Vec<Value> = call_args
                    .iter()
                    .map(|&arg| super::translate_rvalue::float_word_for_i64_slot(builder, arg))
                    .collect();
                full_args.push(desc_ptr);
                if name == "__olive_obj_get_default_boxed_typed" && args.len() == 3 {
                    let recv_static = super::imports::operand_static_type(&args[0], func_mir);
                    let value_ty = match super::imports::concrete_ty(&recv_static) {
                        OliveType::Dict(_, value) => value.clone(),
                        _ => Box::new(OliveType::Any),
                    };
                    let value_desc = super::imports::type_descriptor(
                        &value_ty,
                        struct_fields,
                        field_types,
                        enum_defs,
                    );
                    let value_data_id = *string_ids
                        .get(&value_desc)
                        .expect("get value descriptor not interned during collection");
                    let value_local_data = module.declare_data_in_func(value_data_id, builder.func);
                    full_args.push(builder.ins().symbol_value(types::I64, value_local_data));
                }
                // `setdefault` discards its default through the value's own
                // descriptor on a hit, so it carries a second descriptor
                // (mirrors the interning in `collect_type_descriptor`).
                if name == "__olive_obj_setdefault_typed" && args.len() == 3 {
                    let recv_static = super::imports::operand_static_type(&args[0], func_mir);
                    let val_ty = match super::imports::concrete_ty(&recv_static) {
                        OliveType::Dict(_, value) => value.clone(),
                        _ => Box::new(OliveType::Any),
                    };
                    let val_desc = super::imports::type_descriptor(
                        &val_ty,
                        struct_fields,
                        field_types,
                        enum_defs,
                    );
                    let val_data_id = *string_ids
                        .get(&val_desc)
                        .expect("setdefault value descriptor not interned during collection");
                    let val_local_data = module.declare_data_in_func(val_data_id, builder.func);
                    full_args.push(builder.ins().symbol_value(types::I64, val_local_data));
                }
                let inst = builder.ins().call(local_func, &full_args);
                let results = builder.inst_results(inst);
                return if results.is_empty() {
                    builder.ins().iconst(types::I64, 0)
                } else {
                    results[0]
                };
            }

            let boxed_value_call = match name.as_str() {
                "__olive_obj_get_boxed" => call_args.len() == 2,
                "__olive_obj_get_default_boxed" => call_args.len() == 3,
                _ => false,
            };
            if boxed_value_call {
                let recv_static = super::imports::operand_static_type(&args[0], func_mir);
                let value_ty = match super::imports::concrete_ty(&recv_static) {
                    OliveType::Dict(_, value) => value.clone(),
                    _ => Box::new(OliveType::Any),
                };
                let value_desc = super::imports::type_descriptor(
                    &value_ty,
                    struct_fields,
                    field_types,
                    enum_defs,
                );
                let value_data_id = *string_ids
                    .get(&value_desc)
                    .expect("get value descriptor not interned during collection");
                let value_local_data = module.declare_data_in_func(value_data_id, builder.func);
                let value_desc_ptr = builder.ins().symbol_value(types::I64, value_local_data);
                let mut full_args: Vec<Value> = call_args
                    .iter()
                    .map(|&arg| super::translate_rvalue::float_word_for_i64_slot(builder, arg))
                    .collect();
                full_args.push(value_desc_ptr);
                let func_id = func_ids[name.as_str()];
                let local_func = module.declare_func_in_func(func_id, builder.func);
                let inst = builder.ins().call(local_func, &full_args);
                let results = builder.inst_results(inst);
                return if results.is_empty() {
                    builder.ins().iconst(types::I64, 0)
                } else {
                    results[0]
                };
            }

            let resolved_name = if name == "round" && args.len() == 2 {
                "__olive_math_round_with_digits"
            } else if (name == "print"
                || name == "str"
                || name == "int"
                || name == "float"
                || name == "bool"
                || name == "iter"
                || name == "next"
                || name == "has_next"
                || name == "slice"
                || name == "list"
                || name == "dict"
                || name == "keys"
                || name == "values"
                || (name == "sum" && !func_ids.contains_key("sum"))
                || (name == "min" && !func_ids.contains_key("min"))
                || (name == "max" && !func_ids.contains_key("max"))
                || (name == "len" && !func_ids.contains_key("len"))
                || name == "remove"
                || name == "abs"
                || name == "round"
                || name == "input")
                && !args.is_empty()
            {
                map_builtin_to_runtime(name, &arg_type).unwrap_or(name.as_str())
            } else if name == "input" && args.is_empty() {
                "__olive_stdin_read_line"
            } else if name == "ffi_errno" {
                "__olive_ffi_errno"
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

                if is_aot_vararg {
                    let fn_ptr = builder.ins().func_addr(types::I64, local_func);
                    return emit_vararg_runtime_call(
                        builder, module, func_ids, fn_ptr, ffi_entry, args, &call_args, func_mir,
                    );
                }

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
                    if matches!(
                        resolved_name,
                        "__olive_free_typed"
                            | "__olive_clear_typed"
                            | "__olive_copy_typed"
                            | "__olive_relocate_typed"
                    ) && i == 1
                        && matches!(args.get(i), Some(Operand::Constant(Constant::Str(_))))
                    {
                        let raw = builder.ins().band_imm(arg, !1);
                        final_args.push(raw);
                        continue;
                    }
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
                                    let eightbyte_ty = |idx: usize,
                                                        small: cranelift::prelude::Type|
                                     -> cranelift::prelude::Type {
                                        if crate::semantic::abi::eightbyte_is_sse(layout, idx) {
                                            if crate::semantic::abi::c_abi_eightbyte_size(
                                                layout, idx,
                                            ) <= 4
                                            {
                                                types::F32
                                            } else {
                                                types::F64
                                            }
                                        } else {
                                            small
                                        }
                                    };
                                    let ty = eightbyte_ty(
                                        0,
                                        match size {
                                            1 => types::I8,
                                            2 => types::I16,
                                            3 | 4 => types::I32,
                                            _ => types::I64,
                                        },
                                    );
                                    let val = builder.ins().load(ty, MemFlags::trusted(), arg, 0);
                                    final_args.push(val);
                                } else if size <= 16 {
                                    let eightbyte_ty = |idx: usize| -> cranelift::prelude::Type {
                                        if crate::semantic::abi::eightbyte_is_sse(layout, idx) {
                                            if crate::semantic::abi::c_abi_eightbyte_size(
                                                layout, idx,
                                            ) <= 4
                                            {
                                                types::F32
                                            } else {
                                                types::F64
                                            }
                                        } else {
                                            types::I64
                                        }
                                    };
                                    let first_ty = eightbyte_ty(0);
                                    let second_ty = eightbyte_ty(1);

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
                        final_args.push(builder.ins().band_imm(arg, !STR_TAG_BITS));
                    } else if is_builtin && builder.func.dfg.value_type(arg) == types::F64 {
                        let expected_float_ty = if let Some(entry) = ffi_entry {
                            match entry.params.get(i).map(String::as_str) {
                                Some("f32") => Some(types::F32),
                                Some("float") | Some("f64") => Some(types::F64),
                                _ => None,
                            }
                        } else {
                            Self::callee_param_float_ty(builder, local_func, i, sret_ptr.is_some())
                        };
                        if expected_float_ty == Some(types::F32) {
                            final_args.push(builder.ins().fdemote(types::F32, arg));
                        } else if expected_float_ty == Some(types::F64) {
                            final_args.push(arg);
                        } else {
                            final_args.push(builder.ins().bitcast(
                                types::I64,
                                MemFlags::new(),
                                arg,
                            ));
                        }
                    } else if is_builtin && builder.func.dfg.value_type(arg) == types::F32 {
                        let expected_float_ty = if let Some(entry) = ffi_entry {
                            match entry.params.get(i).map(String::as_str) {
                                Some("f32") => Some(types::F32),
                                Some("float") | Some("f64") => Some(types::F64),
                                _ => None,
                            }
                        } else {
                            Self::callee_param_float_ty(builder, local_func, i, sret_ptr.is_some())
                        };

                        if expected_float_ty == Some(types::F64) {
                            // The f32->f64 gap: an f32 value reaching a
                            // param the callee's signature declares as f64
                            // (e.g. `__olive_float_to_int`'s `f64` param)
                            // used to be passed through unconverted, landing
                            // a 32-bit value where the callee reads a full
                            // 64-bit double out of the same XMM register.
                            final_args.push(builder.ins().fpromote(types::F64, arg));
                        } else if expected_float_ty == Some(types::F32) {
                            final_args.push(arg);
                        } else {
                            let bitcast_val =
                                builder.ins().bitcast(types::I32, MemFlags::new(), arg);
                            final_args.push(builder.ins().uextend(types::I64, bitcast_val));
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
                            } else if expected == Some(types::F32) && arg_ty == types::F64 {
                                // Untyped float literals lower as f64; an f32
                                // param reads the low half as garbage without
                                // an explicit demote.
                                final_args.push(builder.ins().fdemote(types::F32, arg));
                            } else if expected == Some(types::F64) && arg_ty == types::F32 {
                                final_args.push(builder.ins().fpromote(types::F64, arg));
                            } else {
                                final_args.push(arg);
                            }
                        } else if matches!(arg_ty, types::I8 | types::I16 | types::I32) {
                            // `bool`/`i8`/`i16`/`i32`/`u8`/`u16`/`u32` locals are narrower
                            // than the i64 word every `__olive_*` runtime signature
                            // declares. Cranelift's call lowering does not widen a
                            // narrower argument value on its own, so the callee reads
                            // whatever garbage sits in the upper bits of that register --
                            // undefined, and only "correct" by register-allocation luck
                            // (this is what made `not` on a struct-field-derived `bool`
                            // print wrong while branching on the same value stayed right:
                            // `brif`-style dispatch only reads the low bits, `__olive_write_bool`
                            // reads the full word).
                            let local_sig = builder.func.dfg.ext_funcs[local_func].signature;
                            let params = &builder.func.dfg.signatures[local_sig].params;
                            let param_idx = final_args.len();
                            let expected = params.get(param_idx).map(|p| p.value_type);
                            if expected == Some(types::I64) {
                                let is_signed = args.get(i).is_some_and(|op| {
                                    let ty = match op {
                                        Operand::Copy(l) | Operand::Move(l) => {
                                            &func_mir.locals[l.0].ty
                                        }
                                        _ => &OliveType::Any,
                                    };
                                    matches!(
                                        super::imports::concrete_ty(ty),
                                        OliveType::I8 | OliveType::I16 | OliveType::I32
                                    )
                                });
                                if is_signed {
                                    final_args.push(builder.ins().sextend(types::I64, arg));
                                } else {
                                    final_args.push(builder.ins().uextend(types::I64, arg));
                                }
                            } else {
                                final_args.push(arg);
                            }
                        } else if arg_ty == types::I64 {
                            // The reverse of the F64/F32 case above: a `T |
                            // None` scalar union (e.g. `float | None`) has no
                            // boxed representation and carries its payload
                            // as a raw word (`cl_type` maps every `Union` to
                            // I64), so a narrowed float value reaches here
                            // I64-typed. Passing it unconverted lands the
                            // bits in an integer argument register while the
                            // callee reads its float parameter from XMM0.
                            let local_sig = builder.func.dfg.ext_funcs[local_func].signature;
                            let params = &builder.func.dfg.signatures[local_sig].params;
                            let param_idx = final_args.len();
                            let expected = params.get(param_idx).map(|p| p.value_type);
                            if expected == Some(types::F64) {
                                final_args.push(builder.ins().bitcast(
                                    types::F64,
                                    MemFlags::new(),
                                    arg,
                                ));
                            } else if expected == Some(types::F32) {
                                let low = builder.ins().ireduce(types::I32, arg);
                                final_args.push(builder.ins().bitcast(
                                    types::F32,
                                    MemFlags::new(),
                                    low,
                                ));
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

                // An sret return wrote straight into our frame; hand the value
                // out as an owning heap copy for the same reason as below.
                let mut ret_val = if let Some(ptr) = sret_ptr {
                    if is_ffi {
                        let ret_name = ffi_entry.and_then(|e| e.ret.as_ref()).unwrap();
                        let size = *c_struct_sizes.get(ret_name).unwrap_or(&8);
                        let heap_id = func_ids
                            .get("__olive_alloc")
                            .copied()
                            .expect("missing __olive_alloc");
                        let local_alloc = module.declare_func_in_func(heap_id, builder.func);
                        let size_val = builder.ins().iconst(types::I64, size);
                        let alloc_inst = builder.ins().call(local_alloc, &[size_val]);
                        let heap_ptr = builder.inst_results(alloc_inst)[0];
                        let copy_size = builder.ins().iconst(module.isa().pointer_type(), size);
                        builder.call_memcpy(
                            module.isa().frontend_config(),
                            heap_ptr,
                            ptr,
                            copy_size,
                        );
                        heap_ptr
                    } else {
                        ptr
                    }
                } else {
                    let results = builder.inst_results(inst).to_vec();
                    if is_ffi
                        && let Some(entry) = ffi_entry
                        && let Some(ref r) = entry.ret
                        && c_struct_sizes.contains_key(r)
                    {
                        // A C-struct return lives in caller registers; it must
                        // land in a heap block before it becomes a live Olive
                        // value, because scope exit frees C structs with
                        // `__olive_free_c_struct` and the frame slot below dies
                        // with the function.
                        let size = *c_struct_sizes.get(r).unwrap_or(&8);
                        let heap_id = func_ids
                            .get("__olive_alloc")
                            .copied()
                            .expect("missing __olive_alloc");
                        let local_alloc = module.declare_func_in_func(heap_id, builder.func);
                        let size_val = builder.ins().iconst(types::I64, size);
                        let alloc_inst = builder.ins().call(local_alloc, &[size_val]);
                        let heap_ptr = builder.inst_results(alloc_inst)[0];
                        if results.len() == 1 {
                            builder
                                .ins()
                                .store(MemFlags::trusted(), results[0], heap_ptr, 0);
                        } else if results.len() == 2 {
                            builder
                                .ins()
                                .store(MemFlags::trusted(), results[0], heap_ptr, 0);
                            builder
                                .ins()
                                .store(MemFlags::trusted(), results[1], heap_ptr, 8);
                        }
                        heap_ptr
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

                // A fused py-call `f32` result (R10) carries its bits home as
                // an `I64`, the same as every other scalar the tagged fast
                // path returns -- CPython floats are always `f64`, so
                // `finish_ret` always hands back a full 64-bit pattern. The
                // generic post-call fixup one layer up (`translate.rs`'s
                // `Assign` handler) already bit-reinterprets a same-width
                // `I64` result into `float` correctly, but a narrower `f32`
                // target falls into its int-to-float *numeric* conversion
                // path instead, reading the bit pattern as if it were an
                // integer count. Demote through `f64` here so that fixup
                // sees the `f64 -> f32` case it already handles correctly.
                // Container reads are excluded: their words hold canonical
                // slot bits (f32-extended for `F32` slots), which this
                // f64-first path would misread; the `Assign` handler's
                // container-read branch narrows them instead.
                if *dest_ty == OliveType::F32
                    && builder.func.dfg.value_type(ret_val) == types::I64
                    && !super::imports::is_container_read_call(resolved_name)
                {
                    let bits64 = builder.ins().bitcast(types::F64, MemFlags::new(), ret_val);
                    ret_val = builder.ins().fdemote(types::F32, bits64);
                }
                return ret_val;
            }

            if let Some(&fn_ptr) = ffi_vararg_ptrs.get(resolved_name) {
                let fn_ptr_value = builder.ins().iconst(types::I64, fn_ptr as i64);
                let entry = ffi_entries.iter().find(|e| e.jit_name == resolved_name);
                return emit_vararg_runtime_call(
                    builder,
                    module,
                    func_ids,
                    fn_ptr_value,
                    entry,
                    args,
                    &call_args,
                    func_mir,
                );
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
        } else if let OliveType::Fn(_, ret_ty, _) =
            super::imports::concrete_ty(&super::imports::operand_static_type(func, func_mir))
        {
            // A closure value is always the record pointer built by
            // `closures.rs::build_closure_value`: word 1 is the calling
            // thunk, which takes the call's own args plus the record itself
            // as a trailing hidden env argument (uniform across capturing
            // and non-capturing closures alike, so every indirect call site
            // has exactly one shape -- see the E5.2/E5.3 design note). A
            // direct call by name never reaches this branch at all. Other
            // indirect-call shapes (trait-object vtable dispatch, a raw
            // FFI function pointer) are plain code addresses, not records --
            // the `else` arm below keeps calling those exactly as before.
            let record_ptr =
                Self::translate_operand(builder, func, vars, string_ids, module, func_ids);
            let thunk_ptr = builder
                .ins()
                .load(types::I64, MemFlags::trusted(), record_ptr, 8);

            let mut sig = module.make_signature();
            sig.call_conv = module.isa().default_call_conv();

            for &a in &call_args {
                sig.params
                    .push(AbiParam::new(builder.func.dfg.value_type(a)));
            }
            sig.params.push(AbiParam::new(types::I64));
            // The real return type, not a hardcoded I64: SysV passes a float
            // return in XMM0, an int return in RAX, so a mismatched
            // signature here reads whichever register the callee didn't
            // actually use.
            sig.returns
                .push(AbiParam::new(super::imports::cl_type(ret_ty)));

            let sig_ref = builder.import_signature(sig);
            let mut full_args = call_args.clone();
            full_args.push(record_ptr);
            let inst = builder.ins().call_indirect(sig_ref, thunk_ptr, &full_args);
            let results = builder.inst_results(inst);

            if results.is_empty() {
                builder.ins().iconst(types::I64, 0)
            } else {
                results[0]
            }
        } else {
            let fn_ptr_val =
                Self::translate_operand(builder, func, vars, string_ids, module, func_ids);

            let mut sig = module.make_signature();
            sig.call_conv = module.isa().default_call_conv();

            for &a in &call_args {
                sig.params
                    .push(AbiParam::new(builder.func.dfg.value_type(a)));
            }
            // A raw code-address call (vtable dispatch is the only source of
            // these today): the real return type, not a hardcoded I64 -- a
            // trait method returning `float` puts its result in XMM0, and a
            // signature that declares I64 here reads RAX instead, the same
            // register-mismatch bug the closure-record branch above already
            // guards against.
            sig.returns
                .push(AbiParam::new(super::imports::cl_type(dest_ty)));

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
