use super::super::CraneliftCodegen;
use cranelift_module::{DataDescription, Linkage, Module};

impl<M: Module> CraneliftCodegen<M> {
    pub(super) fn generate_global_vars(&mut self) {
        let vars = self.global_vars.clone();
        for var_name in vars {
            let mut data_ctx = DataDescription::new();
            data_ctx.define_zeroinit(8);
            let id = self
                .module
                .declare_data(&var_name, Linkage::Export, true, false)
                .unwrap();
            self.module.define_data(id, &data_ctx).unwrap();
        }
    }
    pub(super) fn generate_vtables(&mut self) {
        let vtables = self.vtables.clone();
        for (vtable_name, methods) in vtables {
            let mut data_ctx = DataDescription::new();
            let bytes = vec![0u8; methods.len() * 8];
            data_ctx.define(bytes.into_boxed_slice());

            for (i, method) in methods.iter().enumerate() {
                if let Some(&func_id) = self.func_ids.get(method) {
                    let local_func = self.module.declare_func_in_data(func_id, &mut data_ctx);
                    data_ctx.write_function_addr((i * 8) as u32, local_func);
                }
            }

            let id = self
                .module
                .declare_data(&vtable_name, Linkage::Export, true, false)
                .unwrap();
            self.module.define_data(id, &data_ctx).unwrap();
        }
    }
}
