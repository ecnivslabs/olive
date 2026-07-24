use super::errors::Diagnostic;
use crate::lexer::Lexer;
use crate::mangle::mangle_statements;
use crate::parser::{self, Parser};
use crate::span;
use crate::tooling::pods::find_pod_path;
use rustc_hash::FxHashMap as HashMap;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

std::thread_local! {
    static PROJECT_ROOT: std::cell::RefCell<PathBuf> = const { std::cell::RefCell::new(PathBuf::new()) };
    static POD_META: std::cell::RefCell<Option<PodMeta>> = const { std::cell::RefCell::new(None) };
    static SOURCE_OVERLAY: std::cell::RefCell<HashMap<String, String>> = std::cell::RefCell::new(HashMap::default());
}

fn overlay_key(path: &str) -> String {
    fs::canonicalize(path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
}

/// Registers in-memory text for `path` that `load_and_parse`/`load_and_parse_collecting`
/// read instead of the file on disk. For the language server: an editor's
/// unsaved buffer, so diagnostics reflect what's on screen rather than what
/// was last saved. Imports resolve normally through the filesystem; only a
/// path with an active overlay is redirected.
pub fn set_source_overlay(path: &str, content: String) {
    let key = overlay_key(path);
    SOURCE_OVERLAY.with(|o| o.borrow_mut().insert(key, content));
}

/// Removes an overlay so `path` reads from disk again (the editor closed the
/// buffer or it now matches the saved file).
pub fn clear_source_overlay(path: &str) {
    let key = overlay_key(path);
    SOURCE_OVERLAY.with(|o| {
        o.borrow_mut().remove(&key);
    });
}

fn read_source(filename: &str) -> std::io::Result<String> {
    let key = overlay_key(filename);
    if let Some(content) = SOURCE_OVERLAY.with(|o| o.borrow().get(&key).cloned()) {
        return Ok(content);
    }
    fs::read_to_string(filename)
}

pub struct PodMeta {
    pub name: String,
    pub version: String,
    pub author: String,
}

pub fn set_pod_meta(meta: PodMeta) {
    POD_META.with(|m| *m.borrow_mut() = Some(meta));
}

pub fn pod_name() -> Option<String> {
    POD_META.with(|m| m.borrow().as_ref().map(|meta| meta.name.clone()))
}

#[cfg(test)]
pub(crate) fn clear_pod_meta() {
    POD_META.with(|m| *m.borrow_mut() = None);
}

fn synthesize_meta_stmts(span: span::Span) -> Vec<parser::Stmt> {
    let (name, version, author) = POD_META.with(|m| {
        let borrow = m.borrow();
        match &*borrow {
            Some(meta) => (meta.name.clone(), meta.version.clone(), meta.author.clone()),
            None => (String::new(), String::new(), String::new()),
        }
    });
    vec![
        make_str_const("NAME", &name, span),
        make_str_const("VERSION", &version, span),
        make_str_const("AUTHOR", &author, span),
        make_str_const("PIT_VERSION", env!("CARGO_PKG_VERSION"), span),
    ]
}

fn make_str_const(name: &str, value: &str, span: span::Span) -> parser::Stmt {
    parser::Stmt::new(
        parser::StmtKind::Const {
            name: name.to_string(),
            name_span: span,
            type_ann: None,
            value: parser::Expr::new(parser::ExprKind::Str(value.to_string()), span),
        },
        span,
    )
}

fn lex_span(file_id: usize, line: usize, col: usize, start: usize, end: usize) -> span::Span {
    span::Span {
        file_id,
        line,
        col,
        start,
        end,
    }
}

/// Loads and parses `filename`, recursively pulling in its imports, exactly
/// like `load_and_parse` but returning the failing `Diagnostic` instead of
/// printing it to stderr. `load_and_parse` is a thin wrapper over this that
/// preserves the original print-and-swallow behavior for the compiler's own
/// pipeline; this entry point is for callers that render diagnostics
/// themselves (the language server).
pub fn load_and_parse_collecting(
    filename: &str,
    is_main: bool,
    loaded: &mut HashSet<String>,
    file_id_counter: &mut usize,
    sources: &mut HashMap<usize, (String, String)>,
) -> Result<Vec<parser::Stmt>, Box<Diagnostic>> {
    struct ResetRoot;
    impl Drop for ResetRoot {
        fn drop(&mut self) {
            PROJECT_ROOT.with(|r| r.borrow_mut().clear());
        }
    }
    let _reset = if is_main { Some(ResetRoot) } else { None };

    if is_main {
        let root = Path::new(filename)
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        PROJECT_ROOT.with(|r| *r.borrow_mut() = root);
    }
    let current_file_id = *file_id_counter;
    *file_id_counter += 1;

    let source = read_source(filename).map_err(|e| {
        Box::new(Diagnostic::error(
            "",
            format!("error reading {filename}: {e}"),
            span::Span::default(),
        ))
    })?;

    sources.insert(current_file_id, (filename.to_string(), source.clone()));

    let tokens = match Lexer::new(&source, current_file_id).tokenise() {
        Ok(t) => t,
        Err(e) => {
            return Err(Box::new(
                Diagnostic::error(
                    "E0100",
                    "invalid token",
                    lex_span(current_file_id, e.line, e.col, e.start, e.end),
                )
                .label(e.message),
            ));
        }
    };

    let program = match Parser::new(tokens).parse_program() {
        Ok(p) => p,
        Err(e) => {
            return Err(Box::new(
                Diagnostic::error(
                    "E0200",
                    "syntax error",
                    lex_span(current_file_id, e.line, e.col, e.start, e.end),
                )
                .label(e.message),
            ));
        }
    };

    if !is_main {
        for stmt in &program.stmts {
            match &stmt.kind {
                parser::StmtKind::Fn { .. }
                | parser::StmtKind::Struct { .. }
                | parser::StmtKind::Impl { .. }
                | parser::StmtKind::Trait { .. }
                | parser::StmtKind::Enum { .. }
                | parser::StmtKind::Let { .. }
                | parser::StmtKind::MultiLet { .. }
                | parser::StmtKind::Const { .. }
                | parser::StmtKind::MultiConst { .. }
                | parser::StmtKind::Import { .. }
                | parser::StmtKind::NativeImport { .. }
                | parser::StmtKind::FromImport { .. }
                | parser::StmtKind::PyImport { .. }
                | parser::StmtKind::Pass => {}
                _ => {
                    return Err(Box::new(
                        Diagnostic::error(
                            "E0301",
                            "executable statement at module top level",
                            stmt.span,
                        )
                        .label("not allowed in an imported module")
                        .note("imported modules may only declare items (fn, struct, impl, trait, enum, let, const, import)")
                        .help("move this statement into a function, or run the file directly instead of importing it"),
                    ));
                }
            }
        }
    }

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
            name_span: span::Span::default(),
            type_ann: None,
            value: parser::Expr::new(parser::ExprKind::Str(mod_name), span::Span::default()),
        },
        span::Span::default(),
    ));

    let parent_dir = Path::new(filename).parent().unwrap_or(Path::new("."));

    for stmt in program.stmts {
        match &stmt.kind {
            parser::StmtKind::Import { module, alias } => {
                if module.len() == 1 && module[0] == "meta" {
                    let mod_prefix = alias.as_deref().unwrap_or("meta");
                    let mut imported_stmts = synthesize_meta_stmts(stmt.span);
                    let defined_names: HashSet<String> = imported_stmts
                        .iter()
                        .filter_map(|s| {
                            if let parser::StmtKind::Const { name, .. } = &s.kind {
                                Some(name.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    mangle_statements(&mut imported_stmts, mod_prefix, &defined_names);
                    all_stmts.extend(imported_stmts);
                    all_stmts.push(stmt.clone());
                    continue;
                }

                let mod_name = module.join("/");
                let mut mod_path = parent_dir.join(format!("{}.liv", mod_name));

                if !mod_path.exists() {
                    mod_path = find_std_lib_src_dir().join(format!("{}.liv", mod_name));
                }

                if !mod_path.exists() {
                    let root_path = PROJECT_ROOT.with(|r| r.borrow().clone());
                    if !root_path.as_os_str().is_empty() {
                        mod_path = root_path.join(format!("{}.liv", mod_name));
                    }
                }

                if !mod_path.exists()
                    && let Some(pkg_path) = find_pod_path(&mod_name)
                {
                    mod_path = pkg_path;
                }

                if !mod_path.exists() {
                    if is_main && super::laws::is_laws_import(module, alias) {
                        all_stmts.push(super::laws::make_laws_stmt(stmt.span));
                        continue;
                    }
                    return Err(Box::new(
                        Diagnostic::error("E0300", format!("module `{mod_name}` not found"), stmt.span)
                            .label("imported here")
                            .note("searched the project directory, the standard library, and installed pods")
                            .help(format!("create `{mod_name}.liv` next to this file, or install the pod that provides it")),
                    ));
                }

                let path_str = mod_path.to_string_lossy().to_string();

                if !loaded.contains(&path_str) {
                    loaded.insert(path_str.clone());
                    let mut imported_stmts = load_and_parse_collecting(
                        &path_str,
                        false,
                        loaded,
                        file_id_counter,
                        sources,
                    )?;

                    let mod_prefix = alias
                        .as_deref()
                        .unwrap_or_else(|| module.last().unwrap().as_str());
                    // A nested `import` module's symbols keep their canonical
                    // module-qualified names: re-prefixing them per importer
                    // (`a::json::loads`) breaks the second file to import the
                    // same module, whose flattened copy is deduplicated away
                    // and whose references then point at a name only the
                    // first importer's chain defines. Names already holding a
                    // `::` and import-bound module names are therefore left
                    // out of the mangle set, so every importer resolves to
                    // the one canonical copy.
                    let mut defined_names = HashSet::new();
                    for s in &imported_stmts {
                        match &s.kind {
                            parser::StmtKind::Fn { name, .. }
                            | parser::StmtKind::Struct { name, .. }
                            | parser::StmtKind::Enum { name, .. }
                            | parser::StmtKind::Let { name, .. }
                            | parser::StmtKind::Const { name, .. } => {
                                if !name.contains("::") {
                                    defined_names.insert(name.clone());
                                }
                            }
                            parser::StmtKind::MultiLet { names, .. }
                            | parser::StmtKind::MultiConst { names, .. } => {
                                for name in names {
                                    if !name.contains("::") {
                                        defined_names.insert(name.clone());
                                    }
                                }
                            }
                            parser::StmtKind::Impl { type_name, .. } => {
                                let tn = type_name.to_string();
                                if !tn.contains("::") {
                                    defined_names.insert(tn);
                                }
                            }
                            parser::StmtKind::PyImport { alias, .. } => {
                                defined_names.insert(alias.clone());
                            }
                            parser::StmtKind::NativeImport { alias, .. } => {
                                defined_names.insert(alias.clone());
                            }
                            parser::StmtKind::FromImport { names, is_star, .. } => {
                                if *is_star {
                                } else {
                                    for (name, alias) in names {
                                        let bound = alias.as_deref().unwrap_or(name.as_str());
                                        defined_names.insert(bound.to_string());
                                    }
                                }
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
                                | parser::StmtKind::Enum { .. }
                                | parser::StmtKind::Let { .. }
                                | parser::StmtKind::MultiLet { .. }
                                | parser::StmtKind::Const { .. }
                                | parser::StmtKind::MultiConst { .. }
                                | parser::StmtKind::Import { .. }
                                | parser::StmtKind::PyImport { .. }
                                | parser::StmtKind::NativeImport { .. }
                                | parser::StmtKind::FromImport { .. }
                        )
                    });

                    all_stmts.extend(imported_stmts);
                }
                all_stmts.push(stmt.clone());
            }
            parser::StmtKind::NativeImport { .. } => {
                all_stmts.push(stmt.clone());
            }
            parser::StmtKind::PyImport { .. } => {
                all_stmts.push(stmt.clone());
            }
            parser::StmtKind::FromImport {
                module,
                names: _names,
                is_star: _is_star,
            } => {
                if module.len() == 1 && module[0] == "meta" {
                    let imported_stmts = synthesize_meta_stmts(stmt.span);
                    all_stmts.extend(imported_stmts);
                    all_stmts.push(stmt.clone());
                    continue;
                }

                let mod_name = module.join("/");
                let mut mod_path = parent_dir.join(format!("{}.liv", mod_name));

                if !mod_path.exists() {
                    mod_path = find_std_lib_src_dir().join(format!("{}.liv", mod_name));
                }

                if !mod_path.exists() {
                    let root_path = PROJECT_ROOT.with(|r| r.borrow().clone());
                    if !root_path.as_os_str().is_empty() {
                        mod_path = root_path.join(format!("{}.liv", mod_name));
                    }
                }

                if !mod_path.exists()
                    && let Some(pkg_path) = find_pod_path(&mod_name)
                {
                    mod_path = pkg_path;
                }

                if !mod_path.exists() {
                    return Err(Box::new(
                        Diagnostic::error("E0300", format!("module `{mod_name}` not found"), stmt.span)
                            .label("imported here")
                            .note("searched the project directory, the standard library, and installed pods")
                            .help(format!("create `{mod_name}.liv` next to this file, or install the pod that provides it")),
                    ));
                }

                let path_str = mod_path.to_string_lossy().to_string();

                if !loaded.contains(&path_str) {
                    loaded.insert(path_str.clone());
                    let imported_stmts = load_and_parse_collecting(
                        &path_str,
                        false,
                        loaded,
                        file_id_counter,
                        sources,
                    )?;

                    all_stmts.extend(imported_stmts);
                }
                all_stmts.push(stmt.clone());
            }
            _ => all_stmts.push(stmt),
        }
    }

    Ok(all_stmts)
}

/// Loads and parses `filename`, recursively pulling in its imports. Prints
/// the first diagnostic hit to stderr and returns `Err(())`; see
/// `load_and_parse_collecting` for a variant that hands the diagnostic back
/// instead of printing it.
pub fn load_and_parse(
    filename: &str,
    is_main: bool,
    loaded: &mut HashSet<String>,
    file_id_counter: &mut usize,
    sources: &mut HashMap<usize, (String, String)>,
) -> Result<Vec<parser::Stmt>, ()> {
    load_and_parse_collecting(filename, is_main, loaded, file_id_counter, sources)
        .map_err(|diag| diag.emit(sources))
}

pub fn collect_source_files(
    filename: &str,
    collected: &mut Vec<String>,
    py_files: &mut Vec<String>,
    visited: &mut HashSet<String>,
) {
    // Set project root on first call so deep modules hash correctly and cache invalidates properly.
    if visited.is_empty() {
        let root = Path::new(filename)
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        PROJECT_ROOT.with(|r| *r.borrow_mut() = root);
    }
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
        match &stmt.kind {
            parser::StmtKind::Import { module, .. }
            | parser::StmtKind::FromImport { module, .. } => {
                let mod_name = module.join("/");
                let mut mod_path = parent_dir.join(format!("{}.liv", mod_name));
                if !mod_path.exists() {
                    mod_path = find_std_lib_src_dir().join(format!("{}.liv", mod_name));
                }
                if !mod_path.exists() {
                    let root_path = PROJECT_ROOT.with(|r| r.borrow().clone());
                    if !root_path.as_os_str().is_empty() {
                        mod_path = root_path.join(format!("{}.liv", mod_name));
                    }
                }
                if !mod_path.exists()
                    && let Some(pkg_path) = find_pod_path(&mod_name)
                {
                    mod_path = pkg_path;
                }
                if mod_path.exists() {
                    collect_source_files(
                        mod_path.to_string_lossy().as_ref(),
                        collected,
                        py_files,
                        visited,
                    );
                }
            }
            parser::StmtKind::PyImport { module, .. } => {
                let py_name = format!("{}.py", module);
                if !visited.contains(&py_name) {
                    visited.insert(py_name.clone());
                    if let Ok(canonical) = fs::canonicalize(&py_name) {
                        py_files.push(canonical.to_string_lossy().to_string());
                    }
                }
            }
            _ => {}
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
            if let Some(grandparent) = parent.parent() {
                let dev_lib = grandparent.join("lib");
                if dev_lib.exists() {
                    return dev_lib;
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_valid_module_imports_declarations_only() {
        let temp_dir = std::env::temp_dir().join("olive_test_valid");
        fs::create_dir_all(&temp_dir).unwrap();

        let mod_path = temp_dir.join("my_module.liv");
        fs::write(&mod_path, "fn add(a: int, b: int) -> int:\n    return a + b\n\nstruct Point:\n    x: int\n    y: int\n").unwrap();

        let mut loaded = HashSet::new();
        let mut file_id_counter = 0;
        let mut sources = HashMap::default();

        let stmts = load_and_parse(
            &mod_path.to_string_lossy(),
            false,
            &mut loaded,
            &mut file_id_counter,
            &mut sources,
        )
        .unwrap();
        assert!(!stmts.is_empty());

        fs::remove_dir_all(&temp_dir).ok();
    }
}
