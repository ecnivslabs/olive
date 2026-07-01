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

        // A state-machine `async fn` (a real `await` in its body) compiles to
        // a heap-frame poll plus a wrapper that hands that poll to the
        // executor. Its `$debug` variant is the same shape, only the poll is
        // translated with the shadow-frame hooks (`sm_debug_fn_id` gates that
        // on `debug_sm_fn_ids`), and the wrapper baked to hand the executor
        // the `$debug` poll. The cell on the wrapper (`setup/dispatch.rs`)
        // swaps callers between this and the clean wrapper.
        if debug_func.is_async
            && let Some(await_points) = Self::analyze_async_sm(debug_func)
        {
            let mut variant = debug_func.clone();
            variant.name = variant_name.clone();

            let poll_name = format!("{variant_name}__sm_poll");
            let mut poll_sig = self.module.make_signature();
            poll_sig.params.push(AbiParam::new(types::I64));
            poll_sig.returns.push(AbiParam::new(types::I64));
            let poll_id = self
                .module
                .declare_function(&poll_name, Linkage::Local, &poll_sig)
                .unwrap();
            self.func_ids.insert(poll_name, poll_id);

            let mut wrapper_sig = self.module.make_signature();
            for i in 0..variant.arg_count {
                let ty = &variant.locals[i + 1].ty;
                wrapper_sig
                    .params
                    .push(AbiParam::new(super::imports::cl_type(ty)));
            }
            wrapper_sig.returns.push(AbiParam::new(types::I64));
            let wrapper_id = self
                .module
                .declare_function(&variant_name, Linkage::Local, &wrapper_sig)
                .unwrap();
            self.func_ids.insert(variant_name, wrapper_id);

            self.translate_async_sm_poll(&variant, &await_points);
            self.generate_sm_wrapper(&variant);
            return Some(());
        }

        // A non-state-machine async fn (no `await` in its own body): real body
        // compiled separately, wrapped in a spawn-task wrapper -- exactly what
        // `generate()`'s own async branch does for the clean variant.
        // Compiling `debug_func` straight through `translate_function` under
        // the `$debug` name (an old behavior here) produced the *raw body*,
        // not a wrapper, so activating it made a call to the function run
        // inline on whichever thread called it instead of on its own thread.
        if debug_func.is_async {
            let body_name = format!("{variant_name}__async_body");
            let mut body = debug_func.clone();
            body.name = body_name.clone();
            body.is_async = false;

            let body_sig = self.own_signature(&body);
            let body_id = self
                .module
                .declare_function(&body_name, Linkage::Local, &body_sig)
                .unwrap();
            self.func_ids.insert(body_name, body_id);
            self.translate_function(&body);

            let mut wrapper_sig = self.module.make_signature();
            for i in 0..debug_func.arg_count {
                let ty = &debug_func.locals[i + 1].ty;
                wrapper_sig
                    .params
                    .push(AbiParam::new(super::imports::cl_type(ty)));
            }
            wrapper_sig.returns.push(AbiParam::new(types::I64));
            let wrapper_id = self
                .module
                .declare_function(&variant_name, Linkage::Local, &wrapper_sig)
                .unwrap();
            self.func_ids.insert(variant_name, wrapper_id);
            self.generate_async_wrapper_body(debug_func, body_id, wrapper_id);
            return Some(());
        }

        let mut variant = debug_func.clone();
        variant.name = variant_name.clone();
        let sig = self.own_signature(&variant);
        let func_id = self
            .module
            .declare_function(&variant_name, Linkage::Local, &sig)
            .unwrap();
        self.func_ids.insert(variant_name, func_id);
        self.translate_function(&variant);
        Some(())
    }

    /// `func`'s own signature (param types, real return type) -- the shape
    /// both the ordinary variant and a non-SM async fn's inner body compile
    /// under (the body still returns `T`, only the wrapper around it
    /// returns a `Future[T]` handle).
    fn own_signature(&self, func: &MirFunction) -> Signature {
        let mut sig = self.module.make_signature();
        for i in 0..func.arg_count {
            let ty = &func.locals[i + 1].ty;
            sig.params.push(AbiParam::new(super::imports::cl_type(ty)));
        }
        sig.returns
            .push(AbiParam::new(super::imports::cl_type(&func.locals[0].ty)));
        sig
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
    fn install_debug_variant_compiles_a_state_machine_async_fn() {
        // `fetch` really awaits something, so it's state-machine-lowered
        // (`analyze_async_sm` finds a real await point). It now gets a
        // dispatch cell on its wrapper and its `$debug` variant compiles as a
        // shadow-frame-instrumented poll plus a wrapper baked to that poll, so
        // `tooling::dap::launch` can swap it in for breakpoints/stepping
        // inside the state machine (end-to-end: `tooling::dap::tests::
        // state_machine_async_debugging_end_to_end`).
        let src = "async fn leaf() -> int:\n    return 1\nasync fn fetch() -> int:\n    let x = await leaf()\n    return x\nfn main():\n    print(1)\n";
        let mut cg = debug_dual_variant_codegen(src);
        assert!(
            cg.dispatch_ids.contains_key("fetch"),
            "a state-machine async fn gets a wrapper cell under debug_dual_variant"
        );
        let mut functions = build_mir(src);
        Optimizer::minimal().run(&mut functions);
        crate::mir::debug_hooks::instrument(&mut functions);
        let fetch = functions.iter().find(|f| f.name == "fetch").unwrap();
        assert!(cg.install_debug_variant(fetch).is_some());
        cg.finalize();

        let clean_addr = cg.clean_variant_addr("fetch").unwrap();
        let debug_addr = cg.debug_variant_addr("fetch").unwrap();
        assert_ne!(
            clean_addr, debug_addr,
            "the $debug wrapper is a distinct body from the clean one"
        );
    }

    #[test]
    fn install_debug_variant_wraps_a_non_sm_async_fn_as_a_spawning_wrapper() {
        // A no-await async fn now gets a real cell and its `$debug` variant
        // must compile (as a spawn-task wrapper, not the raw body -- an
        // end-to-end run confirming it actually spawns rather than running
        // inline is `tooling::dap::tests::
        // no_await_async_fn_hits_its_breakpoint_on_its_spawned_thread`,
        // which has the full runtime symbols a real call needs).
        let src = "async fn worker(n: int) -> int:\n    return n + 1\nfn main():\n    print(1)\n";
        let mut cg = debug_dual_variant_codegen(src);
        assert!(
            cg.dispatch_ids.contains_key("worker"),
            "a no-await async fn should get a dispatch cell now"
        );

        let mut functions = build_mir(src);
        Optimizer::minimal().run(&mut functions);
        crate::mir::debug_hooks::instrument(&mut functions);
        let worker_debug = functions.iter().find(|f| f.name == "worker").unwrap();
        assert!(cg.install_debug_variant(worker_debug).is_some());
        cg.finalize();

        let clean_addr = cg.clean_variant_addr("worker").unwrap();
        let debug_addr = cg.debug_variant_addr("worker").unwrap();
        assert_ne!(clean_addr, debug_addr);
    }
}
