use super::super::CraneliftCodegen;
use cranelift::prelude::*;
use cranelift_module::{Linkage, Module};

impl<M: Module> CraneliftCodegen<M> {
    pub(super) fn emit_extern_var_getter(&mut self, name: &str, addr: i64, ty_str: &str) {
        use cranelift::prelude::FunctionBuilderContext;
        let cl_ty = super::super::ffi_cl_type(ty_str);
        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I64));
        let Ok(func_id) = self.module.declare_function(name, Linkage::Local, &sig) else {
            return;
        };
        self.func_ids.insert(name.to_string(), func_id);
        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let block = builder.create_block();
        builder.switch_to_block(block);
        builder.seal_block(block);
        let addr_val = builder.ins().iconst(types::I64, addr);
        let raw = builder
            .ins()
            .load(cl_ty, cranelift::prelude::MemFlags::trusted(), addr_val, 0);
        let val = if cl_ty != types::I64 {
            if cl_ty.is_float() {
                builder
                    .ins()
                    .bitcast(types::I64, cranelift::prelude::MemFlags::new(), raw)
            } else {
                builder.ins().uextend(types::I64, raw)
            }
        } else {
            raw
        };
        builder.ins().return_(&[val]);
        builder.finalize();
        if self.module.define_function(func_id, &mut ctx).is_err() {
            eprintln!("warning: failed to emit getter for extern var '{}'", name);
        }
    }

    pub(super) fn emit_aot_extern_var_getter(&mut self, name: &str, ty_str: &str, c_name: &str) {
        use cranelift::prelude::FunctionBuilderContext;
        let cl_ty = super::super::ffi_cl_type(ty_str);
        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I64));
        let Ok(func_id) = self.module.declare_function(name, Linkage::Local, &sig) else {
            return;
        };
        self.func_ids.insert(name.to_string(), func_id);

        let data_id = match self
            .module
            .declare_data(c_name, Linkage::Import, false, false)
        {
            Ok(id) => id,
            Err(_) => return,
        };

        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let block = builder.create_block();
        builder.switch_to_block(block);
        builder.seal_block(block);

        let sym_val = self.module.declare_data_in_func(data_id, builder.func);
        let addr_val = builder.ins().symbol_value(types::I64, sym_val);

        let raw = builder
            .ins()
            .load(cl_ty, cranelift::prelude::MemFlags::trusted(), addr_val, 0);

        let val = if cl_ty != types::I64 {
            if cl_ty.is_float() {
                builder
                    .ins()
                    .bitcast(types::I64, cranelift::prelude::MemFlags::new(), raw)
            } else {
                builder.ins().uextend(types::I64, raw)
            }
        } else {
            raw
        };
        builder.ins().return_(&[val]);
        builder.finalize();
        if self.module.define_function(func_id, &mut ctx).is_err() {
            eprintln!("warning: failed to emit getter for extern var '{}'", name);
        }
    }

    pub(super) fn emit_aot_main(&mut self) {
        let Some(&olive_main_id) = self.func_ids.get("__main__") else {
            return;
        };
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I32));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I32));
        let Ok(func_id) = self.module.declare_function("main", Linkage::Export, &sig) else {
            return;
        };
        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let block = builder.create_block();
        builder.append_block_params_for_function_params(block);
        builder.switch_to_block(block);
        builder.seal_block(block);
        let local_fn = self
            .module
            .declare_func_in_func(olive_main_id, builder.func);
        let call = builder.ins().call(local_fn, &[]);
        let exit_code_i64 = builder.inst_results(call)[0];

        // Finalize the Python interpreter so atexit handlers run. No-op if never initialized.
        if let Some(fin_fn) = self.declare_runtime_void_fn("__olive_py_finalize") {
            let local_fin = self.module.declare_func_in_func(fin_fn, builder.func);
            builder.ins().call(local_fin, &[]);
        }

        let exit_code_i32 = builder.ins().ireduce(types::I32, exit_code_i64);
        builder.ins().return_(&[exit_code_i32]);
        builder.finalize();
        self.module.define_function(func_id, &mut ctx).unwrap();
        self.func_ids.insert("main".to_string(), func_id);
    }

    /// Declares a void->void runtime function resolved from SYMBOL_MAP. Returns cached id.
    pub(super) fn declare_runtime_void_fn(
        &mut self,
        name: &str,
    ) -> Option<cranelift_module::FuncId> {
        if let Some(&id) = self.func_ids.get(name) {
            return Some(id);
        }
        let decl_name = super::super::SYMBOL_MAP
            .iter()
            .find(|&&(k, _)| k == name)
            .map(|&(_, v)| std::str::from_utf8(&v[..v.len() - 1]).unwrap())?;
        let sig = self.module.make_signature();
        let id = self
            .module
            .declare_function(decl_name, Linkage::Import, &sig)
            .ok()?;
        self.func_ids.insert(name.to_string(), id);
        Some(id)
    }

    /// Declares a runtime function with given Cranelift param/return types,
    /// resolved from SYMBOL_MAP. Returns cached id.
    pub(super) fn declare_runtime_fn(
        &mut self,
        name: &str,
        param_types: &[cranelift::prelude::Type],
        return_types: &[cranelift::prelude::Type],
    ) -> Option<cranelift_module::FuncId> {
        if let Some(&id) = self.func_ids.get(name) {
            return Some(id);
        }
        let decl_name = super::super::SYMBOL_MAP
            .iter()
            .find(|&&(k, _)| k == name)
            .map(|&(_, v)| std::str::from_utf8(&v[..v.len() - 1]).unwrap())?;
        let mut sig = self.module.make_signature();
        for &p in param_types {
            sig.params.push(AbiParam::new(p));
        }
        for &r in return_types {
            sig.returns.push(AbiParam::new(r));
        }
        let id = self
            .module
            .declare_function(decl_name, Linkage::Import, &sig)
            .ok()?;
        self.func_ids.insert(name.to_string(), id);
        Some(id)
    }
}
