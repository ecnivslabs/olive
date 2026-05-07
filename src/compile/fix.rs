//! Compiler-native autofix. `pit fix` runs the front end exactly as a normal
//! build would, gathers the machine-applicable suggestions every diagnostic
//! carries, and rewrites the source in place: no external tool, no second
//! parser, the same spans the error renderer points at. Only fixes the compiler
//! is certain about are applied; anything advisory is left to the programmer.

use super::errors::{Applicability, Diagnostic, Sources};
use super::loader::load_and_parse;
use crate::parser::Program;
use crate::semantic::lint::lint_program;
use crate::semantic::{Resolver, TypeChecker};
use rustc_hash::FxHashMap as HashMap;
use std::collections::HashSet;

/// Outcome of an autofix run, so the caller can choose an exit code and report
/// without re-deriving anything.
pub struct FixReport {
    pub applied: usize,
    pub files_changed: usize,
    pub codes: Vec<String>,
}

/// Collects every diagnostic the front end produces for `filename`, in the same
/// order a build would surface them. Resolution errors short-circuit the later
/// passes, since a program with unresolved names is not yet meaningful to
/// type-check, but their suggestions are still returned so a typo can be fixed first.
fn collect_diagnostics(filename: &str) -> Result<(Vec<Diagnostic>, Sources), ()> {
    let mut loaded = HashSet::new();
    loaded.insert(filename.to_string());
    let mut file_id_counter = 0;
    let mut sources = HashMap::default();
    let stmts = load_and_parse(
        filename,
        true,
        &mut loaded,
        &mut file_id_counter,
        &mut sources,
    )?;
    let program = Program { stmts };

    let mut diagnostics = Vec::new();
    let mut resolver = Resolver::new();
    resolver.resolve_program(&program);
    for e in &resolver.errors {
        diagnostics.push(e.to_diagnostic());
    }
    for w in &resolver.warnings {
        diagnostics.push(w.to_diagnostic());
    }
    if !resolver.errors.is_empty() {
        return Ok((diagnostics, sources));
    }

    let mut type_checker = TypeChecker::new();
    type_checker.check_program(&program);
    for w in &type_checker.warnings {
        diagnostics.push(w.to_diagnostic());
    }
    for e in &type_checker.errors {
        diagnostics.push(e.to_diagnostic());
    }
    diagnostics.extend(lint_program(&program));

    Ok((diagnostics, sources))
}

/// A single resolved edit against one file's byte range.
struct Edit {
    start: usize,
    end: usize,
    replacement: String,
    code: Option<String>,
}

/// Runs the front end, applies every machine-applicable fix to disk (unless
/// `dry_run`), and returns what changed. Advisory suggestions are reported but
/// never written.
pub fn run_fix(filename: &str, dry_run: bool) -> Result<FixReport, ()> {
    let (diagnostics, sources) = collect_diagnostics(filename)?;
    let first_party = super::pipeline::first_party_files(filename, &sources);

    let mut per_file: HashMap<usize, Vec<Edit>> = HashMap::default();
    for diag in &diagnostics {
        for sug in diag.suggestions() {
            if sug.applicability != Applicability::MachineApplicable
                || !first_party.contains(&sug.span.file_id)
            {
                continue;
            }
            per_file.entry(sug.span.file_id).or_default().push(Edit {
                start: sug.span.start,
                end: sug.span.end,
                replacement: sug.replacement.clone(),
                code: diag.code().map(str::to_string),
            });
        }
    }

    let mut applied = 0;
    let mut files_changed = 0;
    let mut codes: Vec<String> = Vec::new();
    for (file_id, mut edits) in per_file {
        let Some((path, original)) = sources.get(&file_id) else {
            continue;
        };
        // Apply from the end backwards so earlier offsets stay valid as text is
        // spliced. Overlapping edits are dropped; only the rightmost of an
        // overlapping pair survives, never a partial mangle.
        edits.sort_by(|a, b| b.start.cmp(&a.start).then(b.end.cmp(&a.end)));
        let mut text = original.clone();
        let mut last_start = usize::MAX;
        let mut file_applied = 0;
        for edit in edits {
            if edit.end > last_start {
                continue;
            }
            if edit.start > edit.end
                || edit.end > text.len()
                || !text.is_char_boundary(edit.start)
                || !text.is_char_boundary(edit.end)
            {
                continue;
            }
            text.replace_range(edit.start..edit.end, &edit.replacement);
            last_start = edit.start;
            file_applied += 1;
            if let Some(c) = edit.code
                && !codes.contains(&c)
            {
                codes.push(c);
            }
        }
        if file_applied == 0 {
            continue;
        }
        applied += file_applied;
        files_changed += 1;
        if !dry_run && let Err(e) = std::fs::write(path, &text) {
            eprintln!("error writing {path}: {e}");
            return Err(());
        }
    }

    codes.sort();
    Ok(FixReport {
        applied,
        files_changed,
        codes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A scratch `.liv` in its own directory, removed whole on drop so no temp
    /// artifact survives the test (and each test stays isolated from the others).
    struct TempProject {
        dir: std::path::PathBuf,
        file: std::path::PathBuf,
    }

    impl TempProject {
        fn new(name: &str, src: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("olive_fix_{}_{name}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("main.liv");
            std::fs::File::create(&file)
                .unwrap()
                .write_all(src.as_bytes())
                .unwrap();
            Self { dir, file }
        }

        fn path(&self) -> &str {
            self.file.to_str().unwrap()
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    #[test]
    fn applies_name_typo_fix() {
        let p = TempProject::new("typo", "let total = 1\nprint(totl)\n");
        let report = run_fix(p.path(), false).unwrap();
        assert_eq!(report.applied, 1);
        assert_eq!(report.files_changed, 1);
        assert!(
            std::fs::read_to_string(&p.file)
                .unwrap()
                .contains("print(total)")
        );
    }

    #[test]
    fn dry_run_leaves_file_untouched() {
        let src = "let total = 1\nprint(totl)\n";
        let p = TempProject::new("dry", src);
        let report = run_fix(p.path(), true).unwrap();
        assert_eq!(report.applied, 1);
        assert_eq!(std::fs::read_to_string(&p.file).unwrap(), src);
    }

    #[test]
    fn nothing_to_fix_reports_zero() {
        let p = TempProject::new("clean", "let x = 1\nprint(x)\n");
        let report = run_fix(p.path(), false).unwrap();
        assert_eq!(report.applied, 0);
        assert_eq!(report.files_changed, 0);
    }
}
