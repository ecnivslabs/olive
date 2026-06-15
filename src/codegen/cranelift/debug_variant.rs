//! Compiles a debug session's `$debug`-instrumented function bodies
//! alongside the clean ones `generate()` already produced, and exposes the
//! addresses/cells `tooling::dap::launch` needs to build its runtime swap
//! table. Mirrors `tier_up::retier`'s compile-under-a-new-name shape, minus
//! the Any-add specialization bookkeeping that only applies to tier-up.

use super::CraneliftCodegen;
use crate::mir::ir::MirFunction;
use cranelift::prelude::*;
use cranelift_jit::JITModule;
use cranelift_module::{Linkage, Module};

impl CraneliftCodegen<JITModule> {
    /// Compiles `debug_func` (one function's output from
    /// `mir::debug_hooks::instrument`) under a `$debug`-suffixed symbol.
    /// Does not touch the function's dispatch cell -- callers read the
    /// resulting address via `debug_variant_addr` and decide when to
    /// activate it. `None` if `debug_func`'s name has no dispatch cell
    /// (only functions selected when `debug_dual_variant` was set get
    /// one -- see `setup/dispatch.rs`).
    pub fn install_debug_variant(&mut self, debug_func: &MirFunction) -> Option<()> {
        if !self.dispatch_ids.contains_key(&debug_func.name) {
            return None;
        }
        let variant_name = format!("{}$debug", debug_func.name);
        let mut variant = debug_func.clone();
        variant.name = variant_name.clone();

        let mut sig = self.module.make_signature();
        for i in 0..variant.arg_count {
            let ty = &variant.locals[i + 1].ty;
            sig.params.push(AbiParam::new(super::imports::cl_type(ty)));
        }
        sig.returns.push(AbiParam::new(super::imports::cl_type(
            &variant.locals[0].ty,
        )));

        let func_id = self
            .module
            .declare_function(&variant_name, Linkage::Local, &sig)
            .unwrap();
        self.func_ids.insert(variant_name, func_id);
        self.translate_function(&variant);
        Some(())
    }

    /// Address of `func_name`'s clean (default) compiled body. Must be
    /// called after `finalize()`.
    pub fn clean_variant_addr(&self, func_name: &str) -> Option<i64> {
        let &id = self.func_ids.get(func_name)?;
        Some(self.module.get_finalized_function(id) as i64)
    }

    /// Address of `func_name`'s already-`install_debug_variant`-compiled
    /// `$debug` body. Must be called after `finalize()`.
    pub fn debug_variant_addr(&self, func_name: &str) -> Option<i64> {
        let id = *self.func_ids.get(&format!("{func_name}$debug"))?;
        Some(self.module.get_finalized_function(id) as i64)
    }

    /// Raw pointer to `func_name`'s dispatch cell data, for the runtime-side
    /// swap table `tooling::dap::launch` builds. Must be called after
    /// `finalize()`; the cell is a `'static`-lifetime JIT data segment for
    /// the module's whole life, so the pointer stays valid for the session.
    pub fn dispatch_cell_ptr(&self, func_name: &str) -> Option<*mut u8> {
        let &id = self.dispatch_ids.get(func_name)?;
        Some(self.module.get_finalized_data(id).0 as *mut u8)
    }
}

#[cfg(test)]
mod tests {
    use crate::codegen::cranelift::CraneliftCodegen;
    use crate::mir::Optimizer;
    use crate::test_utils::{build_mir, call_i64_1};

    fn debug_dual_variant_codegen(src: &str) -> CraneliftCodegen<cranelift_jit::JITModule> {
        let mut functions = build_mir(src);
        Optimizer::minimal().run(&mut functions);
        let mut cg = CraneliftCodegen::new_jit(
            functions,
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            &[],
            false,
        );
        cg.debug_dual_variant = true;
        cg.generate();
        cg
    }

    #[test]
    fn install_debug_variant_compiles_under_dollar_debug_name() {
        let src = concat!(
            "fn add(a: int, b: int) -> int:\n    return a + b\n",
            "fn f(x: int) -> int:\n    return add(x, x)\n",
        );
        let mut cg = debug_dual_variant_codegen(src);
        assert!(
            cg.dispatch_ids.contains_key("add"),
            "debug_dual_variant should get every debug-instrumentable fn a cell"
        );

        let mut functions = build_mir(src);
        Optimizer::minimal().run(&mut functions);
        let program = crate::mir::debug_hooks::instrument(&mut functions);
        let add_debug = functions.iter().find(|f| f.name == "add").unwrap();
        assert!(cg.install_debug_variant(add_debug).is_some());
        cg.finalize();

        assert!(cg.clean_variant_addr("add").is_some());
        assert!(cg.debug_variant_addr("add").is_some());
        assert_ne!(cg.clean_variant_addr("add"), cg.debug_variant_addr("add"));
        assert!(program.functions.iter().any(|f| f.name == "add"));
        assert!(cg.dispatch_cell_ptr("add").is_some());
        // Cell still points at the clean body -- install doesn't activate.
        assert_eq!(call_i64_1(&mut cg, "f", 5), 10);
    }

    #[test]
    fn install_debug_variant_refuses_function_without_a_cell() {
        let src = "async fn fetch() -> int:\n    return 1\nfn main():\n    print(1)\n";
        let mut cg = debug_dual_variant_codegen(src);
        let mut functions = build_mir(src);
        Optimizer::minimal().run(&mut functions);
        crate::mir::debug_hooks::instrument(&mut functions);
        let fetch = functions.iter().find(|f| f.name == "fetch").unwrap();
        assert!(cg.install_debug_variant(fetch).is_none());
    }
}
