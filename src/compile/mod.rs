pub(crate) mod cache;
pub(crate) mod errors;
pub(crate) mod fix;
pub(crate) mod laws;
mod linker;
pub(crate) mod lints;
pub(crate) mod loader;
pub(crate) mod pgo;
pub(crate) mod pipeline;
#[cfg(test)]
mod tests;

use crate::codegen::cranelift::CraneliftCodegen;
use crate::parser;
use linker::{ensure_dir, exec_binary, link_object};
use pipeline::run_pipeline_opt;
use std::{fs, path::Path, process};

/// Compiles `filename` via JIT and executes its `main`, returning the
/// program's exit code instead of terminating the process. This lets a
/// caller run a script (e.g. a build script) and decide what to do next
/// based on the result, rather than the whole `pit` process dying inside
/// what's meant to be a sub-step.
fn run_jit_to_exit_code(
    filename: &str,
    show_time: bool,
    emit_ast: bool,
    emit_mir: bool,
    release: bool,
    write_profile: bool,
    explain_copies: bool,
) -> i32 {
    let out = match run_pipeline_opt(filename, release, None, explain_copies) {
        Ok(o) => o,
        Err(_) => return 1,
    };

    if emit_ast {
        println!("{:#?}", out.program);
    }

    if emit_mir {
        for f in &out.functions {
            println!("{:#?}", f);
        }
    }

    // `write_profile` gates PGO for this run entirely: a build script isn't
    // the program PGO is meant to capture, so it neither reads nor writes one.
    let target = write_profile.then(|| cache::prepare(filename, release).0);

    let cg_start = std::time::Instant::now();
    let mut codegen = CraneliftCodegen::new_jit(
        out.functions,
        out.struct_fields.clone(),
        out.field_types.clone(),
        out.enum_defs.clone(),
        out.vtables.clone(),
        out.global_vars.clone(),
        out.file_names.clone(),
        &out.native_libs,
        release,
    );
    // Auto-pickup, symmetric with the write-on-exit below: a repeated run of
    // the same file starts pre-specialized instead of re-observing from cold.
    if let Some(t) = &target
        && let Some(path) = pgo::auto_detect(t.hash())
        && let Some(profile) = pgo::load(&path)
    {
        let applied = codegen.apply_profile(&profile);
        if applied > 0 {
            println!("\x1b[1;32m   PGO\x1b[0m applied {applied} specialization(s) from {path}");
        }
    }
    codegen.generate();
    codegen.finalize();
    let cg_duration = cg_start.elapsed();

    let Some(main_ptr) = codegen.get_function("__main__") else {
        println!("No `main` function found to execute.");
        return 0;
    };

    // Handed off to the background tier-up thread from here on; the main thread
    // never touches `codegen` directly again, only the raw `main_ptr` obtained above.
    let codegen = std::sync::Arc::new(std::sync::Mutex::new(codegen));
    let _tier_up_handle = crate::codegen::cranelift::tier_up::spawn_tier_up_thread(codegen.clone());

    let main_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(main_ptr) };
    let exec_start = std::time::Instant::now();
    let exit_code = main_fn();
    let exec_duration = exec_start.elapsed();

    // Finalize the Python interpreter for atexit handlers. No-op if never initialized.
    finalize_python_runtime();

    // Best-effort: a failed write must never affect the program's exit code.
    if let Some(t) = &target
        && let Ok(mut cg) = codegen.lock()
    {
        let profile = cg.export_profile();
        pgo::write(&profile, &pgo::path_for_hash(t.hash()));
    }

    if show_time {
        print_jit_timings(&out.timings, cg_duration, Some(exec_duration));
    }
    exit_code as i32
}

pub fn compile_and_run(
    filename: &str,
    show_time: bool,
    emit_ast: bool,
    emit_mir: bool,
    release: bool,
    explain_copies: bool,
) {
    let code = run_jit_to_exit_code(
        filename,
        show_time,
        emit_ast,
        emit_mir,
        release,
        true,
        explain_copies,
    );
    std::process::exit(code);
}

/// Runs a script (e.g. `build.liv`) to completion and returns its exit code
/// without terminating the process, so the caller can continue afterward.
/// No PGO write -- a build script isn't the program `--pgo` is meant to capture.
pub fn run_script(filename: &str, show_time: bool, release: bool) -> i32 {
    run_jit_to_exit_code(filename, show_time, false, false, release, false, false)
}

/// Calls `olive_py_finalize` via `dlsym(RTLD_DEFAULT)`, working whether
/// `olive_std` is linked statically or loaded by the JIT.
fn finalize_python_runtime() {
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn dlsym(
                handle: *mut std::ffi::c_void,
                symbol: *const std::ffi::c_char,
            ) -> *mut std::ffi::c_void;
        }
        let sym = dlsym(std::ptr::null_mut(), c"olive_py_finalize".as_ptr());
        if !sym.is_null() {
            let f: extern "C" fn() = std::mem::transmute(sym);
            f();
        }
    }
}

pub fn compile_and_emit(
    filename: &str,
    output: &str,
    show_time: bool,
    release: bool,
    pgo: Option<&str>,
    explain_copies: bool,
) {
    // Loaded early: feeds both the inliner's hot-function threshold below
    // and `apply_profile` later, one file read for both.
    let profile = pgo.and_then(|path| match pgo::load(path) {
        Some(p) => Some(p),
        None => {
            eprintln!("warning: could not read PGO profile at {path}, building without it");
            None
        }
    });
    let hot_functions = profile.as_ref().map(pgo::hot_functions);

    let out = match run_pipeline_opt(filename, release, hot_functions, explain_copies) {
        Ok(o) => o,
        Err(_) => std::process::exit(1),
    };

    let cg_start = std::time::Instant::now();
    let mut codegen = CraneliftCodegen::new_aot(
        out.functions,
        out.struct_fields.clone(),
        out.field_types.clone(),
        out.enum_defs.clone(),
        out.vtables.clone(),
        out.global_vars.clone(),
        out.file_names.clone(),
        &out.native_libs,
        release,
    );
    // Must run before `generate()` -- seeds `specialize_sites` for translation.
    if let Some(profile) = &profile {
        let applied = codegen.apply_profile(profile);
        let path = pgo.expect("profile is only Some when pgo path was Some");
        println!("\x1b[1;32m   PGO\x1b[0m applied {applied} specialization(s) from {path}");
    }
    codegen.generate();
    let obj_bytes = codegen.emit_object();
    let cg_duration = cg_start.elapsed();

    let link_start = std::time::Instant::now();
    if let Some(parent) = Path::new(output)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).unwrap_or_else(|e| {
            eprintln!(
                "error: could not create output directory {}: {e}",
                parent.display()
            );
            process::exit(1);
        });
    }

    let obj_path = format!("{}.o", output);
    fs::write(&obj_path, &obj_bytes).unwrap_or_else(|e| {
        eprintln!("error: could not write object file: {e}");
        process::exit(1);
    });

    link_object(&obj_path, output, &out.native_libs);
    let link_duration = link_start.elapsed();

    println!("\x1b[1;32mFinished\x1b[0m build `{}` successfully.", output);
    if show_time {
        print_aot_timings(&out.timings, cg_duration, link_duration);
    }
}

pub fn compile_hybrid(filename: &str, show_time: bool, release: bool, explain_copies: bool) {
    let (target, py_files) = cache::prepare(filename, release);

    if cache::is_fresh(&target) {
        let code = exec_binary(&target.binary_path);
        process::exit(code);
    }

    // Invalidate stale .pyc bytecode for all referenced Python modules so
    // Python always recompiles from the current source on the next run.
    for py_path in &py_files {
        invalidate_pyc(py_path);
    }

    // Auto-pickup: reuse a profile from an earlier `pit run`, no flag needed.
    let pgo_arg = pgo::auto_detect(target.hash());
    compile_and_emit(
        filename,
        &target.binary_path,
        show_time,
        release,
        pgo_arg.as_deref(),
        explain_copies,
    );
    cache::record(&target);

    let code = exec_binary(&target.binary_path);
    process::exit(code);
}

/// Delete the Python bytecode cache (.pyc) for a given .py source file so
/// Python is forced to recompile from source on next import.
fn invalidate_pyc(py_path: &str) {
    use std::path::Path;
    let p = Path::new(py_path);
    let stem = match p.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return,
    };
    let parent = p.parent().unwrap_or(Path::new("."));
    // Python stores .pyc files in __pycache__/<stem>.cpython-<ver>.pyc
    let pycache = parent.join("__pycache__");
    if let Ok(entries) = fs::read_dir(&pycache) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with(stem) && name_str.ends_with(".pyc") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

pub fn compile_and_run_aot(filename: &str, show_time: bool, release: bool, explain_copies: bool) {
    let binary_path = if cfg!(target_os = "windows") {
        "grove/cache/aot_run.exe"
    } else {
        "grove/cache/aot_run"
    };
    ensure_dir("grove/cache");
    compile_and_emit(
        filename,
        binary_path,
        show_time,
        release,
        None,
        explain_copies,
    );
    let code = exec_binary(binary_path);
    fs::remove_file(binary_path).ok();
    process::exit(code);
}

pub fn compile_and_test(filename: &str, _show_time: bool, release: bool, _explain_copies: bool) {
    let out = match run_pipeline_opt(filename, release, None, _explain_copies) {
        Ok(o) => o,
        Err(_) => std::process::exit(1),
    };

    let mut codegen = CraneliftCodegen::new_jit(
        out.functions,
        out.struct_fields.clone(),
        out.field_types.clone(),
        out.enum_defs.clone(),
        out.vtables.clone(),
        out.global_vars.clone(),
        out.file_names.clone(),
        &out.native_libs,
        release,
    );
    codegen.generate();
    codegen.finalize();

    println!("\x1b[1;34mRunning tests...\x1b[0m\n");
    let mut passed = 0;
    let mut failed = 0;

    for stmt in &out.program.stmts {
        if let parser::StmtKind::Fn {
            name, decorators, ..
        } = &stmt.kind
            && decorators
                .iter()
                .any(|d| d.name == "test" && d.is_directive)
        {
            print!("test {} ... ", name);
            std::io::Write::flush(&mut std::io::stdout()).unwrap();

            if let Some(func_ptr) = codegen.get_function(name) {
                let func: extern "C" fn() -> i64 = unsafe { std::mem::transmute(func_ptr) };

                let start = std::time::Instant::now();
                func();
                let duration = start.elapsed();

                println!("\x1b[1;32mok\x1b[0m ({:?})", duration);
                passed += 1;
            } else {
                println!("\x1b[1;31mfailed\x1b[0m (not found)");
                failed += 1;
            }
        }
    }

    println!(
        "\ntest result: {}. \x1b[1;32m{} passed\x1b[0m; \x1b[1;31m{} failed\x1b[0m\n",
        if failed == 0 {
            "\x1b[1;32mok\x1b[0m"
        } else {
            "\x1b[1;31mFAILED\x1b[0m"
        },
        passed,
        failed
    );
    if failed > 0 {
        process::exit(1);
    }
}

fn print_jit_timings(
    t: &pipeline::PipelineTimings,
    cg: std::time::Duration,
    exec: Option<std::time::Duration>,
) {
    if let Some(exec_duration) = exec {
        println!("\n\x1b[1;32m   Olive Execution Report\x1b[0m");
        println!("\x1b[1;34m   ────────────────────────\x1b[0m");
        println!("   \x1b[1mParse:        \x1b[0m {:?}", t.parse);
        println!("   \x1b[1mResolver:     \x1b[0m {:?}", t.resolve);
        println!("   \x1b[1mType Check:   \x1b[0m {:?}", t.typecheck);
        println!("   \x1b[1mMIR Build:    \x1b[0m {:?}", t.mir);
        println!("   \x1b[1mOptimization: \x1b[0m {:?}", t.optimize);
        println!("   \x1b[1mBorrow Check: \x1b[0m {:?}", t.borrow_check);
        println!("   \x1b[1mCodegen (JIT):\x1b[0m {:?}", cg);
        println!("   \x1b[1mExecution:    \x1b[0m {:?}", exec_duration);
        println!("\x1b[1;34m   ────────────────────────\x1b[0m");
        println!(
            "   \x1b[1mTotal Startup:\x1b[0m {:?}",
            t.parse + t.resolve + t.typecheck + t.mir + t.optimize + t.borrow_check + cg
        );
        println!();
    } else {
        println!("\n\x1b[1;32m   Olive Build Report\x1b[0m");
        println!("\x1b[1;34m   ────────────────────────\x1b[0m");
        println!("   \x1b[1mParse:        \x1b[0m {:?}", t.parse);
        println!("   \x1b[1mResolver:     \x1b[0m {:?}", t.resolve);
        println!("   \x1b[1mType Check:   \x1b[0m {:?}", t.typecheck);
        println!("   \x1b[1mMIR Build:    \x1b[0m {:?}", t.mir);
        println!("   \x1b[1mOptimization: \x1b[0m {:?}", t.optimize);
        println!("   \x1b[1mBorrow Check: \x1b[0m {:?}", t.borrow_check);
        println!("   \x1b[1mCodegen (JIT):\x1b[0m {:?}", cg);
        println!("\x1b[1;34m   ────────────────────────\x1b[0m");
    }
}

fn print_aot_timings(
    t: &pipeline::PipelineTimings,
    cg: std::time::Duration,
    link: std::time::Duration,
) {
    println!("\n\x1b[1;32m   Olive Build Report (AOT)\x1b[0m");
    println!("\x1b[1;34m   ────────────────────────\x1b[0m");
    println!("   \x1b[1mParse:        \x1b[0m {:?}", t.parse);
    println!("   \x1b[1mResolver:     \x1b[0m {:?}", t.resolve);
    println!("   \x1b[1mType Check:   \x1b[0m {:?}", t.typecheck);
    println!("   \x1b[1mMIR Build:    \x1b[0m {:?}", t.mir);
    println!("   \x1b[1mOptimization: \x1b[0m {:?}", t.optimize);
    println!("   \x1b[1mBorrow Check: \x1b[0m {:?}", t.borrow_check);
    println!("   \x1b[1mCodegen (AOT):\x1b[0m {:?}", cg);
    println!("   \x1b[1mLink:         \x1b[0m {:?}", link);
    println!("\x1b[1;34m   ────────────────────────\x1b[0m");
}
