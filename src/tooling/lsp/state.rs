//! Open documents plus each one's last successfully-parsed snapshot, so
//! hover/definition/completion survive a momentary syntax error mid-edit.

use crate::compile::diagnose::DiagnoseOutput;
use crate::compile::loader;
use rustc_hash::FxHashMap as HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct Document {
    pub text: String,
    pub version: i64,
    pub last_good: Option<Rc<DiagnoseOutput>>,
}

#[derive(Default)]
pub struct ServerState {
    pub documents: HashMap<PathBuf, Document>,
    pub initialized: bool,
    pub shutdown_requested: bool,
}

impl ServerState {
    /// Registers the document and returns a fresh `DiagnoseOutput` to publish.
    pub fn open(&mut self, path: PathBuf, text: String, version: i64) -> Rc<DiagnoseOutput> {
        loader::set_source_overlay(&path.to_string_lossy(), text.clone());
        let out = Rc::new(crate::compile::diagnose::diagnose(&path.to_string_lossy()));
        let last_good = out.program.is_some().then(|| out.clone());
        self.documents.insert(
            path,
            Document {
                text,
                version,
                last_good,
            },
        );
        out
    }

    pub fn change(
        &mut self,
        path: &Path,
        text: String,
        version: i64,
    ) -> Option<Rc<DiagnoseOutput>> {
        // Bail before touching overlay: unopened doc has no close to clear it later.
        if !self.documents.contains_key(path) {
            return None;
        }
        loader::set_source_overlay(&path.to_string_lossy(), text.clone());
        let out = Rc::new(crate::compile::diagnose::diagnose(&path.to_string_lossy()));
        let doc = self.documents.get_mut(path)?;
        doc.text = text;
        doc.version = version;
        if out.program.is_some() {
            doc.last_good = Some(out.clone());
        }
        Some(out)
    }

    pub fn close(&mut self, path: &Path) {
        loader::clear_source_overlay(&path.to_string_lossy());
        self.documents.remove(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "olive_lsp_state_test_{}_{}",
            std::process::id(),
            name
        ))
    }

    #[test]
    fn open_registers_document_and_diagnoses() {
        let mut state = ServerState::default();
        let path = temp_path("open.liv");
        std::fs::write(&path, "let x = 1\n").unwrap();
        let out = state.open(path.clone(), "print(nope)\n".to_string(), 1);
        assert!(!out.diagnostics.is_empty());
        assert!(state.documents.contains_key(&path));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn change_updates_text_and_preserves_last_good_on_syntax_error() {
        let mut state = ServerState::default();
        let path = temp_path("change.liv");
        std::fs::write(&path, "let x = 1\n").unwrap();
        state.open(path.clone(), "let x = 1\nprint(x)\n".to_string(), 1);
        assert!(state.documents[&path].last_good.is_some());

        // A syntax error must not wipe out the last good snapshot.
        state.change(&path, "let x = \n".to_string(), 2);
        let doc = &state.documents[&path];
        assert_eq!(doc.version, 2);
        assert!(doc.last_good.is_some());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn change_on_unopened_document_is_a_no_op_and_leaves_no_overlay() {
        let mut state = ServerState::default();
        let path = temp_path("never_opened.liv");
        std::fs::write(&path, "let x = 1\n").unwrap();
        assert!(
            state
                .change(&path, "print(nope)\n".to_string(), 1)
                .is_none()
        );
        assert!(!state.documents.contains_key(&path));
        // No overlay leaked: diagnosing straight from disk sees the clean file.
        let out = crate::compile::diagnose::diagnose(path.to_str().unwrap());
        assert!(out.diagnostics.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn close_removes_document_and_clears_overlay() {
        let mut state = ServerState::default();
        let path = temp_path("close.liv");
        std::fs::write(&path, "let x = 1\n").unwrap();
        state.open(path.clone(), "let x = 1\n".to_string(), 1);
        state.close(&path);
        assert!(!state.documents.contains_key(&path));
        let out = crate::compile::diagnose::diagnose(path.to_str().unwrap());
        assert!(out.diagnostics.is_empty());
        std::fs::remove_file(&path).ok();
    }
}
