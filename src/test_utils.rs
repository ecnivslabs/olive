use crate::codegen::cranelift::CraneliftCodegen;
use crate::lexer::Lexer;
use crate::mir::{MirBuilder, Optimizer};
use crate::parser::Parser;
use crate::semantic::{Resolver, TypeChecker};
use cranelift_jit::JITModule;
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
    compile_with(src, Optimizer::new())
}

/// Lean `pit run` pipeline; full opt can scalarize/inline a bug away.
pub fn compile_minimal(src: &str) -> CraneliftCodegen<JITModule> {
    compile_with(src, Optimizer::minimal())
}

fn compile_with(src: &str, opt: Optimizer) -> CraneliftCodegen<JITModule> {
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
    cg
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
