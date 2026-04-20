use super::errors::report_error;
use super::linker::collect_native_libs;
use super::loader::load_and_parse;
use crate::borrow_check::BorrowChecker;
use crate::mir::{self, MirBuilder, MirFunction, Rvalue, StatementKind};
use crate::parser::{self, ast::FfiFnSig, ast::FfiStructDef, ast::FfiVarDef};
use crate::semantic::{self, Resolver, TypeChecker};
use rustc_hash::FxHashMap as HashMap;
use std::{collections::HashSet, time::Duration};

pub type NativeLib = (
    String,
    String,
    Vec<FfiFnSig>,
    Vec<FfiStructDef>,
    Vec<FfiVarDef>,
);

pub struct PipelineTimings {
    pub parse: Duration,
    pub resolve: Duration,
    pub typecheck: Duration,
    pub mir: Duration,
    pub optimize: Duration,
    pub borrow_check: Duration,
}

pub struct PipelineOutput {
    pub functions: Vec<MirFunction>,
    pub struct_fields: HashMap<String, Vec<String>>,
    pub vtables: HashMap<String, Vec<String>>,
    pub global_vars: Vec<String>,
    pub native_libs: Vec<NativeLib>,
    pub program: parser::Program,
    pub timings: PipelineTimings,
}

pub fn run_pipeline(filename: &str) -> Result<PipelineOutput, ()> {
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
    println!("AST length: {}", combined_stmts.len());
    for s in &combined_stmts {
        println!("AST item: {:?}", s.kind);
    }
    let program = parser::Program {
        stmts: combined_stmts,
    };
    let parse_duration = t0.elapsed();

    let resolve_start = std::time::Instant::now();
    let mut resolver = Resolver::new();
    resolver.resolve_program(&program);
    if !resolver.errors.is_empty() {
        for e in &resolver.errors {
            report_error(&sources, &format!("{}", e), e.span());
        }
        return Err(());
    }
    let resolve_duration = resolve_start.elapsed();

    let typecheck_start = std::time::Instant::now();
    let mut type_checker = TypeChecker::new();
    type_checker.check_program(&program);
    if !type_checker.errors.is_empty() {
        for e in &type_checker.errors {
            report_error(&sources, &format!("{}", e), e.span());
        }
        return Err(());
    }
    let typecheck_duration = typecheck_start.elapsed();

    let mir_start = std::time::Instant::now();
    let mut mir_builder = MirBuilder::new(
        &type_checker.expr_types,
        &type_checker.expr_kwarg_maps,
        &type_checker.type_env[0],
        type_checker.struct_fields.clone(),
        &type_checker.traits,
        type_checker.c_ffi_fns.clone(),
    );

    mir_builder.build_program(&program);
    let mir_duration = mir_start.elapsed();

    let opt_start = std::time::Instant::now();
    let optimizer = mir::Optimizer::new();
    optimizer.run(&mut mir_builder.functions);
    let opt_duration = opt_start.elapsed();

    let borrow_start = std::time::Instant::now();
    let mut borrow_failed = false;
    for func in &mir_builder.functions {
        let is_init = func.name.ends_with("::__init__");
        let needs_check = is_init
            || func.locals.iter().any(|l| l.ty.is_move_type())
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
        let mut checker = BorrowChecker::new(func, &type_checker.struct_fields);
        checker.check();
        if !checker.errors.is_empty() {
            borrow_failed = true;
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
        }
    }
    if borrow_failed {
        return Err(());
    }
    let borrow_duration = borrow_start.elapsed();

    let native_libs = collect_native_libs(&program);

    Ok(PipelineOutput {
        functions: mir_builder.functions,
        struct_fields: mir_builder.struct_fields,
        vtables: mir_builder.vtables,
        global_vars: mir_builder.global_vars,
        native_libs,
        program,
        timings: PipelineTimings {
            parse: parse_duration,
            resolve: resolve_duration,
            typecheck: typecheck_duration,
            mir: mir_duration,
            optimize: opt_duration,
            borrow_check: borrow_duration,
        },
    })
}
