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
fn exec_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub fn compile(src: &str) -> CraneliftCodegen<JITModule> {
    compile_with(src, Optimizer::new(), true, false)
}

/// Lean `pit run` pipeline; full opt can scalarize/inline a bug away.
pub fn compile_minimal(src: &str) -> CraneliftCodegen<JITModule> {
    compile_with(src, Optimizer::minimal(), true, false)
}

/// Same JIT pipeline as `compile`, with call-count profiling disabled. Lets
/// tests and benchmarks isolate the cost of the profiling instrumentation itself.
pub fn compile_unprofiled(src: &str) -> CraneliftCodegen<JITModule> {
    compile_with(src, Optimizer::new(), false, false)
}

/// `compile_minimal` with profiling off -- avoids the inliner folding the
/// callee away, unlike `compile`/`compile_unprofiled`.
pub fn compile_minimal_unprofiled(src: &str) -> CraneliftCodegen<JITModule> {
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
    );
    builder.build_program(&prog);
    Optimizer::minimal().run(&mut builder.functions);
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

fn compile_with(
    src: &str,
    opt: Optimizer,
    profile: bool,
    release_backend: bool,
) -> CraneliftCodegen<JITModule> {
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
    );
    builder.build_program(&prog);
    opt.run(&mut builder.functions);
    for f in &builder.functions {
        println!("MIR FUNCTION: {}\n{:#?}", f.name, f);
    }
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
    cg
}

pub fn call_i64(cg: &mut CraneliftCodegen<JITModule>, name: &str) -> i64 {
    let ptr = cg
        .get_function(name)
        .unwrap_or_else(|| panic!("function '{}' not found", name));
    let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(ptr) };
    let _guard = exec_lock();
    println!("DEBUG CALL JIT FUNCTION START: {}", name);
    let res = f();
    println!("DEBUG CALL JIT FUNCTION END: {} -> {}", name, res);
    res
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
