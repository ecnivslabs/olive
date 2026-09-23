use super::errors::Diagnostic;
use crate::lexer::Lexer;
use crate::mangle::mangle_statements;
use crate::parser::{self, Parser};
use crate::span;
use crate::tooling::pods::find_pod_path;
use crate::tooling::{manifest, target};
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
    static POD_NATIVE_CACHE: std::cell::RefCell<HashMap<PathBuf, Option<(PathBuf, manifest::Native)>>> =
        std::cell::RefCell::new(HashMap::default());
    static POD_NATIVE_RESOLVED: std::cell::RefCell<HashSet<String>> = std::cell::RefCell::new(HashSet::default());
}

/// True if `path` (already-resolved, absolute) was produced by
/// [`resolve_pod_native`] rather than being a system library or an explicit
/// path-ref import. The linker uses this to decide whether to stage and
/// `$ORIGIN`-rpath the library (pod-relative, relocatable) rather than
/// treating it as a fixed absolute path on this machine.
pub fn is_pod_native_lib(path: &str) -> bool {
    POD_NATIVE_RESOLVED.with(|s| s.borrow().contains(path))
}

/// Strips a `lib` prefix and a shared-library suffix (`.so[.N...]`,
/// `.dylib`, `.dll`) from a native import spec to recover the stem a pod's
/// `[native].lib` would name it by, e.g. `libtokenizer.so` -> `tokenizer`.
/// A spec with no recognizable prefix/suffix (a bare stem already) is
/// returned unchanged.
fn native_lib_stem(spec: &str) -> &str {
    let base = spec.strip_prefix("lib").unwrap_or(spec);
    for ext in [".so", ".dylib", ".dll"] {
        if let Some(idx) = base.find(ext) {
            return &base[..idx];
        }
    }
    base
}

/// Walks up from `file_dir` to the first `pit.toml`, returning its directory
/// and `[native]` table if that pod declares one. Stops at the first
/// manifest found regardless of whether it declares `[native]`, since a
/// project's imports are never meant to resolve against some unrelated
/// ancestor pod. Memoized per directory for the life of the process: a
/// pod's `pit.toml` cannot change mid-compile.
fn find_pod_native(file_dir: &Path) -> Option<(PathBuf, manifest::Native)> {
    let canon_dir = file_dir.canonicalize().ok()?;
    if let Some(cached) = POD_NATIVE_CACHE.with(|c| c.borrow().get(&canon_dir).cloned()) {
        return cached;
    }

    let mut dir = canon_dir.clone();
    let mut result = None;
    for _ in 0..16 {
        let pit_toml = dir.join("pit.toml");
        if pit_toml.is_file() {
            if let Ok(content) = fs::read_to_string(&pit_toml)
                && let Ok(config) = toml::from_str::<manifest::Config>(&content)
                && let Some(native) = config.native
                && crate::tooling::manifest::validate_native_layout(&native).is_ok()
            {
                result = Some((dir.clone(), native));
            }
            break;
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => break,
        }
    }

    POD_NATIVE_CACHE.with(|c| c.borrow_mut().insert(canon_dir, result.clone()));
    result
}

/// Resolves a bare native import spec to the owning pod's installed artifact,
/// if one exists: `import "libtokenizer.so"` inside a pod declaring
/// `[native] lib = "tokenizer"` resolves to `<pod root>/native/libtokenizer.so`
/// (extension chosen for the host, so the same source line is correct on
/// every platform). Returns `None` for anything else - a system library like
/// `libc.so.6`, a pod with no `[native]` table, or a stem mismatch - and the
/// caller falls back to the existing system-wide search.
fn resolve_pod_native(file_dir: &Path, spec: &str) -> Option<String> {
    let stem = native_lib_stem(spec);
    let (root, native) = find_pod_native(file_dir)?;
    if native.lib != stem {
        return None;
    }
    let local_name = target::local_name(&native.lib)?;
    let artifact = root.join("native").join(local_name);
    if !artifact.is_file() {
        return None;
    }
    let canon = artifact.canonicalize().ok()?;
    let native_root = root.join("native").canonicalize().ok()?;
    if !canon.starts_with(&native_root) {
        return None;
    }
    Some(canon.to_string_lossy().to_string())
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
        return Ok(crate::lexer::normalize_newlines(&content));
    }
    fs::read_to_string(filename).map(|s| crate::lexer::normalize_newlines(&s))
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

#[derive(Debug, PartialEq, Eq)]
pub enum ResolvedModule {
    File(PathBuf),
    ModFile(PathBuf),
    Directory(PathBuf, Vec<PathBuf>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ModuleResolutionError {
    NotFound(String),
    Ambiguous {
        module: String,
        file_path: PathBuf,
        mod_path: PathBuf,
    },
}

pub fn resolve_module_target(
    base_dir: &Path,
    module: &[String],
) -> Result<ResolvedModule, ModuleResolutionError> {
    let mod_rel = module.join("/");
    let root_path = PROJECT_ROOT.with(|r| r.borrow().clone());
    let stdlib_dir = find_std_lib_src_dir();

    let mut search_dirs: Vec<&Path> = vec![base_dir];
    if stdlib_dir != base_dir {
        search_dirs.push(&stdlib_dir);
    }
    if !root_path.as_os_str().is_empty() && root_path != base_dir && root_path != stdlib_dir {
        search_dirs.push(&root_path);
    }

    for dir in search_dirs {
        let file_cand = dir.join(format!("{mod_rel}.liv"));
        let mod_cand = dir.join(&mod_rel).join("mod.liv");
        let dir_cand = dir.join(&mod_rel);

        let file_exists = file_cand.is_file();
        let mod_exists = mod_cand.is_file();

        if file_exists && mod_exists {
            return Err(ModuleResolutionError::Ambiguous {
                module: mod_rel,
                file_path: file_cand,
                mod_path: mod_cand,
            });
        }
        if file_exists {
            return Ok(ResolvedModule::File(file_cand));
        }
        if mod_exists {
            return Ok(ResolvedModule::ModFile(mod_cand));
        }
        if dir_cand.is_dir() {
            let mut submodules = Vec::new();
            if let Ok(entries) = fs::read_dir(&dir_cand) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() && p.extension().is_some_and(|ext| ext == "liv") {
                        submodules.push(p);
                    }
                }
            }
            if !submodules.is_empty() {
                submodules.sort();
                return Ok(ResolvedModule::Directory(dir_cand, submodules));
            }
        }
    }

    if let Some(pkg_path) = find_pod_path(&mod_rel) {
        return Ok(ResolvedModule::File(pkg_path));
    }

    Err(ModuleResolutionError::NotFound(mod_rel))
}

fn resolve_python_module(base_dir: &Path, module: &str) -> Option<PathBuf> {
    let spec = module.strip_suffix(".py").unwrap_or(module);
    let parts: Vec<&str> = spec.split('.').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }

    let mut roots = Vec::<PathBuf>::new();
    let mut add_root = |path: PathBuf| {
        let path = fs::canonicalize(&path).unwrap_or(path);
        if !roots.contains(&path) {
            roots.push(path);
        }
    };
    add_root(base_dir.to_path_buf());
    let project_root = PROJECT_ROOT.with(|root| root.borrow().clone());
    if !project_root.as_os_str().is_empty() {
        add_root(project_root);
    }
    if let Ok(cwd) = std::env::current_dir() {
        add_root(cwd);
    }
    if let Some(paths) = std::env::var_os("PYTHONPATH") {
        for path in std::env::split_paths(&paths) {
            if !path.as_os_str().is_empty() {
                add_root(path);
            }
        }
    }

    for root in roots {
        let mut module_path = root;
        for part in &parts {
            module_path.push(part);
        }
        let file = module_path.with_extension("py");
        if file.is_file() {
            return Some(file);
        }
        let package = module_path.join("__init__.py");
        if package.is_file() {
            return Some(package);
        }
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
struct PythonImport {
    module: String,
    level: usize,
}

fn python_imports(source: &str) -> Vec<PythonImport> {
    let mut imports = Vec::new();
    for raw_line in source.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("import ") {
            for spec in rest.split(',') {
                let module = spec.split_whitespace().next().unwrap_or_default();
                if !module.is_empty() {
                    imports.push(PythonImport {
                        module: module.to_string(),
                        level: 0,
                    });
                }
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("from ") else {
            continue;
        };
        let Some((module_spec, imported)) = rest.split_once(" import ") else {
            continue;
        };
        let module_spec = module_spec.trim();
        let level = module_spec.chars().take_while(|c| *c == '.').count();
        let module = module_spec[level..].trim().to_string();
        if module.is_empty() {
            for name in imported.split(',') {
                let name = name.split_whitespace().next().unwrap_or_default();
                if !name.is_empty() && name != "*" {
                    imports.push(PythonImport {
                        module: name.to_string(),
                        level,
                    });
                }
            }
        } else {
            imports.push(PythonImport { module, level });
        }
    }
    imports
}

fn resolve_python_import(source_path: &Path, import: &PythonImport) -> Option<PathBuf> {
    if import.level == 0 {
        return resolve_python_module(
            source_path.parent().unwrap_or(Path::new(".")),
            &import.module,
        );
    }
    let mut base = source_path.parent()?.to_path_buf();
    for _ in 1..import.level {
        base = base.parent()?.to_path_buf();
    }
    resolve_python_module(&base, &import.module)
}

fn collect_python_file(path: &Path, py_files: &mut Vec<String>, visited: &mut HashSet<String>) {
    let canonical = fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned());
    if !visited.insert(canonical.clone()) {
        return;
    }
    py_files.push(canonical.clone());
    let Ok(source) = fs::read_to_string(path) else {
        return;
    };
    for import in python_imports(&source) {
        if let Some(path) = resolve_python_import(path, &import) {
            collect_python_file(&path, py_files, visited);
        }
    }
}

fn load_module_file(
    file_path: &Path,
    mod_prefix: &str,
    loaded: &mut HashSet<String>,
    file_id_counter: &mut usize,
    sources: &mut HashMap<usize, (String, String)>,
) -> Result<Vec<parser::Stmt>, Box<Diagnostic>> {
    let path_str = fs::canonicalize(file_path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| file_path.to_string_lossy().into_owned());

    if loaded.contains(&path_str) {
        return Ok(Vec::new());
    }
    loaded.insert(path_str.clone());

    let mut imported_stmts =
        load_and_parse_collecting(&path_str, false, loaded, file_id_counter, sources)?;

    let mut defined_names = HashSet::new();
    for s in &imported_stmts {
        match &s.kind {
            parser::StmtKind::Fn { name, .. }
            | parser::StmtKind::Struct { name, .. }
            | parser::StmtKind::Enum { name, .. }
            | parser::StmtKind::TypeAlias { name, .. }
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
            parser::StmtKind::FromImport { names, is_star, .. } if !*is_star => {
                for (name, alias) in names {
                    let bound = alias.as_deref().unwrap_or(name.as_str());
                    defined_names.insert(bound.to_string());
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
                | parser::StmtKind::TypeAlias { .. }
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

    Ok(imported_stmts)
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

    let mut program = match Parser::new(tokens).parse_program() {
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

    let file_dir = Path::new(filename)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    for stmt in &mut program.stmts {
        if let parser::StmtKind::NativeImport { path, .. } = &mut stmt.kind {
            let p = Path::new(path.as_str());
            if !p.is_absolute() && (path.contains('/') || path.contains('\\')) {
                let resolved = file_dir.join(p);
                if let Ok(canon) = resolved.canonicalize() {
                    *path = canon.to_string_lossy().to_string();
                } else {
                    *path = resolved.to_string_lossy().to_string();
                }
            } else if !p.is_absolute()
                && let Some(resolved) = resolve_pod_native(&file_dir, path)
            {
                POD_NATIVE_RESOLVED.with(|s| s.borrow_mut().insert(resolved.clone()));
                *path = resolved;
            }
        }
    }

    if !is_main {
        for stmt in &program.stmts {
            match &stmt.kind {
                parser::StmtKind::Fn { .. }
                | parser::StmtKind::Struct { .. }
                | parser::StmtKind::Impl { .. }
                | parser::StmtKind::Trait { .. }
                | parser::StmtKind::Enum { .. }
                | parser::StmtKind::TypeAlias { .. }
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
                        .note("imported modules may only declare items (fn, struct, impl, trait, enum, type, let, const, import)")
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
        let p = Path::new(filename);
        let stem = p.file_stem().unwrap_or_default().to_string_lossy();
        if stem == "mod" {
            p.parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| stem.to_string())
        } else {
            stem.to_string()
        }
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

                let mod_prefix = alias
                    .as_deref()
                    .unwrap_or_else(|| module.last().unwrap().as_str());

                match resolve_module_target(parent_dir, module) {
                    Ok(ResolvedModule::File(p)) | Ok(ResolvedModule::ModFile(p)) => {
                        let stmts =
                            load_module_file(&p, mod_prefix, loaded, file_id_counter, sources)?;
                        all_stmts.extend(stmts);
                    }
                    Ok(ResolvedModule::Directory(_dir_path, submodules)) => {
                        for sub_path in submodules {
                            if let Some(stem) = sub_path.file_stem().and_then(|s| s.to_str()) {
                                let sub_prefix = format!("{mod_prefix}::{stem}");
                                let stmts = load_module_file(
                                    &sub_path,
                                    &sub_prefix,
                                    loaded,
                                    file_id_counter,
                                    sources,
                                )?;
                                all_stmts.extend(stmts);
                            }
                        }
                    }
                    Err(ModuleResolutionError::Ambiguous {
                        module: m,
                        file_path,
                        mod_path,
                    }) => {
                        return Err(Box::new(
                            Diagnostic::error(
                                "E0302",
                                format!("ambiguous module `{m}`"),
                                stmt.span,
                            )
                            .label("imported here")
                            .note(format!(
                                "found both `{}` and `{}`",
                                file_path.display(),
                                mod_path.display()
                            ))
                            .help("remove or rename one of them to resolve the ambiguity"),
                        ));
                    }
                    Err(ModuleResolutionError::NotFound(m)) => {
                        if is_main && super::laws::is_laws_import(module, alias) {
                            all_stmts.push(super::laws::make_laws_stmt(stmt.span));
                            continue;
                        }
                        return Err(Box::new(
                            Diagnostic::error("E0300", format!("module `{m}` not found"), stmt.span)
                                .label("imported here")
                                .note("searched the project directory, the standard library, and installed pods")
                                .help(format!("create `{m}.liv` or `{m}/mod.liv` next to this file, or install the pod that provides it")),
                        ));
                    }
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

                match resolve_module_target(parent_dir, module) {
                    Ok(ResolvedModule::File(p)) | Ok(ResolvedModule::ModFile(p)) => {
                        let path_str = fs::canonicalize(&p)
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_else(|_| p.to_string_lossy().into_owned());

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
                    }
                    Ok(ResolvedModule::Directory(_dir_path, submodules)) => {
                        for sub_path in submodules {
                            let path_str = fs::canonicalize(&sub_path)
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or_else(|_| sub_path.to_string_lossy().into_owned());

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
                        }
                    }
                    Err(ModuleResolutionError::Ambiguous {
                        module: m,
                        file_path,
                        mod_path,
                    }) => {
                        return Err(Box::new(
                            Diagnostic::error(
                                "E0302",
                                format!("ambiguous module `{m}`"),
                                stmt.span,
                            )
                            .label("imported here")
                            .note(format!(
                                "found both `{}` and `{}`",
                                file_path.display(),
                                mod_path.display()
                            ))
                            .help("remove or rename one of them to resolve the ambiguity"),
                        ));
                    }
                    Err(ModuleResolutionError::NotFound(m)) => {
                        return Err(Box::new(
                            Diagnostic::error("E0300", format!("module `{m}` not found"), stmt.span)
                                .label("imported here")
                                .note("searched the project directory, the standard library, and installed pods")
                                .help(format!("create `{m}.liv` or `{m}/mod.liv` next to this file, or install the pod that provides it")),
                        ));
                    }
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
        Ok(s) => crate::lexer::normalize_newlines(&s),
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
                if let Ok(resolved) = resolve_module_target(&parent_dir, module) {
                    match resolved {
                        ResolvedModule::File(p) | ResolvedModule::ModFile(p) => {
                            collect_source_files(
                                p.to_string_lossy().as_ref(),
                                collected,
                                py_files,
                                visited,
                            );
                        }
                        ResolvedModule::Directory(_dir, submodules) => {
                            for sub in submodules {
                                collect_source_files(
                                    sub.to_string_lossy().as_ref(),
                                    collected,
                                    py_files,
                                    visited,
                                );
                            }
                        }
                    }
                }
            }
            parser::StmtKind::PyImport { module, .. } => {
                if let Some(path) = resolve_python_module(&parent_dir, module) {
                    collect_python_file(&path, py_files, visited);
                }
            }
            // A native library is part of the build's actual input: rebuilding
            // it (a new mtime) must invalidate the AOT cache exactly like
            // editing a .liv file does, or `pit run` would keep executing a
            // binary linked against the previous library.
            parser::StmtKind::NativeImport { path, .. } => {
                let p = Path::new(path.as_str());
                let resolved = if p.is_absolute() {
                    Some(path.clone())
                } else if path.contains('/') || path.contains('\\') {
                    let joined = parent_dir.join(p);
                    Some(
                        joined
                            .canonicalize()
                            .map(|c| c.to_string_lossy().to_string())
                            .unwrap_or_else(|_| joined.to_string_lossy().to_string()),
                    )
                } else {
                    resolve_pod_native(&parent_dir, path)
                };
                if let Some(resolved) = resolved
                    && Path::new(&resolved).is_file()
                    && visited.insert(resolved.clone())
                {
                    collected.push(resolved);
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

    #[test]
    fn test_resolve_module_target_variants() {
        let temp_dir =
            std::env::temp_dir().join(format!("olive_mod_resolve_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).unwrap();

        // 1. Single file
        let single_file = temp_dir.join("single.liv");
        fs::write(&single_file, "pass\n").unwrap();
        let res = resolve_module_target(&temp_dir, &["single".to_string()]).unwrap();
        assert_eq!(res, ResolvedModule::File(single_file));

        // 2. Directory with mod.liv
        let pkg_dir = temp_dir.join("pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        let mod_file = pkg_dir.join("mod.liv");
        fs::write(&mod_file, "pass\n").unwrap();
        let res = resolve_module_target(&temp_dir, &["pkg".to_string()]).unwrap();
        assert_eq!(res, ResolvedModule::ModFile(mod_file));

        // 3. Directory without mod.liv (optional mod.liv)
        let sub_dir = temp_dir.join("ns");
        fs::create_dir_all(&sub_dir).unwrap();
        let a = sub_dir.join("a.liv");
        let b = sub_dir.join("b.liv");
        fs::write(&a, "pass\n").unwrap();
        fs::write(&b, "pass\n").unwrap();
        let res = resolve_module_target(&temp_dir, &["ns".to_string()]).unwrap();
        assert_eq!(res, ResolvedModule::Directory(sub_dir, vec![a, b]));

        // 4. Ambiguous: both foo.liv and foo/mod.liv
        let amb_dir = temp_dir.join("amb");
        fs::create_dir_all(&amb_dir).unwrap();
        let amb_file = temp_dir.join("amb.liv");
        let amb_mod = amb_dir.join("mod.liv");
        fs::write(&amb_file, "pass\n").unwrap();
        fs::write(&amb_mod, "pass\n").unwrap();
        let err = resolve_module_target(&temp_dir, &["amb".to_string()]).unwrap_err();
        assert!(matches!(err, ModuleResolutionError::Ambiguous { .. }));

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_load_and_parse_mod_liv() {
        let temp_dir = std::env::temp_dir().join(format!("olive_mod_load_{}", std::process::id()));
        let pkg_dir = temp_dir.join("tokenizer");
        fs::create_dir_all(&pkg_dir).unwrap();

        let main_path = temp_dir.join("main.liv");
        let mod_path = pkg_dir.join("mod.liv");
        fs::write(&main_path, "import tokenizer\n").unwrap();
        fs::write(
            &mod_path,
            "fn tokenize() -> int:\n    return 42\nstruct Tokenizer:\n    val: int\n",
        )
        .unwrap();

        let mut loaded = HashSet::new();
        let mut file_id_counter = 0;
        let mut sources = HashMap::default();

        let stmts = load_and_parse(
            &main_path.to_string_lossy(),
            true,
            &mut loaded,
            &mut file_id_counter,
            &mut sources,
        )
        .unwrap();

        let has_mangled_fn = stmts.iter().any(|s| match &s.kind {
            parser::StmtKind::Fn { name, .. } => name == "tokenizer::tokenize",
            _ => false,
        });
        assert!(has_mangled_fn, "expected tokenizer::tokenize from mod.liv");

        let has_mangled_struct = stmts.iter().any(|s| match &s.kind {
            parser::StmtKind::Struct { name, .. } => name == "tokenizer::Tokenizer",
            _ => false,
        });
        assert!(
            has_mangled_struct,
            "expected tokenizer::Tokenizer from mod.liv"
        );

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_load_and_parse_directory_optional_mod() {
        let temp_dir = std::env::temp_dir().join(format!("olive_dir_load_{}", std::process::id()));
        let pkg_dir = temp_dir.join("tokenizer");
        fs::create_dir_all(&pkg_dir).unwrap();

        let main_path = temp_dir.join("main.liv");
        let bpe_path = pkg_dir.join("bpe.liv");
        let tok_path = pkg_dir.join("tokenizer.liv");
        fs::write(&main_path, "import tokenizer\n").unwrap();
        fs::write(&bpe_path, "fn encode() -> int:\n    return 1\n").unwrap();
        fs::write(&tok_path, "struct Tokenizer:\n    id: int\n").unwrap();

        let mut loaded = HashSet::new();
        let mut file_id_counter = 0;
        let mut sources = HashMap::default();

        let stmts = load_and_parse(
            &main_path.to_string_lossy(),
            true,
            &mut loaded,
            &mut file_id_counter,
            &mut sources,
        )
        .unwrap();

        let has_bpe_fn = stmts.iter().any(|s| match &s.kind {
            parser::StmtKind::Fn { name, .. } => name == "tokenizer::bpe::encode",
            _ => false,
        });
        assert!(
            has_bpe_fn,
            "expected tokenizer::bpe::encode in directory module"
        );

        let has_tok_struct = stmts.iter().any(|s| match &s.kind {
            parser::StmtKind::Struct { name, .. } => name == "tokenizer::tokenizer::Tokenizer",
            _ => false,
        });
        assert!(
            has_tok_struct,
            "expected tokenizer::tokenizer::Tokenizer in directory module"
        );

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_mod_name_binds_to_parent_dir() {
        let temp_dir = std::env::temp_dir().join(format!("olive_mod_name_{}", std::process::id()));
        let pkg_dir = temp_dir.join("tokenizer");
        fs::create_dir_all(&pkg_dir).unwrap();

        let mod_path = pkg_dir.join("mod.liv");
        fs::write(&mod_path, "pass\n").unwrap();

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

        let name_const = stmts.iter().find_map(|s| match &s.kind {
            parser::StmtKind::Const { name, value, .. } if name == "__name__" => {
                match &value.kind {
                    parser::ExprKind::Str(val) => Some(val.clone()),
                    _ => None,
                }
            }
            _ => None,
        });

        assert_eq!(name_const.as_deref(), Some("tokenizer"));

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_collect_source_files_tracks_python_import_relative_to_source() {
        let temp_dir =
            std::env::temp_dir().join(format!("olive_python_dep_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).unwrap();
        let main_path = temp_dir.join("main.liv");
        let helper_path = temp_dir.join("helper.py");
        fs::write(&main_path, "import py \"helper\" as h\n").unwrap();
        fs::write(&helper_path, "def value():\n    return 1\n").unwrap();

        let mut collected = Vec::new();
        let mut py_files = Vec::new();
        let mut visited = HashSet::new();
        collect_source_files(
            main_path.to_str().unwrap(),
            &mut collected,
            &mut py_files,
            &mut visited,
        );

        let helper_canonical = fs::canonicalize(&helper_path)
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(py_files.contains(&helper_canonical));
        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_collect_source_files_tracks_nested_python_imports() {
        let temp_dir = std::env::temp_dir().join(format!(
            "olive_python_nested_dep_{}_{}",
            std::process::id(),
            line!()
        ));
        let package_dir = temp_dir.join("pkg");
        fs::create_dir_all(&package_dir).unwrap();
        let main_path = temp_dir.join("main.liv");
        let init_path = package_dir.join("__init__.py");
        let dep_path = package_dir.join("dep.py");
        let leaf_path = package_dir.join("leaf.py");
        fs::write(&main_path, "import py \"pkg\" as p\n").unwrap();
        fs::write(&init_path, "from .dep import value\n").unwrap();
        fs::write(&dep_path, "from .leaf import value\n").unwrap();
        fs::write(&leaf_path, "value = 1\n").unwrap();

        let mut collected = Vec::new();
        let mut py_files = Vec::new();
        let mut visited = HashSet::new();
        collect_source_files(
            main_path.to_str().unwrap(),
            &mut collected,
            &mut py_files,
            &mut visited,
        );

        for path in [&init_path, &dep_path, &leaf_path] {
            let canonical = fs::canonicalize(path)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            assert!(py_files.contains(&canonical), "missing {}", canonical);
        }
        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_collect_source_files_tracks_directory_module() {
        let temp_dir = std::env::temp_dir().join(format!("olive_csf_{}", std::process::id()));
        let pkg_dir = temp_dir.join("tokenizer");
        fs::create_dir_all(&pkg_dir).unwrap();

        let main_path = temp_dir.join("main.liv");
        let bpe_path = pkg_dir.join("bpe.liv");
        let tok_path = pkg_dir.join("tokenizer.liv");
        fs::write(&main_path, "import tokenizer\n").unwrap();
        fs::write(&bpe_path, "pass\n").unwrap();
        fs::write(&tok_path, "pass\n").unwrap();

        let mut collected = Vec::new();
        let mut py_files = Vec::new();
        let mut visited = HashSet::new();

        collect_source_files(
            main_path.to_str().unwrap(),
            &mut collected,
            &mut py_files,
            &mut visited,
        );

        let main_canon = fs::canonicalize(&main_path)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let bpe_canon = fs::canonicalize(&bpe_path)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let tok_canon = fs::canonicalize(&tok_path)
            .unwrap()
            .to_string_lossy()
            .to_string();

        assert!(collected.contains(&main_canon));
        assert!(collected.contains(&bpe_canon));
        assert!(collected.contains(&tok_canon));

        fs::remove_dir_all(&temp_dir).ok();
    }
}
