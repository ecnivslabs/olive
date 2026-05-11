mod borrow_check;
mod codegen;
mod lexer;
mod mir;
mod parser;
mod semantic;
mod span;

use ariadne::{Label, Report, ReportKind, Source};
use borrow_check::BorrowChecker;
use clap::{Parser as ClapParser, Subcommand};
use codegen::cranelift::CraneliftCodegen;
use lexer::{Lexer, TokenKind};
use mir::MirBuilder;
use parser::Parser;
use rustc_hash::FxHashMap as HashMap;
use semantic::{Resolver, TypeChecker};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path, process};

#[derive(Serialize, Deserialize, Debug)]
struct Config {
    package: Package,
}

#[derive(Serialize, Deserialize, Debug)]
struct Package {
    name: String,
    version: String,
    #[serde(default = "default_entry")]
    entry: String,
}

fn default_entry() -> String {
    "src/main.liv".to_string()
}

#[derive(ClapParser, Debug)]
#[command(name = "pit", version, about = "The Olive programming language toolchain", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create a new olive project
    New { name: String },
    /// Build the current project (checks for errors)
    Build {
        #[arg(short, long)]
        time: bool,
    },
    Run {
        /// The file to run (optional if in a project)
        file: Option<String>,

        #[arg(short, long)]
        time: bool,

        #[arg(long)]
        emit_ast: bool,

        #[arg(long)]
        emit_mir: bool,
    },
    /// Format the current project or a specific file
    Format {
        /// The file to format (optional if in a project)
        file: Option<String>,
    },
    /// Run tests in the current project
    Test {
        #[arg(short, long)]
        time: bool,
    },
}

fn report_error(sources: &HashMap<usize, (String, String)>, msg: &str, span: span::Span) {
    let (filename, source) = sources
        .get(&span.file_id)
        .expect("file not found in sources");
    let _ = Report::build(ReportKind::Error, (filename.as_str(), span.start..span.end))
        .with_message(msg)
        .with_label(Label::new((filename.as_str(), span.start..span.end)).with_message(msg))
        .finish()
        .print((filename.as_str(), Source::from(source)));
}

fn load_and_parse(
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
            value: parser::Expr::new(
                parser::ExprKind::Str(mod_name),
                span::Span::default(),
            ),
        },
        span::Span::default(),
    ));

    let parent_dir = Path::new(filename).parent().unwrap_or(Path::new("."));

    for stmt in program.stmts {
        match &stmt.kind {
            parser::StmtKind::Import(parts) => {
                let mod_name = parts.join("/");
                let mut mod_path = parent_dir.join(format!("{}.liv", mod_name));
                
                if !mod_path.exists() {
                    // Try std lib in /lib
                    mod_path = Path::new("lib").join(format!("{}.liv", mod_name));
                }

                if !mod_path.exists() {
                    report_error(sources, &format!("module '{}' not found", mod_name), stmt.span);
                    process::exit(1);
                }

                let path_str = mod_path.to_string_lossy().to_string();

                if !loaded.contains(&path_str) {
                    loaded.insert(path_str.clone());
                    let mut imported_stmts =
                        load_and_parse(&path_str, false, loaded, file_id_counter, sources);
                    
                    // Mangling for namespacing: module::name
                    let mod_prefix = parts.last().unwrap();
                    let mut defined_names = HashSet::new();
                    for s in &imported_stmts {
                        match &s.kind {
                            parser::StmtKind::Fn { name, .. }
                            | parser::StmtKind::Class { name, .. }
                            | parser::StmtKind::Let { name, .. }
                            | parser::StmtKind::Const { name, .. } => {
                                defined_names.insert(name.clone());
                            }
                            _ => {}
                        }
                    }

                    mangle_statements(&mut imported_stmts, mod_prefix, &defined_names);

                    imported_stmts.retain(|s| {
                        matches!(
                            s.kind,
                            parser::StmtKind::Fn { .. }
                                | parser::StmtKind::Class { .. }
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
                    report_error(sources, &format!("module '{}' not found", mod_name), stmt.span);
                    process::exit(1);
                }

                let path_str = mod_path.to_string_lossy().to_string();

                if !loaded.contains(&path_str) {
                    loaded.insert(path_str.clone());
                    let mut imported_stmts =
                        load_and_parse(&path_str, false, loaded, file_id_counter, sources);
                    
                    imported_stmts.retain(|s| {
                        match &s.kind {
                            parser::StmtKind::Fn { name, .. } | parser::StmtKind::Class { name, .. } => {
                                names.contains(name)
                            }
                            _ => false,
                        }
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

fn compile_and_run(filename: &str, run: bool, show_time: bool, emit_ast: bool, emit_mir: bool) {
    let mut loaded = HashSet::new();
    loaded.insert(filename.to_string());
    let mut file_id_counter = 0;
    let mut sources = HashMap::default();
    let combined_stmts = load_and_parse(filename, true, &mut loaded, &mut file_id_counter, &mut sources);
    let program = parser::Program {
        stmts: combined_stmts,
    };

    if emit_ast {
        println!("{:#?}", program);
    }

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

    println!("DEBUG: starting MIR build");
    let mut mir_builder = MirBuilder::new(&type_checker.expr_types, &type_checker.type_env[0]);
    mir_builder.build_program(&program);
    println!("DEBUG: MIR build finished, starting optimization");

    let opt_start = std::time::Instant::now();
    let optimizer = mir::Optimizer::new();
    optimizer.run(&mut mir_builder.functions);
    println!("DEBUG: optimization finished, starting borrow check");
    let opt_duration = opt_start.elapsed();
    
    // show __main__ mir after optimizations
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
                        mir::StatementKind::Assign(_, mir::Rvalue::Ref(_) | mir::Rvalue::MutRef(_))
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
    let mut codegen = CraneliftCodegen::new(&mir_builder.functions);
    codegen.generate();
    codegen.finalize();
    let cg_duration = cg_start.elapsed();

    if !run {
        println!("\x1b[1;32mFinished\x1b[0m build successfully.");
        if show_time {
            println!("\n\x1b[1;32m   Olive Build Report\x1b[0m");
            println!("\x1b[1;34m   ────────────────────────\x1b[0m");
            println!("   \x1b[1mOptimization:\x1b[0m  {:?}", opt_duration);
            println!("   \x1b[1mBorrow Check:\x1b[0m  {:?}", borrow_duration);
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
            println!("   \x1b[1mOptimization:\x1b[0m  {:?}", opt_duration);
            println!("   \x1b[1mBorrow Check:\x1b[0m  {:?}", borrow_duration);
            println!("   \x1b[1mCodegen (JIT):\x1b[0m {:?}", cg_duration);
            println!("   \x1b[1mExecution:\x1b[0m     {:?}", exec_duration);
            println!("\x1b[1;34m   ────────────────────────\x1b[0m");
            println!(
                "   \x1b[1mTotal Startup:\x1b[0m {:?}",
                opt_duration + borrow_duration + cg_duration
            );
            println!();
        }
    } else {
        println!("No `main` function found to execute.");
    }
}

fn compile_and_test(filename: &str, _show_time: bool) {
    let mut loaded = HashSet::new();
    loaded.insert(filename.to_string());
    let mut file_id_counter = 0;
    let mut sources = HashMap::default();
    let combined_stmts = load_and_parse(filename, true, &mut loaded, &mut file_id_counter, &mut sources);
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

    let mut mir_builder = MirBuilder::new(&type_checker.expr_types, &type_checker.type_env[0]);
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

    let mut codegen = CraneliftCodegen::new(&mir_builder.functions);
    codegen.generate();
    codegen.finalize();

    println!("\x1b[1;34mRunning tests...\x1b[0m\n");
    let mut passed = 0;
    let mut failed = 0;

    for stmt in &program.stmts {
        if let parser::StmtKind::Fn {
            name, decorators, ..
        } = &stmt.kind
        {
            if decorators.iter().any(|d| d == "test") {
                print!("test {} ... ", name);
                std::io::Write::flush(&mut std::io::stdout()).unwrap();

                if let Some(func_ptr) = codegen.get_function(name) {
                    let func: extern "C" fn() -> i64 = unsafe { std::mem::transmute(func_ptr) };

                    let start = std::time::Instant::now();
                    // Catching traps in JIT needs a signal handler, so we'll just run it.
                    // If it fails, the process might exit.
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

fn format_file(filename: &str) {
    let source = match fs::read_to_string(filename) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error reading {}: {}", filename, e);
            return;
        }
    };
    let mut lexer = Lexer::new(&source, 0);
    let tokens = match lexer.tokenise() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error formatting {}: {}", filename, e.message);
            return;
        }
    };

    let mut formatted = String::new();
    let mut indent_level = 0;
    let mut at_start_of_line = true;
    let mut last_kind = TokenKind::Eof;

    for tok in tokens {
        match tok.kind {
            TokenKind::Indent => {
                indent_level += 1;
                continue;
            }
            TokenKind::Dedent => {
                indent_level -= 1;
                continue;
            }
            TokenKind::Newline => {
                formatted.push('\n');
                at_start_of_line = true;
                last_kind = TokenKind::Newline;
                continue;
            }
            TokenKind::Eof => break,
            _ => {
                if at_start_of_line {
                    formatted.push_str(&"    ".repeat(indent_level));
                    at_start_of_line = false;
                } else {
                    match tok.kind {
                        TokenKind::LParen
                        | TokenKind::LBracket
                        | TokenKind::LBrace
                        | TokenKind::Colon
                        | TokenKind::Comma
                        | TokenKind::RParen
                        | TokenKind::RBracket
                        | TokenKind::RBrace
                        | TokenKind::Dot => {}
                        _ => {
                            if !matches!(
                                last_kind,
                                TokenKind::LParen
                                    | TokenKind::LBracket
                                    | TokenKind::LBrace
                                    | TokenKind::Dot
                                    | TokenKind::At
                            ) {
                                formatted.push(' ');
                            }
                        }
                    }
                }

                match tok.kind {
                    TokenKind::String => {
                        formatted.push('"');
                        formatted.push_str(&tok.value);
                        formatted.push('"');
                    }
                    TokenKind::FString => {
                        formatted.push('f');
                        formatted.push('"');
                        formatted.push_str(&tok.value);
                        formatted.push('"');
                    }
                    _ => formatted.push_str(&tok.value),
                }

                last_kind = tok.kind;
            }
        }
    }

    fs::write(filename, formatted).unwrap();
    println!("\x1b[1;32mFormatted\x1b[0m {}", filename);
}

fn walk_and_format(path: &Path) {
    if path.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            walk_and_format(&entry.path());
        }
    } else if path.extension().map_or(false, |ext| ext == "liv") {
        format_file(path.to_str().unwrap());
    }
}

fn main() {
    println!("DEBUG: main started");
    let cli = Cli::parse();

    match cli.command {
        Commands::New { name } => {
            let path = Path::new(&name);
            if path.exists() {
                eprintln!("error: directory `{}` already exists", name);
                process::exit(1);
            }

            fs::create_dir_all(path.join("src")).unwrap();

            let config = Config {
                package: Package {
                    name: name.clone(),
                    version: "0.1.0".to_string(),
                    entry: "src/main.liv".to_string(),
                },
            };

            let toml = toml::to_string(&config).unwrap();
            fs::write(path.join("pit.toml"), toml).unwrap();

            let main_liv = "fn main():\n    print(\"Hello from Olive!\")\n\nmain()\n";
            fs::write(path.join("src/main.liv"), main_liv).unwrap();

            let gitignore = ".env\n.env.*\n*.secret\n";
            fs::write(path.join(".gitignore"), gitignore).unwrap();

            println!(
                "\x1b[1;32mCreated\x1b[0m binary (application) `{}` package",
                name
            );
        }
        Commands::Build { time } => {
            let config_path = Path::new("pit.toml");
            if !config_path.exists() {
                eprintln!("error: could not find `pit.toml` in current directory");
                process::exit(1);
            }

            let config_str = fs::read_to_string(config_path).unwrap();
            let config: Config = toml::from_str(&config_str).unwrap();

            compile_and_run(&config.package.entry, false, time, false, false);
        }
        Commands::Run {
            file,
            time,
            emit_ast,
            emit_mir,
        } => {
            if let Some(f) = file {
                compile_and_run(&f, true, time, emit_ast, emit_mir);
            } else {
                let config_path = Path::new("pit.toml");
                if !config_path.exists() {
                    eprintln!("error: no file specified and no `pit.toml` found");
                    process::exit(1);
                }

                let config_str = fs::read_to_string(config_path).unwrap();
                let config: Config = toml::from_str(&config_str).unwrap();

                compile_and_run(&config.package.entry, true, time, emit_ast, emit_mir);
            }
        }
        Commands::Format { file } => {
            if let Some(f) = file {
                let path = Path::new(&f);
                if path.is_dir() {
                    walk_and_format(path);
                } else {
                    format_file(&f);
                }
            } else {
                let config_path = Path::new("pit.toml");
                if config_path.exists() {
                    walk_and_format(Path::new("."));
                } else {
                    eprintln!("error: no file specified and no `pit.toml` found");
                    process::exit(1);
                }
            }
        }
        Commands::Test { time } => {
            let config_path = Path::new("pit.toml");
            if !config_path.exists() {
                eprintln!("error: could not find `pit.toml` in current directory");
                process::exit(1);
            }

            let config_str = fs::read_to_string(config_path).unwrap();
            let config: Config = toml::from_str(&config_str).unwrap();

            compile_and_test(&config.package.entry, time);
        }
    }
}

fn mangle_statements(stmts: &mut [parser::Stmt], prefix: &str, names: &HashSet<String>) {
    for stmt in stmts {
        mangle_stmt(stmt, prefix, names);
    }
}

fn mangle_stmt(stmt: &mut parser::Stmt, prefix: &str, names: &HashSet<String>) {
    match &mut stmt.kind {
        parser::StmtKind::Fn { name, body, .. } => {
            if names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
            for s in body {
                mangle_stmt(s, prefix, names);
            }
        }
        parser::StmtKind::Class { name, body, .. } => {
            if names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
            for s in body {
                mangle_stmt(s, prefix, names);
            }
        }
        parser::StmtKind::If { then_body, elif_clauses, else_body, condition } => {
            mangle_expr(condition, prefix, names);
            for s in then_body { mangle_stmt(s, prefix, names); }
            for (cond, body) in elif_clauses {
                mangle_expr(cond, prefix, names);
                for s in body { mangle_stmt(s, prefix, names); }
            }
            if let Some(body) = else_body {
                for s in body { mangle_stmt(s, prefix, names); }
            }
        }
        parser::StmtKind::While { condition, body, else_body } => {
            mangle_expr(condition, prefix, names);
            for s in body { mangle_stmt(s, prefix, names); }
            if let Some(body) = else_body {
                for s in body { mangle_stmt(s, prefix, names); }
            }
        }
        parser::StmtKind::For { iter, body, else_body, .. } => {
            mangle_expr(iter, prefix, names);
            for s in body { mangle_stmt(s, prefix, names); }
            if let Some(body) = else_body {
                for s in body { mangle_stmt(s, prefix, names); }
            }
        }
        parser::StmtKind::Let { name, value, .. } | parser::StmtKind::Const { name, value, .. } => {
            if names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
            mangle_expr(value, prefix, names);
        }
        parser::StmtKind::Assign { target, value } | parser::StmtKind::AugAssign { target, value, .. } => {
            mangle_expr(target, prefix, names);
            mangle_expr(value, prefix, names);
        }
        parser::StmtKind::Return(Some(e)) | parser::StmtKind::Raise(Some(e)) | parser::StmtKind::ExprStmt(e) => {
            mangle_expr(e, prefix, names);
        }
        _ => {}
    }
}

fn mangle_expr(expr: &mut parser::Expr, prefix: &str, names: &HashSet<String>) {
    match &mut expr.kind {
        parser::ExprKind::Identifier(name) => {
            if names.contains(name) {
                *name = format!("{}::{}", prefix, name);
            }
        }
        parser::ExprKind::BinOp { left, right, .. } => {
            mangle_expr(left, prefix, names);
            mangle_expr(right, prefix, names);
        }
        parser::ExprKind::UnaryOp { operand, .. } => mangle_expr(operand, prefix, names),
        parser::ExprKind::Call { callee, args } => {
            mangle_expr(callee, prefix, names);
            for arg in args {
                match arg {
                    parser::CallArg::Positional(e) | parser::CallArg::Keyword(_, e) | parser::CallArg::Splat(e) | parser::CallArg::KwSplat(e) => {
                        mangle_expr(e, prefix, names);
                    }
                }
            }
        }
        parser::ExprKind::Index { obj, index } => {
            mangle_expr(obj, prefix, names);
            mangle_expr(index, prefix, names);
        }
        parser::ExprKind::Attr { obj, .. } => mangle_expr(obj, prefix, names),
        parser::ExprKind::List(elems) | parser::ExprKind::Tuple(elems) | parser::ExprKind::Set(elems) => {
            for e in elems { mangle_expr(e, prefix, names); }
        }
        parser::ExprKind::Dict(pairs) => {
            for (k, v) in pairs {
                mangle_expr(k, prefix, names);
                mangle_expr(v, prefix, names);
            }
        }
        _ => {}
    }
}
