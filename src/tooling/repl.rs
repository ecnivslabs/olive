use crate::borrow_check::BorrowChecker;
use crate::codegen::cranelift::CraneliftCodegen;
use crate::lexer::Lexer;
use crate::mir::{self, MirBuilder, Rvalue, StatementKind};
use crate::parser::{self, Parser};
use crate::semantic::{self, Resolver, TypeChecker};
use ariadne::{Label, Report, ReportKind, Source};
use rustc_hash::FxHashMap as HashMap;
use rustyline::{DefaultEditor, error::ReadlineError};

fn make_print_call(expr: parser::Expr) -> parser::Expr {
    let span = expr.span;
    parser::Expr::new(
        parser::ExprKind::Call {
            callee: Box::new(parser::Expr::new(
                parser::ExprKind::Identifier("print".to_string()),
                span,
            )),
            args: vec![parser::CallArg::Positional(expr)],
        },
        span,
    )
}

pub fn repl_compile_run(
    def_stmts: &[parser::Stmt],
    let_stmts: &[parser::Stmt],
    exec_stmts: Vec<parser::Stmt>,
    sources: &HashMap<usize, (String, String)>,
) -> bool {
    let mut combined = def_stmts.to_vec();
    combined.extend_from_slice(let_stmts);
    combined.extend(exec_stmts);
    let program = parser::Program { stmts: combined };

    let mut resolver = Resolver::new();
    resolver.resolve_program(&program);
    if !resolver.errors.is_empty() {
        for e in &resolver.errors {
            eprintln!("\x1b[1;31merror\x1b[0m: {}", e);
        }
        return false;
    }

    let mut type_checker = TypeChecker::new();
    type_checker.check_program(&program);
    if !type_checker.errors.is_empty() {
        for e in &type_checker.errors {
            let _ = Report::build(ReportKind::Error, ("<repl>", e.span().start..e.span().end))
                .with_message(format!("{}", e))
                .with_label(
                    Label::new(("<repl>", e.span().start..e.span().end))
                        .with_message(format!("{}", e)),
                )
                .finish()
                .print((
                    "<repl>",
                    Source::from(
                        sources
                            .get(&e.span().file_id)
                            .map(|(_, s)| s.as_str())
                            .unwrap_or(""),
                    ),
                ));
        }
        return false;
    }

    let mut mir_builder = MirBuilder::new(
        &type_checker.expr_types,
        &type_checker.type_env[0],
        type_checker.struct_fields.clone(),
    );
    mir_builder.build_program(&program);

    let optimizer = mir::Optimizer::new();
    optimizer.run(&mut mir_builder.functions);

    let mut had_borrow_error = false;
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
                        eprintln!("\x1b[1;31mborrow error\x1b[0m in {}: {}", func.name, msg);
                        let _ = sources.get(&span.file_id);
                    }
                    _ => eprintln!("\x1b[1;31mborrow error\x1b[0m in {}: {}", func.name, e),
                }
            }
            had_borrow_error = true;
        }
    }
    if had_borrow_error {
        return false;
    }

    let mut codegen = CraneliftCodegen::new_jit(
        mir_builder.functions,
        mir_builder.struct_fields.clone(),
        &[],
    );
    codegen.generate();
    codegen.finalize();

    if let Some(main_ptr) = codegen.get_function("__main__") {
        let main_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(main_ptr) };
        let _ = main_fn();
    }

    true
}

pub fn run_shell() {
    println!(
        "Olive {} ({}, {}) on {}",
        env!("CARGO_PKG_VERSION"),
        env!("GIT_BRANCH"),
        env!("BUILD_DATE"),
        std::env::consts::OS,
    );
    println!("Type \"help\", \"copyright\", \"credits\" or \"license\" for more information.");

    let mut rl = DefaultEditor::new().expect("failed to init readline");

    let mut def_stmts: Vec<parser::Stmt> = vec![parser::Stmt::new(
        parser::StmtKind::Const {
            name: "__name__".to_string(),
            type_ann: None,
            value: parser::Expr::new(
                parser::ExprKind::Str("__repl__".to_string()),
                crate::span::Span::default(),
            ),
        },
        crate::span::Span::default(),
    )];
    let mut let_stmts: Vec<parser::Stmt> = Vec::new();
    let mut sources: HashMap<usize, (String, String)> = HashMap::default();
    let mut file_id: usize = 0;

    loop {
        let first_line = match rl.readline(">>> ") {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("readline error: {e}");
                break;
            }
        };

        let trimmed = first_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed {
            "quit" | "exit" | "quit()" | "exit()" => break,
            "help" => {
                println!("Olive interactive shell. Type Olive code to execute it.");
                println!("Commands:");
                println!("  quit / exit    exit the shell");
                println!("  clear          clear screen and reset state");
                continue;
            }
            "copyright" => {
                println!("Copyright (c) 2024 ecnivs. MIT License.");
                continue;
            }
            "credits" => {
                println!("Built with Cranelift JIT. Thanks to the Rust ecosystem.");
                continue;
            }
            "license" => {
                println!("MIT License: see https://github.com/ecnivs/olive/blob/master/LICENSE");
                continue;
            }
            "clear" => {
                def_stmts.clear();
                def_stmts.push(parser::Stmt::new(
                    parser::StmtKind::Const {
                        name: "__name__".to_string(),
                        type_ann: None,
                        value: parser::Expr::new(
                            parser::ExprKind::Str("__repl__".to_string()),
                            crate::span::Span::default(),
                        ),
                    },
                    crate::span::Span::default(),
                ));
                let_stmts.clear();
                print!("\x1b[2J\x1b[H");
                use std::io::Write;
                std::io::stdout().flush().ok();
                continue;
            }
            _ => {}
        }

        rl.add_history_entry(&first_line).ok();

        let mut input = first_line.clone();
        if trimmed.ends_with(':') {
            loop {
                match rl.readline("... ") {
                    Ok(cont) => {
                        rl.add_history_entry(&cont).ok();
                        if cont.trim().is_empty() {
                            break;
                        }
                        input.push('\n');
                        input.push_str(&cont);
                    }
                    Err(ReadlineError::Interrupted) => break,
                    Err(_) => break,
                }
            }
        }

        let cur_file_id = file_id;
        file_id += 1;
        sources.insert(cur_file_id, ("<repl>".to_string(), input.clone()));

        let tokens = match Lexer::new(&input, cur_file_id).tokenise() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("\x1b[1;31merror\x1b[0m: {}", e.message);
                continue;
            }
        };

        let program = match Parser::new(tokens).parse_program() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("\x1b[1;31merror\x1b[0m: {}", e.message);
                continue;
            }
        };

        let mut exec_stmts: Vec<parser::Stmt> = Vec::new();
        for stmt in program.stmts {
            match &stmt.kind {
                parser::StmtKind::Fn { name, .. } => {
                    def_stmts.retain(
                        |s| !matches!(&s.kind, parser::StmtKind::Fn { name: n, .. } if n == name),
                    );
                    def_stmts.push(stmt);
                }
                parser::StmtKind::Struct { name, .. } => {
                    def_stmts.retain(|s| {
                        !matches!(&s.kind, parser::StmtKind::Struct { name: n, .. } if n == name)
                    });
                    def_stmts.push(stmt);
                }
                parser::StmtKind::Impl { type_name, .. } => {
                    def_stmts.retain(|s| {
                        !matches!(&s.kind, parser::StmtKind::Impl { type_name: n, .. } if n == type_name)
                    });
                    def_stmts.push(stmt);
                }
                parser::StmtKind::Trait { name, .. } => {
                    def_stmts.retain(|s| {
                        !matches!(&s.kind, parser::StmtKind::Trait { name: n, .. } if n == name)
                    });
                    def_stmts.push(stmt);
                }
                parser::StmtKind::Let { name, .. } => {
                    let_stmts.retain(|s| {
                        !matches!(&s.kind, parser::StmtKind::Let { name: n, .. } | parser::StmtKind::Const { name: n, .. } if n == name)
                    });
                    let_stmts.push(stmt);
                }
                parser::StmtKind::Const { name, .. } => {
                    let_stmts.retain(|s| {
                        !matches!(&s.kind, parser::StmtKind::Let { name: n, .. } | parser::StmtKind::Const { name: n, .. } if n == name)
                    });
                    let_stmts.push(stmt);
                }
                parser::StmtKind::ExprStmt(e) => {
                    let wrapped = match &e.kind {
                        parser::ExprKind::Call { .. } => stmt,
                        _ => parser::Stmt::new(
                            parser::StmtKind::ExprStmt(make_print_call(e.clone())),
                            stmt.span,
                        ),
                    };
                    exec_stmts.push(wrapped);
                }
                _ => {
                    exec_stmts.push(stmt);
                }
            }
        }

        repl_compile_run(&def_stmts, &let_stmts, exec_stmts, &sources);
    }

    println!("\nBye!");
}
