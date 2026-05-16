use super::errors::report_error;
use crate::lexer::Lexer;
use crate::mangle::mangle_statements;
use crate::parser::{self, Parser};
use crate::pods::find_pod_path;
use crate::span;
use rustc_hash::FxHashMap as HashMap;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process,
};

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
                    mod_path = find_std_lib_src_dir().join(format!("{}.liv", mod_name));
                }

                if !mod_path.exists()
                    && let Some(pkg_path) = find_pod_path(&mod_name)
                {
                    mod_path = pkg_path;
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
            parser::StmtKind::NativeImport { .. } => {
                all_stmts.push(stmt.clone());
            }
            parser::StmtKind::FromImport { module, names } => {
                let mod_name = module.join("/");
                let mut mod_path = parent_dir.join(format!("{}.liv", mod_name));

                if !mod_path.exists() {
                    mod_path = find_std_lib_src_dir().join(format!("{}.liv", mod_name));
                }

                if !mod_path.exists()
                    && let Some(pkg_path) = find_pod_path(&mod_name)
                {
                    mod_path = pkg_path;
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

pub fn collect_source_files(
    filename: &str,
    collected: &mut Vec<String>,
    visited: &mut HashSet<String>,
) {
    let canonical = fs::canonicalize(filename)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| filename.to_string());
    if !visited.insert(canonical.clone()) {
        return;
    }
    collected.push(canonical.clone());
    let source = match fs::read_to_string(filename) {
        Ok(s) => s,
        Err(_) => return,
    };
    let tokens = match crate::lexer::Lexer::new(&source, 0).tokenise() {
        Ok(t) => t,
        Err(_) => return,
    };
    let program = match crate::parser::Parser::new(tokens).parse_program() {
        Ok(p) => p,
        Err(_) => return,
    };
    let parent_dir = Path::new(filename)
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    for stmt in &program.stmts {
        if let parser::StmtKind::Import { module, .. }
        | parser::StmtKind::FromImport { module, .. } = &stmt.kind
        {
            let mod_name = module.join("/");
            let mut mod_path = parent_dir.join(format!("{}.liv", mod_name));
            if !mod_path.exists() {
                mod_path = find_std_lib_src_dir().join(format!("{}.liv", mod_name));
            }
            if !mod_path.exists()
                && let Some(pkg_path) = find_pod_path(&mod_name)
            {
                mod_path = pkg_path;
            }
            if mod_path.exists() {
                collect_source_files(mod_path.to_string_lossy().as_ref(), collected, visited);
            }
        }
    }
}

pub fn find_std_lib_src_dir() -> PathBuf {
    if Path::new("lib").exists() {
        return PathBuf::from("lib");
    }
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        let lib_dir = exe_dir.join("lib");
        if lib_dir.exists() {
            return lib_dir;
        }
        if let Some(parent) = exe_dir.parent() {
            let std_lib = parent.join("lib").join("olive");
            if std_lib.exists() {
                return std_lib;
            }
        }
    }
    for dir in &["/usr/local/lib/olive", "/usr/lib/olive", "/lib/olive"] {
        let path = Path::new(dir);
        if path.exists() {
            return path.to_path_buf();
        }
    }
    PathBuf::from("lib")
}
