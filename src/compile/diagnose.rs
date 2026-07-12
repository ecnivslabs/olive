//! Read-only front half of the pipeline (lex..lint, no MIR/codegen) for the
//! language server; mirrors `pipeline::run_pipeline_opt`'s stage ordering.

use super::errors::Diagnostic;
use super::loader;
use super::pipeline::first_party_files;
use crate::parser;
use crate::semantic::types::Type;
use crate::semantic::{Resolver, TypeChecker};
use crate::span::Span;
use rustc_hash::FxHashMap as HashMap;
use std::collections::HashSet;

/// Populated as far as analysis got; a typecheck error still leaves
/// `def_sites` intact so hover/go-to-definition keep working.
pub struct DiagnoseOutput {
    pub diagnostics: Vec<Diagnostic>,
    pub sources: super::errors::Sources,
    pub program: Option<parser::Program>,
    pub expr_types: HashMap<usize, Type>,
    pub def_sites: HashMap<usize, Span>,
    pub struct_fields: HashMap<String, Vec<String>>,
}

impl DiagnoseOutput {
    fn empty(sources: super::errors::Sources) -> Self {
        Self {
            diagnostics: Vec::new(),
            sources,
            program: None,
            expr_types: HashMap::default(),
            def_sites: HashMap::default(),
            struct_fields: HashMap::default(),
        }
    }
}

/// `filename` reads from the source overlay if set; imports always read disk.
pub fn diagnose(filename: &str) -> DiagnoseOutput {
    let mut loaded = HashSet::new();
    loaded.insert(filename.to_string());
    let mut file_id_counter = 0;
    let mut sources = HashMap::default();

    let combined_stmts = match loader::load_and_parse_collecting(
        filename,
        true,
        &mut loaded,
        &mut file_id_counter,
        &mut sources,
    ) {
        Ok(stmts) => stmts,
        Err(diag) => {
            let mut out = DiagnoseOutput::empty(sources);
            out.diagnostics.push(*diag);
            return out;
        }
    };

    let mut program = parser::Program {
        stmts: combined_stmts,
    };
    crate::semantic::desugar::desugar_trait_defaults(&mut program);
    crate::semantic::desugar::desugar_bare_variants(&mut program);

    let first_party = first_party_files(filename, &sources);
    let mut diagnostics = Vec::new();

    let mut resolver = Resolver::new();
    resolver.resolve_program(&program);
    if !resolver.errors.is_empty() {
        diagnostics.extend(resolver.errors.iter().map(|e| e.to_diagnostic()));
        return DiagnoseOutput {
            diagnostics,
            sources,
            program: Some(program),
            expr_types: HashMap::default(),
            def_sites: resolver.def_sites,
            struct_fields: HashMap::default(),
        };
    }
    diagnostics.extend(
        resolver
            .warnings
            .iter()
            .filter(|w| first_party.contains(&w.span().file_id))
            .map(|w| w.to_diagnostic().into_warning()),
    );

    let mut type_checker = TypeChecker::new();
    type_checker.check_program(&program);
    diagnostics.extend(
        type_checker
            .warnings
            .iter()
            .map(|w| w.to_diagnostic().into_warning()),
    );
    if !type_checker.errors.is_empty() {
        diagnostics.extend(type_checker.errors.iter().map(|e| e.to_diagnostic()));
        return DiagnoseOutput {
            diagnostics,
            sources,
            program: Some(program),
            expr_types: type_checker.expr_types,
            def_sites: resolver.def_sites,
            struct_fields: type_checker.struct_fields,
        };
    }

    let closure_errors = crate::semantic::closure_check::check_closures(&program);
    if !closure_errors.is_empty() {
        diagnostics.extend(closure_errors.iter().map(|e| e.to_diagnostic()));
        return DiagnoseOutput {
            diagnostics,
            sources,
            program: Some(program),
            expr_types: type_checker.expr_types,
            def_sites: resolver.def_sites,
            struct_fields: type_checker.struct_fields,
        };
    }

    diagnostics.extend(
        crate::semantic::lint::lint_program(&program)
            .into_iter()
            .filter(|lint| first_party.contains(&lint.primary_span().file_id)),
    );

    DiagnoseOutput {
        diagnostics,
        sources,
        program: Some(program),
        expr_types: type_checker.expr_types,
        def_sites: resolver.def_sites,
        struct_fields: type_checker.struct_fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("olive_diagnose_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn clean_program_has_no_diagnostics() {
        let path = write_temp("clean.liv", "let x = 1\nprint(x)\n");
        let out = diagnose(path.to_str().unwrap());
        assert!(out.diagnostics.is_empty());
        assert!(out.program.is_some());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn undefined_name_is_reported() {
        let path = write_temp("undef.liv", "print(nope)\n");
        let out = diagnose(path.to_str().unwrap());
        assert!(!out.diagnostics.is_empty());
        assert_eq!(out.diagnostics[0].code(), Some("E0001"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn type_error_still_returns_expr_types() {
        let path = write_temp("typeerr.liv", "let x: int = \"s\"\nlet y = 1 + 2\n");
        let out = diagnose(path.to_str().unwrap());
        assert!(!out.diagnostics.is_empty());
        assert!(!out.expr_types.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn overlay_text_wins_over_disk() {
        let path = write_temp("overlay.liv", "let x = 1\n");
        loader::set_source_overlay(path.to_str().unwrap(), "print(nope)\n".to_string());
        let out = diagnose(path.to_str().unwrap());
        assert!(!out.diagnostics.is_empty());
        loader::clear_source_overlay(path.to_str().unwrap());
        let out2 = diagnose(path.to_str().unwrap());
        assert!(out2.diagnostics.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn syntax_error_is_reported() {
        let path = write_temp("syn.liv", "let x = \n");
        let out = diagnose(path.to_str().unwrap());
        assert!(!out.diagnostics.is_empty());
        assert_eq!(out.diagnostics[0].code(), Some("E0200"));
        std::fs::remove_file(&path).ok();
    }
}
