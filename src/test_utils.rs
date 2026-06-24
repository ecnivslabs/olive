use crate::codegen::cranelift::CraneliftCodegen;
use crate::lexer::Lexer;
use crate::mir::{MirBuilder, Optimizer};
use crate::parser::Parser;
use crate::semantic::{Resolver, TypeChecker};
use cranelift_jit::JITModule;
use rustc_hash::FxHashSet as HashSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// The Olive runtime keeps a process-global object registry (`std_lib`'s pointer
/// bounds and active-object set), which is sound for one program per process —
/// how an Olive binary actually runs. The unit tests instead JIT and run many
/// independent programs inside the single test process, so running them at once
/// lets their registries corrupt each other. Execution is serialized through
/// this lock (compilation stays parallel) so the harness mirrors one program at
/// a time. Compilation is the slow phase, so the suite stays fast.
fn exec_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub fn compile(src: &str) -> CraneliftCodegen<JITModule> {
    let tokens = Lexer::new(src, 0).tokenise().unwrap();
    let prog = Parser::new(tokens).parse_program().unwrap();
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
    let opt = Optimizer::new();
    opt.run(&mut builder.functions);
    let mut cg = CraneliftCodegen::new_jit(
        builder.functions,
        builder.struct_fields,
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
