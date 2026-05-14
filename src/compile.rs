use crate::borrow_check::BorrowChecker;
use crate::codegen::cranelift::CraneliftCodegen;
use crate::lexer::Lexer;
use crate::mangle::mangle_statements;
use crate::mir::{self, MirBuilder, Rvalue, StatementKind};
use crate::parser::{self, Parser};
use crate::semantic::{self, Resolver, TypeChecker};
use crate::span;
use ariadne::{Label, Report, ReportKind, Source};
use rustc_hash::FxHashMap as HashMap;
use std::{collections::HashSet, fs, path::Path, process};

pub fn report_error(sources: &HashMap<usize, (String, String)>, msg: &str, span: span::Span) {
    let (filename, source) = sources
        .get(&span.file_id)
        .expect("file not found in sources");
    let _ = Report::build(ReportKind::Error, (filename.as_str(), span.start..span.end))
        .with_message(msg)
        .with_label(Label::new((filename.as_str(), span.start..span.end)).with_message(msg))
        .finish()
        .print((filename.as_str(), Source::from(source)));
}

pub fn load_and_parse(
    filename: &str,
    is_main: bool,
    loaded: &mut HashSet<String>,
    file_id_counter: &mut usize,
    sources: &mut HashMap<usize, (String, String)>,
) -> Vec<parser::Stmt> {
    let current_file_id = *file_id_counter;
    *file_id_counter += 1;

    let source = fs::read_to_string(filename).unwrap_or_else(|e| {
        eprintln!("error reading {}: {e}", filename);
        process::exit(1);
    });

    sources.insert(current_file_id, (filename.to_string(), source.clone()));

    let tokens = match Lexer::new(&source, current_file_id).tokenise() {
        Ok(t) => t,
        Err(e) => {
            report_error(
                sources,
                &e.message,
                span::Span {
                    file_id: current_file_id,
                    line: e.line,
                    col: e.col,
                    start: e.start,
                    end: e.end,
                },
            );
            process::exit(1);
        }
    };

    let program = match Parser::new(tokens).parse_program() {
        Ok(p) => p,
        Err(e) => {
            report_error(
                sources,
                &e.message,
                span::Span {
                    file_id: current_file_id,
                    line: e.line,
                    col: e.col,
                    start: e.start,
                    end: e.end,
                },
            );
            process::exit(1);
        }
    };

    let mut all_stmts = Vec::new();
    let mod_name = if is_main {
        "__main__".to_string()
    } else {
        Path::new(filename)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    };

    all_stmts.push(parser::Stmt::new(
        parser::StmtKind::Const {
            name: "__name__".to_string(),
            type_ann: None,
            value: parser::Expr::new(parser::ExprKind::Str(mod_name), span::Span::default()),
        },
        span::Span::default(),
    ));

    let parent_dir = Path::new(filename).parent().unwrap_or(Path::new("."));

    for stmt in program.stmts {
        match &stmt.kind {
            parser::StmtKind::Import { module, alias } => {
                let mod_name = module.join("/");
                let mut mod_path = parent_dir.join(format!("{}.liv", mod_name));

                if !mod_path.exists() {
                    mod_path = Path::new("lib").join(format!("{}.liv", mod_name));
                }

                if !mod_path.exists() {
                    report_error(
                        sources,
                        &format!("module '{}' not found", mod_name),
                        stmt.span,
                    );
                    process::exit(1);
                }

                let path_str = mod_path.to_string_lossy().to_string();

                if !loaded.contains(&path_str) {
                    loaded.insert(path_str.clone());
                    let mut imported_stmts =
                        load_and_parse(&path_str, false, loaded, file_id_counter, sources);

                    let mod_prefix = alias
                        .as_deref()
                        .unwrap_or_else(|| module.last().unwrap().as_str());
                    let mut defined_names = HashSet::new();
                    for s in &imported_stmts {
                        match &s.kind {
                            parser::StmtKind::Fn { name, .. }
                            | parser::StmtKind::Struct { name, .. }
                            | parser::StmtKind::Let { name, .. }
                            | parser::StmtKind::Const { name, .. } => {
                                defined_names.insert(name.clone());
                            }
                            parser::StmtKind::Impl { type_name, .. } => {
                                defined_names.insert(type_name.clone());
                            }
                            _ => {}
                        }
                    }

                    mangle_statements(&mut imported_stmts, mod_prefix, &defined_names);

                    imported_stmts.retain(|s| {
                        matches!(
                            s.kind,
                            parser::StmtKind::Fn { .. }
                                | parser::StmtKind::Struct { .. }
                                | parser::StmtKind::Impl { .. }
                                | parser::StmtKind::Trait { .. }
                                | parser::StmtKind::Let { .. }
                                | parser::StmtKind::Const { .. }
                        )
                    });

                    all_stmts.extend(imported_stmts);
                }
                all_stmts.push(stmt.clone());
            }
            parser::StmtKind::FromImport { module, names } => {
                let mod_name = module.join("/");
                let mut mod_path = parent_dir.join(format!("{}.liv", mod_name));

                if !mod_path.exists() {
                    mod_path = Path::new("lib").join(format!("{}.liv", mod_name));
                }

                if !mod_path.exists() {
                    report_error(
                        sources,
                        &format!("module '{}' not found", mod_name),
                        stmt.span,
                    );
                    process::exit(1);
                }

                let path_str = mod_path.to_string_lossy().to_string();

                if !loaded.contains(&path_str) {
                    loaded.insert(path_str.clone());
                    let mut imported_stmts =
                        load_and_parse(&path_str, false, loaded, file_id_counter, sources);

                    imported_stmts.retain(|s| match &s.kind {
                        parser::StmtKind::Fn { name, .. }
                        | parser::StmtKind::Struct { name, .. } => {
                            names.iter().any(|(n, _)| n == name)
                        }
                        parser::StmtKind::Impl { type_name, .. } => {
                            names.iter().any(|(n, _)| n == type_name)
                        }
                        _ => false,
                    });
                    all_stmts.extend(imported_stmts);
                }
                all_stmts.push(stmt.clone());
            }
            _ => all_stmts.push(stmt),
        }
    }

    all_stmts
}

pub fn compile_and_run(filename: &str, run: bool, show_time: bool, emit_ast: bool, emit_mir: bool) {
    let t0 = std::time::Instant::now();
    let mut loaded = HashSet::new();
    loaded.insert(filename.to_string());
    let mut file_id_counter = 0;
    let mut sources = HashMap::default();
    let combined_stmts = load_and_parse(
        filename,
        true,
        &mut loaded,
        &mut file_id_counter,
        &mut sources,
    );
    let program = parser::Program {
        stmts: combined_stmts,
    };
    let parse_duration = t0.elapsed();

    if emit_ast {
        println!("{:#?}", program);
    }

    let resolve_start = std::time::Instant::now();
    let mut resolver = Resolver::new();
    resolver.resolve_program(&program);

    if !resolver.errors.is_empty() {
        for e in &resolver.errors {
            report_error(&sources, &format!("{}", e), e.span());
        }
        process::exit(1);
    }
    let resolve_duration = resolve_start.elapsed();

    let typecheck_start = std::time::Instant::now();
    let mut type_checker = TypeChecker::new();
    type_checker.check_program(&program);

    if !type_checker.errors.is_empty() {
        for e in &type_checker.errors {
            report_error(&sources, &format!("{}", e), e.span());
        }
        process::exit(1);
    }
    let typecheck_duration = typecheck_start.elapsed();

    let mir_start = std::time::Instant::now();
    let mut mir_builder = MirBuilder::new(&type_checker.expr_types, &type_checker.type_env[0], type_checker.struct_fields.clone());
    mir_builder.build_program(&program);
    let mir_duration = mir_start.elapsed();

    let opt_start = std::time::Instant::now();
    let optimizer = mir::Optimizer::new();
    optimizer.run(&mut mir_builder.functions);
    let opt_duration = opt_start.elapsed();

    if emit_mir {
        for f in &mir_builder.functions {
            println!("{:#?}", f);
        }
    }

    let borrow_start = std::time::Instant::now();
    for func in &mir_builder.functions {
        let needs_check = func.locals.iter().any(|l| l.ty.is_move_type())
            || func.basic_blocks.iter().any(|bb| {
                bb.statements.iter().any(|s| {
                    matches!(
                        &s.kind,
                        StatementKind::Assign(_, Rvalue::Ref(_) | Rvalue::MutRef(_))
                    )
                })
            });
        if !needs_check {
            continue;
        }
        let mut checker = BorrowChecker::new(func);
        checker.check();
        if !checker.errors.is_empty() {
            for e in &checker.errors {
                match e {
                    semantic::SemanticError::Custom { msg, span } => {
                        report_error(
                            &sources,
                            &format!("borrow error in {}: {}", func.name, msg),
                            *span,
                        );
                    }
                    _ => report_error(
                        &sources,
                        &format!("borrow error in {}: {}", func.name, e),
                        e.span(),
                    ),
                }
            }
            process::exit(1);
        }
    }
    let borrow_duration = borrow_start.elapsed();

    let cg_start = std::time::Instant::now();
    let mut codegen = CraneliftCodegen::new(&mir_builder.functions, mir_builder.struct_fields.clone());
    codegen.generate();
    codegen.finalize();
    let cg_duration = cg_start.elapsed();

    if !run {
        println!("\x1b[1;32mFinished\x1b[0m build successfully.");
        if show_time {
            println!("\n\x1b[1;32m   Olive Build Report\x1b[0m");
            println!("\x1b[1;34m   ────────────────────────\x1b[0m");
            println!("   \x1b[1mParse:        \x1b[0m {:?}", parse_duration);
            println!("   \x1b[1mResolver:     \x1b[0m {:?}", resolve_duration);
            println!("   \x1b[1mType Check:   \x1b[0m {:?}", typecheck_duration);
            println!("   \x1b[1mMIR Build:    \x1b[0m {:?}", mir_duration);
            println!("   \x1b[1mOptimization: \x1b[0m {:?}", opt_duration);
            println!("   \x1b[1mBorrow Check: \x1b[0m {:?}", borrow_duration);
            println!("   \x1b[1mCodegen (JIT):\x1b[0m {:?}", cg_duration);
            println!("\x1b[1;34m   ────────────────────────\x1b[0m");
        }
        return;
    }

    if let Some(main_ptr) = codegen.get_function("__main__") {
        let main_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(main_ptr) };
        let exec_start = std::time::Instant::now();
        let _result = main_fn();
        let exec_duration = exec_start.elapsed();

        if show_time {
            println!("\n\x1b[1;32m   Olive Execution Report\x1b[0m");
            println!("\x1b[1;34m   ────────────────────────\x1b[0m");
            println!("   \x1b[1mParse:        \x1b[0m {:?}", parse_duration);
            println!("   \x1b[1mResolver:     \x1b[0m {:?}", resolve_duration);
            println!("   \x1b[1mType Check:   \x1b[0m {:?}", typecheck_duration);
            println!("   \x1b[1mMIR Build:    \x1b[0m {:?}", mir_duration);
            println!("   \x1b[1mOptimization: \x1b[0m {:?}", opt_duration);
            println!("   \x1b[1mBorrow Check: \x1b[0m {:?}", borrow_duration);
            println!("   \x1b[1mCodegen (JIT):\x1b[0m {:?}", cg_duration);
            println!("   \x1b[1mExecution:    \x1b[0m {:?}", exec_duration);
            println!("\x1b[1;34m   ────────────────────────\x1b[0m");
            println!(
                "   \x1b[1mTotal Startup:\x1b[0m {:?}",
                parse_duration + resolve_duration + typecheck_duration + mir_duration + opt_duration + borrow_duration + cg_duration
            );
            println!();
        }
    } else {
        println!("No `main` function found to execute.");
    }
}

pub fn compile_and_test(filename: &str, _show_time: bool) {
    let mut loaded = HashSet::new();
    loaded.insert(filename.to_string());
    let mut file_id_counter = 0;
    let mut sources = HashMap::default();
    let combined_stmts = load_and_parse(
        filename,
        true,
        &mut loaded,
        &mut file_id_counter,
        &mut sources,
    );
    let program = parser::Program {
        stmts: combined_stmts,
    };

    let mut resolver = Resolver::new();
    resolver.resolve_program(&program);

    if !resolver.errors.is_empty() {
        for e in &resolver.errors {
            report_error(&sources, &format!("{}", e), e.span());
        }
        process::exit(1);
    }

    let mut type_checker = TypeChecker::new();
    type_checker.check_program(&program);

    if !type_checker.errors.is_empty() {
        for e in &type_checker.errors {
            report_error(&sources, &format!("{}", e), e.span());
        }
        process::exit(1);
    }

    let mut mir_builder = MirBuilder::new(&type_checker.expr_types, &type_checker.type_env[0], type_checker.struct_fields.clone());
    mir_builder.build_program(&program);

    let optimizer = mir::Optimizer::new();
    optimizer.run(&mut mir_builder.functions);

    for func in &mir_builder.functions {
        let mut checker = BorrowChecker::new(func);
        checker.check();
        if !checker.errors.is_empty() {
            for e in &checker.errors {
                report_error(
                    &sources,
                    &format!("borrow error in {}: {}", func.name, e),
                    e.span(),
                );
            }
            process::exit(1);
        }
    }

    let mut codegen = CraneliftCodegen::new(&mir_builder.functions, mir_builder.struct_fields.clone());
    codegen.generate();
    codegen.finalize();

    println!("\x1b[1;34mRunning tests...\x1b[0m\n");
    let mut passed = 0;
    let mut failed = 0;

    for stmt in &program.stmts {
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
                let _res = func();
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
