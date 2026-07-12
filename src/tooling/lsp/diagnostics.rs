//! Builds `publishDiagnostics` params, grouped by file; edited file always
//! included (even empty) so the editor can clear stale errors.

use super::position::LineIndex;
use super::uri::path_to_uri;
use crate::compile::diagnose::DiagnoseOutput;
use crate::compile::errors::Diagnostic;
use rustc_hash::FxHashMap as HashMap;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;

fn severity(is_error: bool) -> i64 {
    if is_error { 1 } else { 2 }
}

/// Location for `span`, using its own file_id (may differ from the primary span's).
fn location_json(output: &DiagnoseOutput, span: crate::span::Span) -> Value {
    let (path_str, source_text) = output
        .sources
        .get(&span.file_id)
        .cloned()
        .unwrap_or_default();
    let range = LineIndex::new(&source_text).span_to_range(span);
    json!({
        "uri": path_to_uri(Path::new(&path_str)),
        "range": range.to_json(),
    })
}

fn diagnostic_to_json(output: &DiagnoseOutput, d: &Diagnostic, line_index: &LineIndex) -> Value {
    let range = line_index.span_to_range(d.primary_span());
    let mut message = d.headline().to_string();
    if let Some(label) = d.primary_label() {
        message.push_str(&format!("\n{label}"));
    }
    for note in d.notes() {
        message.push_str(&format!("\nnote: {note}"));
    }
    for help in d.helps() {
        message.push_str(&format!("\nhelp: {help}"));
    }

    let related: Vec<Value> = d
        .secondary_labels()
        .iter()
        .map(|(span, text)| {
            json!({
                "location": location_json(output, *span),
                "message": text,
            })
        })
        .collect();

    let mut obj = json!({
        "range": range.to_json(),
        "severity": severity(d.is_error()),
        "source": "pit",
        "message": message,
    });
    if let Some(code) = d.code() {
        obj["code"] = json!(code);
    }
    if !related.is_empty() {
        obj["relatedInformation"] = json!(related);
    }
    obj
}

fn build_one(output: &DiagnoseOutput, file_id: usize, diags: &[&Diagnostic]) -> (String, Value) {
    let (path_str, source_text) = output.sources.get(&file_id).cloned().unwrap_or_default();
    let line_index = LineIndex::new(&source_text);
    let items: Vec<Value> = diags
        .iter()
        .map(|d| diagnostic_to_json(output, d, &line_index))
        .collect();
    let uri = path_to_uri(Path::new(&path_str));
    (uri.clone(), json!({"uri": uri, "diagnostics": items}))
}

/// One `(uri, publishDiagnostics params)` pair per affected file.
pub fn publish_params(output: &DiagnoseOutput, main_path: &Path) -> Vec<(String, Value)> {
    let mut by_file: HashMap<usize, Vec<&Diagnostic>> = HashMap::default();
    for d in &output.diagnostics {
        by_file.entry(d.primary_span().file_id).or_default().push(d);
    }

    let mut results = Vec::new();
    let mut seen = HashSet::new();

    let main_file_id = output
        .sources
        .iter()
        .find(|(_, (p, _))| Path::new(p) == main_path)
        .map(|(id, _)| *id);
    if let Some(id) = main_file_id {
        let empty: Vec<&Diagnostic> = Vec::new();
        results.push(build_one(output, id, by_file.get(&id).unwrap_or(&empty)));
        seen.insert(id);
    }
    for (&id, diags) in &by_file {
        if seen.contains(&id) {
            continue;
        }
        results.push(build_one(output, id, diags));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::diagnose::diagnose;
    use std::io::Write;

    fn write_temp(name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("olive_lsp_diag_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn clean_file_publishes_empty_array_for_main_file() {
        let path = write_temp("clean.liv", "let x = 1\nprint(x)\n");
        let out = diagnose(path.to_str().unwrap());
        let params = publish_params(&out, &path);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].1["diagnostics"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn error_publishes_nonempty_array_with_range_and_code() {
        let path = write_temp("err.liv", "print(nope)\n");
        let out = diagnose(path.to_str().unwrap());
        let params = publish_params(&out, &path);
        assert_eq!(params.len(), 1);
        let diags = params[0].1["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0]["code"], "E0001");
        assert_eq!(diags[0]["severity"], 1);
        assert!(diags[0]["range"]["start"]["line"].is_number());
    }
}
