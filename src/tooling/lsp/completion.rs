//! `textDocument/completion`: dot-completion splices a placeholder identifier
//! since bare `.` doesn't parse; elsewhere, scope names plus keywords.

use super::locate::find_expr_at;
use super::position::{LineIndex, LspPosition};
use super::scope::{Binding, BindingKind};
use crate::compile::diagnose::{DiagnoseOutput, diagnose};
use crate::compile::loader;
use crate::semantic::types::Type;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Method,
    Field,
    Variable,
    Function,
    Struct,
    Enum,
    Module,
    Keyword,
}

#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
}

const LIST_METHODS: &[&str] = &[
    "append", "insert", "extend", "sort", "reverse", "pop", "remove", "count", "index", "clear",
];
const DICT_METHODS: &[&str] = &[
    "get",
    "keys",
    "values",
    "items",
    "update",
    "pop",
    "setdefault",
    "clear",
    "remove",
];
const SET_METHODS: &[&str] = &["add", "remove", "contains", "discard", "clear"];
const STR_METHODS: &[&str] = &[
    "upper",
    "lower",
    "strip",
    "lstrip",
    "rstrip",
    "replace",
    "repeat",
    "join",
    "split",
    "find",
    "contains",
    "startswith",
    "endswith",
    "count",
    "rfind",
    "splitlines",
    "title",
    "capitalize",
    "zfill",
    "ljust",
    "rjust",
    "center",
    "partition",
    "removeprefix",
    "removesuffix",
    "isdigit",
    "isalpha",
    "isspace",
    "isupper",
    "islower",
    "to_int",
    "to_float",
];

const KEYWORDS: &[&str] = &[
    "fn", "let", "const", "if", "else", "elif", "while", "for", "in", "return", "True", "False",
    "None", "not", "and", "or", "pass", "break", "continue", "try", "as", "assert", "import",
    "from", "struct", "impl", "trait", "mut", "enum", "match", "async", "await", "unsafe", "defer",
    "with", "lambda", "by",
];

fn methods_for_type(ty: &Type) -> &'static [&'static str] {
    match ty {
        Type::List(_) => LIST_METHODS,
        Type::Dict(_, _) => DICT_METHODS,
        Type::Set(_) => SET_METHODS,
        Type::Str => STR_METHODS,
        _ => &[],
    }
}

/// Struct field names from the checker's field table, not a fixed list.
fn fields_for_type<'a>(out: &'a DiagnoseOutput, ty: &Type) -> &'a [String] {
    match ty {
        Type::Struct(name, ..) => out
            .struct_fields
            .get(name)
            .map(|v| v.as_slice())
            .unwrap_or(&[]),
        _ => &[],
    }
}

fn char_offset_to_byte_offset(text: &str, char_offset: usize) -> usize {
    text.char_indices()
        .nth(char_offset)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

/// Trigger length in chars ending at the cursor: 1 for `.`, 2 for `?.`.
fn trigger_len(chars: &[char], offset: usize) -> Option<usize> {
    if offset >= 1 && chars[offset - 1] == '.' {
        if offset >= 2 && chars[offset - 2] == '?' {
            return Some(2);
        }
        return Some(1);
    }
    None
}

fn dot_completions(path: &Path, text: &str, offset: usize, trigger: usize) -> Vec<CompletionItem> {
    let byte_at = char_offset_to_byte_offset(text, offset);
    let mut spliced = String::with_capacity(text.len() + 20);
    spliced.push_str(&text[..byte_at]);
    spliced.push_str("__pit_completion_probe__");
    spliced.push_str(&text[byte_at..]);

    let path_str = path.to_string_lossy().to_string();
    let original = text.to_string();
    loader::set_source_overlay(&path_str, spliced);
    let out = diagnose(&path_str);
    loader::set_source_overlay(&path_str, original);

    let Some(program) = &out.program else {
        return Vec::new();
    };
    let file_id = match out.sources.iter().find(|(_, (p, _))| Path::new(p) == path) {
        Some((id, _)) => *id,
        None => return Vec::new(),
    };
    // Receiver span ends at the dot, inclusive of find_expr_at's contains check.
    let receiver_query_offset = offset.saturating_sub(trigger);
    let Some(receiver) = find_expr_at(program, file_id, receiver_query_offset) else {
        return Vec::new();
    };
    let Some(ty) = out.expr_types.get(&receiver.id) else {
        return Vec::new();
    };
    let methods = methods_for_type(ty).iter().map(|&name| CompletionItem {
        label: name.to_string(),
        kind: CompletionKind::Method,
    });
    let fields = fields_for_type(&out, ty).iter().map(|name| CompletionItem {
        label: name.clone(),
        kind: CompletionKind::Field,
    });
    methods.chain(fields).collect()
}

fn map_binding_kind(kind: BindingKind) -> CompletionKind {
    match kind {
        BindingKind::Function => CompletionKind::Function,
        BindingKind::Struct => CompletionKind::Struct,
        BindingKind::Enum => CompletionKind::Enum,
        BindingKind::Variable | BindingKind::Parameter => CompletionKind::Variable,
        BindingKind::Module => CompletionKind::Module,
    }
}

fn scope_completions(
    last_good: Option<&DiagnoseOutput>,
    file_id: usize,
    offset: usize,
) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = KEYWORDS
        .iter()
        .map(|&k| CompletionItem {
            label: k.to_string(),
            kind: CompletionKind::Keyword,
        })
        .collect();
    if let Some(out) = last_good
        && let Some(program) = &out.program
    {
        let bindings: Vec<Binding> = super::scope::visible_bindings_at(program, file_id, offset);
        items.extend(bindings.into_iter().map(|b| CompletionItem {
            label: b.name,
            kind: map_binding_kind(b.kind),
        }));
    }
    items
}

pub fn completions_at(
    path: &Path,
    text: &str,
    pos: LspPosition,
    last_good: Option<&DiagnoseOutput>,
) -> Vec<CompletionItem> {
    let line_index = LineIndex::new(text);
    let offset = line_index.to_char_offset(pos);
    let chars: Vec<char> = text.chars().collect();

    if let Some(trigger) = trigger_len(&chars, offset) {
        return dot_completions(path, text, offset, trigger);
    }

    let file_id = last_good
        .and_then(|out| out.sources.iter().find(|(_, (p, _))| Path::new(p) == path))
        .map(|(id, _)| *id)
        .unwrap_or(0);
    scope_completions(last_good, file_id, offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, content: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("olive_completion_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn dot_after_list_local_suggests_list_methods() {
        let src = "fn main():\n    let xs = [1, 2, 3]\n    xs.\n";
        let path = write_temp("dotlist.liv", src);
        let dot_offset = src.find("xs.\n").unwrap() + 3; // char count == byte count, ascii
        let pos = LineIndex::new(src).to_lsp(dot_offset);
        let items = completions_at(&path, src, pos, None);
        let labels: Vec<&str> = items.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"append"));
        assert!(labels.contains(&"sort"));
        assert!(!labels.contains(&"upper"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dot_after_string_local_suggests_string_methods() {
        let src = "fn main():\n    let s = \"hi\"\n    s.\n";
        let path = write_temp("dotstr.liv", src);
        let dot_offset = src.rfind(".\n").unwrap() + 1;
        let pos = LineIndex::new(src).to_lsp(dot_offset);
        let items = completions_at(&path, src, pos, None);
        let labels: Vec<&str> = items.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"upper"));
        assert!(labels.contains(&"split"));
        assert!(!labels.contains(&"append"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dot_after_struct_local_suggests_its_fields() {
        let src = "struct Point:\n    x: int\n    y: int\n\nfn main():\n    let p = Point(1, 2)\n    p.\n";
        let path = write_temp("dotstruct.liv", src);
        let dot_offset = src.rfind("p.\n").unwrap() + 2;
        let pos = LineIndex::new(src).to_lsp(dot_offset);
        let items = completions_at(&path, src, pos, None);
        let labels: Vec<&str> = items.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"x"));
        assert!(labels.contains(&"y"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn non_dot_position_includes_keywords() {
        let src = "fn main():\n    let x = 1\n";
        let path = write_temp("kw.liv", src);
        let pos = LineIndex::new(src).to_lsp(src.len());
        let items = completions_at(&path, src, pos, None);
        assert!(items.iter().any(|c| c.label == "return"));
        std::fs::remove_file(&path).ok();
    }
}
