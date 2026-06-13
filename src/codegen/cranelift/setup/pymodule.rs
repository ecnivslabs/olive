use super::super::CraneliftCodegen;
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_module::{DataDescription, Linkage, Module};

fn type_to_arg_tag(ty: &OliveType) -> Option<i64> {
    match ty {
        OliveType::Int | OliveType::U64 | OliveType::Usize => Some(1),
        OliveType::Float | OliveType::F32 => Some(2),
        OliveType::Str => Some(3),
        OliveType::Bool => Some(4),
        OliveType::Any => Some(5),
        OliveType::Null => Some(6),
        OliveType::Bytes => Some(7),
        OliveType::PyObject | OliveType::PyNamed(..) => Some(0),
        OliveType::List(inner) => match inner.as_ref() {
            OliveType::Float | OliveType::F32 => Some(8),
            OliveType::Int | OliveType::U64 | OliveType::Usize => Some(9),
            OliveType::Str => Some(10),
            OliveType::Bool => Some(11),
            OliveType::Any => Some(12),
            _ => None,
        },
        _ => None,
    }
}

fn name_is_internal(name: &str) -> bool {
    name == "__main__" || name.starts_with("__olive") || name.starts_with('_')
}

pub(super) struct ExportEntry {
    pub name: String,
    pub func_local_name: String,
    pub wrapper_local_name: String,
    pub tags: i64,
}

impl<M: Module> CraneliftCodegen<M> {
    pub(super) fn collect_exports(&self) -> Vec<ExportEntry> {
        let mut exports = Vec::new();
        for func in &self.functions {
            let name = &func.name;
            if name_is_internal(name) {
                continue;
            }
            if func.arg_count > 4 {
                eprintln!(
                    "warning: export '{}' has >4 params, not exportable (arity {})",
                    name, func.arg_count
                );
                continue;
            }
            let ret_ty = &func.locals[0].ty;
            let ret_tag = match type_to_arg_tag(ret_ty) {
                Some(t) => t,
                None => {
                    eprintln!("warning: export '{}' has unsupported return type", name);
                    continue;
                }
            };
            let mut tags = (func.arg_count as i64) << 56;
            tags |= ret_tag << 60;
            let mut ok = true;
            for i in 0..func.arg_count {
                let param_ty = &func.locals[i + 1].ty;
                match type_to_arg_tag(param_ty) {
                    Some(tag) => {
                        tags |= tag << (i as u32 * 4);
                    }
                    None => {
                        eprintln!(
                            "warning: export '{}' param {} has unsupported type",
                            name, i
                        );
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            let wrapper_name = format!("{}__export", name);
            exports.push(ExportEntry {
                name: name.clone(),
                func_local_name: name.clone(),
                wrapper_local_name: wrapper_name,
                tags,
            });
        }
        exports
    }

    fn declare_and_define_export_wrapper(
        &mut self,
        export: &ExportEntry,
    ) -> Option<cranelift_module::FuncId> {
        let real_id = *self.func_ids.get(&export.func_local_name)?;
        let func = self
            .functions
            .iter()
            .find(|f| f.name == export.func_local_name)?;

        let mut sig = self.module.make_signature();
        for i in 0..func.arg_count {
            let ty = &func.locals[i + 1].ty;
            sig.params
                .push(AbiParam::new(super::super::imports::cl_type(ty)));
        }
        sig.params.push(AbiParam::new(types::I64));
        let ret_ty = &func.locals[0].ty;
        sig.returns
            .push(AbiParam::new(super::super::imports::cl_type(ret_ty)));

        let decl_name = &export.wrapper_local_name;
        if self.func_ids.contains_key(decl_name) {
            return self.func_ids.get(decl_name).copied();
        }

        let wrapper_id = self
            .module
            .declare_function(decl_name, Linkage::Export, &sig)
            .ok()?;
        self.func_ids.insert(decl_name.to_string(), wrapper_id);

        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let block = builder.create_block();
        builder.append_block_params_for_function_params(block);
        builder.switch_to_block(block);
        builder.seal_block(block);

        let n_params = func.arg_count;
        let params: Vec<Value> = builder.block_params(block)[..n_params].to_vec();

        let real_ref = self.module.declare_func_in_func(real_id, builder.func);
        let call_inst = builder.ins().call(real_ref, &params);
        let result = builder.inst_results(call_inst)[0];

        if ret_ty == &OliveType::Null {
            builder.ins().return_(&[]);
        } else {
            builder.ins().return_(&[result]);
        }

        builder.finalize();
        self.module.define_function(wrapper_id, &mut ctx).ok()?;
        Some(wrapper_id)
    }

    fn intern_pymodule_string(&mut self, s: &str) -> cranelift_module::DataId {
        if let Some(&id) = self.string_ids.get(s) {
            return id;
        }
        let mut data_ctx = DataDescription::new();
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        if !bytes.len().is_multiple_of(2) {
            bytes.push(0);
        }
        data_ctx.define(bytes.into_boxed_slice());
        let name = format!("pystr_{}", self.string_ids.len());
        let id = self
            .module
            .declare_data(&name, Linkage::Export, false, false)
            .unwrap();
        self.module.define_data(id, &data_ctx).unwrap();
        self.string_ids.insert(s.to_string(), id);
        id
    }

    pub(super) fn emit_pymodule_init(&mut self, module_name: &str) {
        let exports = self.collect_exports();
        if exports.is_empty() {
            return;
        }

        for export in &exports {
            self.declare_and_define_export_wrapper(export);
        }

        let init_sym = "__olive_py_initialize";
        let create_sym = "__olive_py_create_module";
        let make_export_sym = "__olive_py_make_export";
        let add_obj_sym = "__olive_py_module_add_object";

        let init_id = self.declare_runtime_void_fn(init_sym);
        let create_id = self.declare_runtime_fn(create_sym, &[types::I64], &[types::I64]);
        let make_export_id =
            self.declare_runtime_fn(make_export_sym, &[types::I64, types::I64], &[types::I64]);
        let add_obj_id = self.declare_runtime_fn(
            add_obj_sym,
            &[types::I64, types::I64, types::I64],
            &[types::I64],
        );

        let Some(create_id) = create_id else {
            return;
        };
        let Some(make_export_id) = make_export_id else {
            return;
        };
        let Some(add_obj_id) = add_obj_id else {
            return;
        };

        let mod_name_data_id = self.intern_pymodule_string(module_name);

        let pyname = format!("PyInit_{}", module_name);
        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I64));
        let Ok(func_id) = self.module.declare_function(&pyname, Linkage::Export, &sig) else {
            return;
        };
        self.func_ids.insert(pyname.clone(), func_id);

        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let block = builder.create_block();
        builder.switch_to_block(block);
        builder.seal_block(block);

        // call __olive_py_initialize
        if let Some(init_id) = init_id {
            let init_ref = self.module.declare_func_in_func(init_id, builder.func);
            builder.ins().call(init_ref, &[]);
        }

        // mod = __olive_py_create_module(module_name)
        let mod_name_ref = self
            .module
            .declare_data_in_func(mod_name_data_id, builder.func);
        let mod_name_ptr = builder.ins().symbol_value(types::I64, mod_name_ref);
        let create_ref = self.module.declare_func_in_func(create_id, builder.func);
        let call_mod = builder.ins().call(create_ref, &[mod_name_ptr]);
        let mod_val = builder.inst_results(call_mod)[0];

        // if mod == null: return null
        let null_val = builder.ins().iconst(types::I64, 0);
        let is_null = builder.ins().icmp(IntCC::Equal, mod_val, null_val);
        let fail_block = builder.create_block();
        let cont_block = builder.create_block();
        builder
            .ins()
            .brif(is_null, fail_block, &[], cont_block, &[]);
        builder.switch_to_block(fail_block);
        builder.seal_block(fail_block);
        builder.ins().return_(&[null_val]);
        builder.switch_to_block(cont_block);
        builder.seal_block(cont_block);

        for export in &exports {
            let wrapper_id = match self.func_ids.get(&export.wrapper_local_name) {
                Some(id) => *id,
                None => continue,
            };

            let export_name_data_id = self.intern_pymodule_string(&export.name);

            // fn_addr = func_addr(wrapper_fn)
            let wrapper_ref = self.module.declare_func_in_func(wrapper_id, builder.func);
            let fn_addr = builder.ins().func_addr(types::I64, wrapper_ref);

            // callable = __olive_py_make_export(fn_addr, tags)
            let tags_val = builder.ins().iconst(types::I64, export.tags);
            let make_ref = self
                .module
                .declare_func_in_func(make_export_id, builder.func);
            let call_callable = builder.ins().call(make_ref, &[fn_addr, tags_val]);
            let callable_val = builder.inst_results(call_callable)[0];

            // if callable == null: decref mod, return null
            let is_null_callable = builder.ins().icmp(IntCC::Equal, callable_val, null_val);
            let callable_fail = builder.create_block();
            let callable_cont = builder.create_block();
            builder
                .ins()
                .brif(is_null_callable, callable_fail, &[], callable_cont, &[]);
            builder.switch_to_block(callable_fail);
            builder.seal_block(callable_fail);
            builder.ins().return_(&[null_val]);
            builder.switch_to_block(callable_cont);
            builder.seal_block(callable_cont);

            // result = __olive_py_module_add_object(mod, name, callable)
            let export_name_ref = self
                .module
                .declare_data_in_func(export_name_data_id, builder.func);
            let export_name_ptr = builder.ins().symbol_value(types::I64, export_name_ref);
            let add_ref = self.module.declare_func_in_func(add_obj_id, builder.func);
            let call_add = builder
                .ins()
                .call(add_ref, &[mod_val, export_name_ptr, callable_val]);
            let add_result = builder.inst_results(call_add)[0];

            // if result == 0: failed, return null
            let zero_val = builder.ins().iconst(types::I64, 0);
            let is_fail = builder.ins().icmp(IntCC::Equal, add_result, zero_val);
            let add_fail = builder.create_block();
            let add_cont = builder.create_block();
            builder.ins().brif(is_fail, add_fail, &[], add_cont, &[]);
            builder.switch_to_block(add_fail);
            builder.seal_block(add_fail);
            builder.ins().return_(&[null_val]);
            builder.switch_to_block(add_cont);
            builder.seal_block(add_cont);
        }

        builder.ins().return_(&[mod_val]);
        builder.finalize();
        self.module.define_function(func_id, &mut ctx).unwrap();
    }
}
