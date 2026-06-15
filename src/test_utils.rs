use crate::codegen::cranelift::CraneliftCodegen;
use crate::lexer::Lexer;
use crate::mir::{MirBuilder, Optimizer};
use crate::parser::Parser;
use crate::semantic::{Resolver, TypeChecker};
use cranelift_jit::JITModule;
use cranelift_object::ObjectModule;
use rustc_hash::FxHashSet as HashSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Runtime registry is process-global; serialize exec so concurrent JIT'd
/// programs don't corrupt each other's registry. Compilation stays parallel.
pub(crate) fn exec_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Owns a JIT codegen instance and unmaps its executable memory on drop.
/// cranelift leaks JITModule mappings unless `free_memory` runs; per-case
/// compiles in proptest suites exhaust commit space without this (Windows CI).
pub struct JitInstance(Option<CraneliftCodegen<JITModule>>);

impl std::ops::Deref for JitInstance {
    type Target = CraneliftCodegen<JITModule>;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for JitInstance {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().unwrap()
    }
}

impl Drop for JitInstance {
    fn drop(&mut self) {
        if let Some(cg) = self.0.take() {
            // Safety: every pointer into this module dies with the instance;
            // tests never leave threads or handlers holding its code.
            unsafe { cg.into_module().free_memory() }
        }
    }
}

pub fn compile(src: &str) -> JitInstance {
    compile_with(src, Optimizer::new(), true, false)
}

/// Lean `pit run` pipeline; full opt can scalarize/inline a bug away.
pub fn compile_minimal(src: &str) -> JitInstance {
    compile_with(src, Optimizer::minimal(), true, false)
}

/// Same JIT pipeline as `compile`, with call-count profiling disabled. Lets
/// tests and benchmarks isolate the cost of the profiling instrumentation itself.
pub fn compile_unprofiled(src: &str) -> JitInstance {
    compile_with(src, Optimizer::new(), false, false)
}

/// `compile_minimal` with profiling off -- avoids the inliner folding the
/// callee away, unlike `compile`/`compile_unprofiled`.
pub fn compile_minimal_unprofiled(src: &str) -> JitInstance {
    compile_with(src, Optimizer::minimal(), false, false)
}

/// AOT codegen without `generate()`, for `apply_profile` tests that mutate state first.
pub fn compile_minimal_aot(src: &str) -> CraneliftCodegen<ObjectModule> {
    let tokens = Lexer::new(src, 0).tokenise().unwrap();
    let mut prog = Parser::new(tokens).parse_program().unwrap();
    crate::semantic::desugar::desugar_trait_defaults(&mut prog);
    crate::semantic::desugar::desugar_bare_variants(&mut prog);
    let mut r = Resolver::new();
    r.resolve_program(&prog);
    assert!(r.errors.is_empty(), "resolver errors: {:?}", r.errors);
    let mut tc = TypeChecker::new();
    tc.check_program(&prog);
    assert!(tc.errors.is_empty(), "type errors: {:?}", tc.errors);
    let mut builder = MirBuilder::new(
        &tc.expr_types,
        &tc.expr_kwarg_maps,
        &tc.type_env[0],
        tc.struct_fields.clone(),
        &tc.traits,
        HashSet::default(),
        tc.enum_defs.clone(),
    );
    builder.struct_field_types = tc.field_types.clone();
    builder.build_program(&prog);
    builder.monomorphize_drop_fns();
    let (_diags, _copy_sites) = Optimizer::minimal().run(&mut builder.functions);
    CraneliftCodegen::new_aot(
        builder.functions,
        builder.struct_fields,
        tc.field_types.clone(),
        tc.enum_defs.clone(),
        builder.vtables.clone(),
        builder.global_vars,
        builder.file_names.clone(),
        &[],
        false,
    )
}

/// Instruments `src`'s MIR with the debugger hooks (lean pipeline, same as
/// a real debug session) before JIT codegen, for tests that need a running
/// program rather than just the MIR shape `debug_hooks::instrument` leaves behind.
pub fn compile_instrumented(src: &str) -> (JitInstance, crate::mir::debug_hooks::DebugProgramInfo) {
    let tokens = Lexer::new(src, 0).tokenise().unwrap();
    let mut prog = Parser::new(tokens).parse_program().unwrap();
    crate::semantic::desugar::desugar_trait_defaults(&mut prog);
    crate::semantic::desugar::desugar_bare_variants(&mut prog);
    let mut r = Resolver::new();
    r.resolve_program(&prog);
    assert!(r.errors.is_empty(), "resolver errors: {:?}", r.errors);
    let mut tc = TypeChecker::new();
    tc.check_program(&prog);
    assert!(tc.errors.is_empty(), "type errors: {:?}", tc.errors);
    let mut builder = MirBuilder::new(
        &tc.expr_types,
        &tc.expr_kwarg_maps,
        &tc.type_env[0],
        tc.struct_fields.clone(),
        &tc.traits,
        HashSet::default(),
        tc.enum_defs.clone(),
    );
    builder.struct_field_types = tc.field_types.clone();
    builder.build_program(&prog);
    builder.monomorphize_drop_fns();
    Optimizer::minimal().run(&mut builder.functions);
    let program = crate::mir::debug_hooks::instrument(&mut builder.functions);
    let mut cg = CraneliftCodegen::new_jit(
        builder.functions,
        builder.struct_fields,
        tc.field_types.clone(),
        tc.enum_defs.clone(),
        builder.vtables.clone(),
        builder.global_vars,
        builder.file_names.clone(),
        &[],
        false,
    );
    cg.generate();
    cg.finalize();
    (JitInstance(Some(cg)), program)
}

fn compile_with(src: &str, opt: Optimizer, profile: bool, release_backend: bool) -> JitInstance {
    let tokens = Lexer::new(src, 0).tokenise().unwrap();
    let mut prog = Parser::new(tokens).parse_program().unwrap();
    crate::semantic::desugar::desugar_trait_defaults(&mut prog);
    crate::semantic::desugar::desugar_bare_variants(&mut prog);
    let mut r = Resolver::new();
    r.resolve_program(&prog);
    assert!(r.errors.is_empty(), "resolver errors: {:?}", r.errors);
    let mut tc = TypeChecker::new();
    tc.check_program(&prog);
    assert!(tc.errors.is_empty(), "type errors: {:?}", tc.errors);
    let mut builder = MirBuilder::new(
        &tc.expr_types,
        &tc.expr_kwarg_maps,
        &tc.type_env[0],
        tc.struct_fields.clone(),
        &tc.traits,
        HashSet::default(),
        tc.enum_defs.clone(),
    );
    builder.struct_field_types = tc.field_types.clone();
    builder.build_program(&prog);
    builder.monomorphize_drop_fns();
    let (_diags, _copy_sites) = opt.run(&mut builder.functions);
    let mut cg = CraneliftCodegen::new_jit(
        builder.functions,
        builder.struct_fields,
        tc.field_types.clone(),
        tc.enum_defs.clone(),
        builder.vtables.clone(),
        builder.global_vars,
        builder.file_names.clone(),
        &[],
        release_backend,
    );
    cg.profile = profile;
    cg.generate();
    cg.finalize();
    JitInstance(Some(cg))
}

pub fn call_i64(cg: &mut CraneliftCodegen<JITModule>, name: &str) -> i64 {
    let ptr = cg
        .get_function(name)
        .unwrap_or_else(|| panic!("function '{}' not found", name));
    let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(ptr) };
    let _guard = exec_lock();
    f()
}

pub fn call_i64_1(cg: &mut CraneliftCodegen<JITModule>, name: &str, a: i64) -> i64 {
    let ptr = cg
        .get_function(name)
        .unwrap_or_else(|| panic!("function '{}' not found", name));
    let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    let _guard = exec_lock();
    f(a)
}

pub fn call_i64_2(cg: &mut CraneliftCodegen<JITModule>, name: &str, a: i64, b: i64) -> i64 {
    let ptr = cg
        .get_function(name)
        .unwrap_or_else(|| panic!("function '{}' not found", name));
    let f: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    let _guard = exec_lock();
    f(a, b)
}

pub fn call_i64_3(cg: &mut CraneliftCodegen<JITModule>, name: &str, a: i64, b: i64, c: i64) -> i64 {
    let ptr = cg
        .get_function(name)
        .unwrap_or_else(|| panic!("function '{}' not found", name));
    let f: extern "C" fn(i64, i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
    let _guard = exec_lock();
    f(a, b, c)
}

/// Builds MIR only (no optimization, no codegen) -- the exact shape the
/// borrow checker sees. For tests asserting *how* something lowers (e.g. a
/// `for`-loop borrows its iterable via `Rvalue::Ref` instead of copying it)
/// rather than what it computes.
/// Runs already-built MIR (e.g. `debug_hooks::instrument_clean`'s output)
/// straight through codegen, skipping the frontend entirely. Empty
/// struct/enum/vtable/global metadata -- only valid for programs with none
/// of those, which is all `build_mir`'s callers need since it discards them
/// too.
pub fn compile_prebuilt(functions: Vec<crate::mir::ir::MirFunction>) -> JitInstance {
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
    cg.generate();
    cg.finalize();
    JitInstance(Some(cg))
}

pub fn build_mir(src: &str) -> Vec<crate::mir::ir::MirFunction> {
    let tokens = Lexer::new(src, 0).tokenise().unwrap();
    let mut prog = Parser::new(tokens).parse_program().unwrap();
    crate::semantic::desugar::desugar_trait_defaults(&mut prog);
    crate::semantic::desugar::desugar_bare_variants(&mut prog);
    let mut r = Resolver::new();
    r.resolve_program(&prog);
    assert!(r.errors.is_empty(), "resolver errors: {:?}", r.errors);
    let mut tc = TypeChecker::new();
    tc.check_program(&prog);
    assert!(tc.errors.is_empty(), "type errors: {:?}", tc.errors);
    let mut builder = MirBuilder::new(
        &tc.expr_types,
        &tc.expr_kwarg_maps,
        &tc.type_env[0],
        tc.struct_fields.clone(),
        &tc.traits,
        HashSet::default(),
        tc.enum_defs.clone(),
    );
    builder.struct_field_types = tc.field_types.clone();
    builder.build_program(&prog);
    builder.monomorphize_drop_fns();
    builder.functions
}

/// Builds MIR and runs the borrow checker (the pass `pipeline.rs` runs
/// between MIR build and optimization), returning the codes it raised.
/// `check_codes` alone can't see these: exclusivity violations (E05xx) are
/// a MIR-level pass, not part of the type checker.
pub fn check_borrow_codes(src: &str) -> Vec<String> {
    let tokens = Lexer::new(src, 0).tokenise().unwrap();
    let mut prog = Parser::new(tokens).parse_program().unwrap();
    crate::semantic::desugar::desugar_trait_defaults(&mut prog);
    crate::semantic::desugar::desugar_bare_variants(&mut prog);
    let mut r = Resolver::new();
    r.resolve_program(&prog);
    assert!(r.errors.is_empty(), "resolver errors: {:?}", r.errors);
    let mut tc = TypeChecker::new();
    tc.check_program(&prog);
    assert!(tc.errors.is_empty(), "type errors: {:?}", tc.errors);
    let mut builder = MirBuilder::new(
        &tc.expr_types,
        &tc.expr_kwarg_maps,
        &tc.type_env[0],
        tc.struct_fields.clone(),
        &tc.traits,
        HashSet::default(),
        tc.enum_defs.clone(),
    );
    builder.struct_field_types = tc.field_types.clone();
    builder.build_program(&prog);
    builder.monomorphize_drop_fns();
    let mut codes = Vec::new();
    for func in &builder.functions {
        let mut checker = crate::borrow_check::BorrowChecker::new(func, &tc.struct_fields);
        checker.check();
        for e in &checker.errors {
            if let crate::semantic::SemanticError::Rich(d) = e
                && let Some(c) = d.code()
            {
                codes.push(c.to_string());
            }
        }
    }
    codes
}

/// Runs resolve + typecheck only (no MIR/codegen) and returns the sorted
/// diagnostic codes produced, for tests asserting a program is rejected
/// with a specific code rather than asserting a runtime result.
pub fn check_codes(src: &str) -> Vec<String> {
    let tokens = Lexer::new(src, 0).tokenise().unwrap();
    let mut prog = Parser::new(tokens).parse_program().unwrap();
    crate::semantic::desugar::desugar_trait_defaults(&mut prog);
    crate::semantic::desugar::desugar_bare_variants(&mut prog);
    let mut r = Resolver::new();
    r.resolve_program(&prog);
    let mut tc = TypeChecker::new();
    tc.check_program(&prog);
    tc.errors
        .iter()
        .filter_map(|e| match e {
            crate::semantic::SemanticError::Rich(d) => d.code().map(str::to_string),
            _ => None,
        })
        .collect()
}

/// Runs the closure-capture pass only (E0423/E0424), not the type checker.
/// `check_codes` doesn't cover this pass, mirroring `check_borrow_codes`
/// existing separately from it for the same reason.
pub fn check_closure_codes(src: &str) -> Vec<String> {
    let tokens = Lexer::new(src, 0).tokenise().unwrap();
    let prog = Parser::new(tokens).parse_program().unwrap();
    crate::semantic::closure_check::check_closures(&prog)
        .iter()
        .filter_map(|e| match e {
            crate::semantic::SemanticError::Rich(d) => d.code().map(str::to_string),
            _ => None,
        })
        .collect()
}
