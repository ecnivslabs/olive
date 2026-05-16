mod errors;
mod linker;
mod loader;
mod pipeline;

use crate::codegen::cranelift::CraneliftCodegen;
use crate::parser;
use linker::{compute_source_hash, ensure_dir, exec_binary, link_object};
use loader::collect_source_files;
use pipeline::run_pipeline;
use std::{collections::HashSet, fs, path::Path, process};

pub fn compile_and_run(filename: &str, run: bool, show_time: bool, emit_ast: bool, emit_mir: bool) {
    let out = run_pipeline(filename);

    if emit_ast {
        println!("{:#?}", out.program);
    }

    if emit_mir {
        for f in &out.functions {
            println!("{:#?}", f);
        }
    }

    let cg_start = std::time::Instant::now();
    let mut codegen =
        CraneliftCodegen::new_jit(out.functions, out.struct_fields.clone(), &out.native_libs);
    codegen.generate();
    codegen.finalize();
    let cg_duration = cg_start.elapsed();

    if !run {
        println!("\x1b[1;32mFinished\x1b[0m build successfully.");
        if show_time {
            print_jit_timings(&out.timings, cg_duration, None);
        }
        return;
    }

    if let Some(main_ptr) = codegen.get_function("__main__") {
        let main_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(main_ptr) };
        let exec_start = std::time::Instant::now();
        let exit_code = main_fn();
        let exec_duration = exec_start.elapsed();

        if show_time {
            print_jit_timings(&out.timings, cg_duration, Some(exec_duration));
        }
        std::process::exit(exit_code as i32);
    } else {
        println!("No `main` function found to execute.");
    }
}

pub fn compile_and_emit(filename: &str, output: &str, show_time: bool) {
    let out = run_pipeline(filename);

    let cg_start = std::time::Instant::now();
    let mut codegen =
        CraneliftCodegen::new_aot(out.functions, out.struct_fields.clone(), &out.native_libs);
    codegen.generate();
    let obj_bytes = codegen.emit_object();
    let cg_duration = cg_start.elapsed();

    let link_start = std::time::Instant::now();
    if let Some(parent) = Path::new(output).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).unwrap_or_else(|e| {
                eprintln!(
                    "error: could not create output directory {}: {e}",
                    parent.display()
                );
                process::exit(1);
            });
        }
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

pub fn compile_hybrid(filename: &str, show_time: bool) {
    let mut collected = Vec::new();
    let mut visited = HashSet::new();
    collect_source_files(filename, &mut collected, &mut visited);
    let hash = compute_source_hash(&collected);

    ensure_dir("grove/.cache");

    let manifest_path = "grove/.cache/manifest.json";
    let binary_path = if cfg!(target_os = "windows") {
        "grove/.cache/program.exe"
    } else {
        "grove/.cache/program"
    };

    let cached = fs::read_to_string(manifest_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["hash"].as_u64())
        .map(|h| h == hash)
        .unwrap_or(false);

    if cached && Path::new(binary_path).exists() {
        let code = exec_binary(binary_path);
        process::exit(code);
    }

    compile_and_emit(filename, binary_path, show_time);

    let manifest = serde_json::json!({ "hash": hash });
    fs::write(manifest_path, manifest.to_string()).ok();

    let code = exec_binary(binary_path);
    process::exit(code);
}

pub fn compile_and_run_aot(filename: &str, show_time: bool) {
    let binary_path = if cfg!(target_os = "windows") {
        "grove/.cache/aot_run.exe"
    } else {
        "grove/.cache/aot_run"
    };
    ensure_dir("grove/.cache");
    compile_and_emit(filename, binary_path, show_time);
    let code = exec_binary(binary_path);
    fs::remove_file(binary_path).ok();
    process::exit(code);
}

pub fn compile_and_test(filename: &str, _show_time: bool) {
    let out = run_pipeline(filename);

    let mut codegen =
        CraneliftCodegen::new_jit(out.functions, out.struct_fields.clone(), &out.native_libs);
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
