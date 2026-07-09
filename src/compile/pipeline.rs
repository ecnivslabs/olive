use super::linker::collect_native_libs;
use super::loader::load_and_parse;
use crate::borrow_check::BorrowChecker;
use crate::mir::{self, MirBuilder, MirFunction, Rvalue, StatementKind};
use crate::parser::{self, ast::FfiFnSig, ast::FfiStructDef, ast::FfiVarDef};
use crate::semantic::{Resolver, TypeChecker};
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
    pub field_types: HashMap<(String, String), crate::semantic::types::Type>,
    pub enum_defs: HashMap<String, Vec<(String, Vec<crate::semantic::types::Type>)>>,
    pub vtables: HashMap<String, Vec<String>>,
    pub global_vars: Vec<String>,
    pub native_libs: Vec<NativeLib>,
    pub program: parser::Program,
    pub file_names: HashMap<usize, String>,
    pub timings: PipelineTimings,
}

/// The set of loaded file ids that belong to the project being compiled, as
/// opposed to the standard library or installed pods. Lints (warnings) fire only
/// for these, the way Rust lints the local crate but not its dependencies; type
/// errors are still reported everywhere. A file is first-party when it lives
/// under the entry file's directory. If the root cannot be resolved, every file
/// is treated as first-party so nothing the programmer wrote is silently skipped.
pub(super) fn first_party_files(entry: &str, sources: &super::errors::Sources) -> HashSet<usize> {
    let root = std::path::Path::new(entry)
        .parent()
        .and_then(|p| p.canonicalize().ok());
    let Some(root) = root else {
        return sources.keys().copied().collect();
    };
    sources
        .iter()
        .filter(|(_, (path, _))| {
            std::path::Path::new(path)
                .canonicalize()
                .map(|p| p.starts_with(&root))
                .unwrap_or(false)
        })
        .map(|(id, _)| *id)
        .collect()
}

/// Compiles with the full optimizing pipeline. Used by the in-process test
/// harness so every optimizer pass stays exercised.
#[cfg(test)]
pub fn run_pipeline(filename: &str) -> Result<PipelineOutput, ()> {
    run_pipeline_opt(filename, true, None, false)
}

/// Compiles `filename`, running the full optimizer when `release` is set and the
/// lean debug pipeline otherwise, so non-release builds stay fast. `hot_functions`
/// (from a PGO profile) biases the inliner toward proven-hot callees; `None` elsewhere.
pub fn run_pipeline_opt(
    filename: &str,
    release: bool,
    hot_functions: Option<std::collections::HashSet<String>>,
    explain_copies: bool,
) -> Result<PipelineOutput, ()> {
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
    let mut program = parser::Program {
        stmts: combined_stmts,
    };
    crate::semantic::desugar::desugar_trait_defaults(&mut program);
    crate::semantic::desugar::desugar_bare_variants(&mut program);
    let parse_duration = t0.elapsed();

    let first_party = first_party_files(filename, &sources);

    let resolve_start = std::time::Instant::now();
    let mut resolver = Resolver::new();
    resolver.resolve_program(&program);
    if !resolver.errors.is_empty() {
        for e in &resolver.errors {
            e.to_diagnostic().emit(&sources);
        }
        return Err(());
    }
    for w in &resolver.warnings {
        if first_party.contains(&w.span().file_id) {
            w.to_diagnostic().emit(&sources);
        }
    }
    let resolve_duration = resolve_start.elapsed();

    let typecheck_start = std::time::Instant::now();
    let mut type_checker = TypeChecker::new();
    type_checker.check_program(&program);
    for w in &type_checker.warnings {
        w.to_diagnostic().into_warning().emit(&sources);
    }
    if !type_checker.errors.is_empty() {
        for e in &type_checker.errors {
            e.to_diagnostic().emit(&sources);
        }
        return Err(());
    }
    let closure_errors = crate::semantic::closure_check::check_closures(&program);
    if !closure_errors.is_empty() {
        for e in &closure_errors {
            e.to_diagnostic().emit(&sources);
        }
        return Err(());
    }
    for lint in crate::semantic::lint::lint_program(&program) {
        if first_party.contains(&lint.primary_span().file_id) {
            lint.emit(&sources);
        }
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
    mir_builder.struct_field_types = type_checker.field_types.clone();

    mir_builder.build_program(&program);
    let mir_duration = mir_start.elapsed();

    if super::lints::check_const_index_bounds(&mir_builder.functions, &sources) {
        return Err(());
    }

    // Borrow checking runs on the builder's MIR, before optimization:
    // inlining and copy propagation rewrite locals in ways that read as
    // uninitialized uses to the checker but never existed in source.
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
                e.to_diagnostic()
                    .note(format!("in function `{}`", func.name))
                    .emit(&sources);
            }
        }
    }
    if borrow_failed {
        return Err(());
    }
    let borrow_duration = borrow_start.elapsed();

    let opt_start = std::time::Instant::now();
    let mut optimizer = match (release, hot_functions) {
        (true, Some(hot)) => mir::Optimizer::new_with_hot_functions(hot.into_iter().collect()),
        (true, None) => mir::Optimizer::new(),
        (false, _) => mir::Optimizer::minimal(),
    };
    optimizer.set_explain_copies(explain_copies);
    let (gencheck_errors, copy_sites) = optimizer.run(&mut mir_builder.functions);
    if explain_copies && !copy_sites.is_empty() {
        println!("\nexplain-copies:");
        for site in &copy_sites {
            let path = sources
                .get(&site.span.file_id)
                .map(|(p, _)| p.as_str())
                .unwrap_or("?");
            let reason = match site.reason {
                crate::mir::optimizations::ownership::CopyReason::EscapeBorrow => "escaped borrow",
                crate::mir::optimizations::ownership::CopyReason::InteriorReturn => {
                    "interior return"
                }
                crate::mir::optimizations::ownership::CopyReason::TaskBoundary => "task boundary",
                crate::mir::optimizations::ownership::CopyReason::SpawnCapture => "spawn capture",
            };
            println!(
                "  {}:{}:{} in `{}` - copy of {} ({})",
                path, site.span.line, site.span.col, site.function, site.copied_type, reason,
            );
        }
        println!();
    }
    let opt_duration = opt_start.elapsed();
    if !gencheck_errors.is_empty() {
        for d in &gencheck_errors {
            d.emit(&sources);
        }
        return Err(());
    }

    let native_libs = collect_native_libs(&program);

    Ok(PipelineOutput {
        functions: mir_builder.functions,
        struct_fields: mir_builder.struct_fields,
        field_types: type_checker.field_types.clone(),
        enum_defs: type_checker.enum_defs.clone(),
        vtables: mir_builder.vtables,
        global_vars: mir_builder.global_vars,
        native_libs,
        program,
        file_names: mir_builder.file_names,
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
    fn first_party_excludes_files_outside_project_root() {
        let base = std::env::temp_dir().join(format!("olive_fp_{}", std::process::id()));
        let proj = base.join("proj");
        let ext = base.join("ext");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(&ext).unwrap();
        let main = proj.join("main.liv");
        let module = proj.join("mod.liv");
        let stdlib = ext.join("math.liv");
        for p in [&main, &module, &stdlib] {
            std::fs::write(p, "pass\n").unwrap();
        }

        let mut sources = HashMap::default();
        sources.insert(0, (main.to_string_lossy().into_owned(), String::new()));
        sources.insert(1, (module.to_string_lossy().into_owned(), String::new()));
        sources.insert(2, (stdlib.to_string_lossy().into_owned(), String::new()));

        let fp = first_party_files(main.to_str().unwrap(), &sources);
        assert!(fp.contains(&0), "entry file is first-party");
        assert!(fp.contains(&1), "in-project module is first-party");
        assert!(
            !fp.contains(&2),
            "file outside the project root is excluded"
        );

        std::fs::remove_dir_all(&base).ok();
    }

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
            field_types: HashMap::default(),
            enum_defs: HashMap::default(),
            vtables: HashMap::default(),
            global_vars: vec![],
            native_libs: vec![],
            program: parser::Program { stmts: vec![] },
            file_names: HashMap::default(),
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
            field_types: HashMap::default(),
            enum_defs: HashMap::default(),
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
            file_names: HashMap::default(),
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
