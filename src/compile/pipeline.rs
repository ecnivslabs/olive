use super::errors::{report_error, report_warning};
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
    )?;
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
    for w in &type_checker.warnings {
        report_warning(&sources, &format!("{}", w), w.span());
    }
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
    mir_builder.file_names = sources
        .iter()
        .map(|(id, (name, _))| (*id, name.clone()))
        .collect();

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use rustc_hash::FxHashMap as HashMap;
    use std::time::Duration;

    #[test]
    fn pipeline_timings_construction() {
        let t = PipelineTimings {
            parse: Duration::from_secs(1),
            resolve: Duration::from_secs(2),
            typecheck: Duration::from_secs(3),
            mir: Duration::from_secs(4),
            optimize: Duration::from_secs(5),
            borrow_check: Duration::from_secs(6),
        };
        assert_eq!(t.parse.as_secs(), 1);
        assert_eq!(t.resolve.as_secs(), 2);
        assert_eq!(t.typecheck.as_secs(), 3);
        assert_eq!(t.mir.as_secs(), 4);
        assert_eq!(t.optimize.as_secs(), 5);
        assert_eq!(t.borrow_check.as_secs(), 6);
    }

    #[test]
    fn native_lib_type_alias() {
        let lib: NativeLib = (
            "mylib".to_string(),
            "/path/to/lib".to_string(),
            vec![],
            vec![],
            vec![],
        );
        assert_eq!(lib.0, "mylib");
        assert_eq!(lib.1, "/path/to/lib");
        assert!(lib.2.is_empty());
        assert!(lib.3.is_empty());
        assert!(lib.4.is_empty());
    }

    #[test]
    fn pipeline_output_empty() {
        let output = PipelineOutput {
            functions: vec![],
            struct_fields: HashMap::default(),
            vtables: HashMap::default(),
            global_vars: vec![],
            native_libs: vec![],
            program: parser::Program { stmts: vec![] },
            timings: PipelineTimings {
                parse: Duration::ZERO,
                resolve: Duration::ZERO,
                typecheck: Duration::ZERO,
                mir: Duration::ZERO,
                optimize: Duration::ZERO,
                borrow_check: Duration::ZERO,
            },
        };
        assert!(output.functions.is_empty());
        assert!(output.struct_fields.is_empty());
        assert!(output.vtables.is_empty());
        assert!(output.global_vars.is_empty());
        assert!(output.native_libs.is_empty());
        assert!(output.program.stmts.is_empty());
        assert_eq!(output.timings.parse.as_nanos(), 0);
    }

    #[test]
    fn pipeline_output_with_data() {
        let mut struct_fields = HashMap::default();
        struct_fields.insert("Point".to_string(), vec!["x".to_string(), "y".to_string()]);
        let mut vtables = HashMap::default();
        vtables.insert("Draw".to_string(), vec!["render".to_string()]);
        let output = PipelineOutput {
            functions: vec![],
            struct_fields,
            vtables,
            global_vars: vec!["GLOBAL".to_string()],
            native_libs: vec![(
                "sdl".to_string(),
                "libSDL2.so".to_string(),
                vec![],
                vec![],
                vec![],
            )],
            program: parser::Program {
                stmts: vec![parser::Stmt {
                    kind: parser::StmtKind::Pass,
                    span: crate::span::Span {
                        file_id: 0,
                        line: 0,
                        col: 0,
                        start: 0,
                        end: 0,
                    },
                }],
            },
            timings: PipelineTimings {
                parse: Duration::from_millis(10),
                resolve: Duration::from_millis(20),
                typecheck: Duration::from_millis(30),
                mir: Duration::from_millis(40),
                optimize: Duration::from_millis(50),
                borrow_check: Duration::from_millis(60),
            },
        };
        assert_eq!(output.struct_fields.get("Point").unwrap().len(), 2);
        assert_eq!(output.vtables.get("Draw").unwrap()[0], "render");
        assert_eq!(output.global_vars[0], "GLOBAL");
        assert_eq!(output.native_libs.len(), 1);
        assert_eq!(output.native_libs[0].0, "sdl");
        assert_eq!(output.program.stmts.len(), 1);
        assert_eq!(output.timings.resolve.as_millis(), 20);
    }
}
