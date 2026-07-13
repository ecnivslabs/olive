//! `textDocument/hover`: type of the smallest AST node covering the cursor,
//! plus (E13.4) the `///` doc comment on whatever it resolves to, the same
//! text `pit doc` renders -- one extraction function, two consumers.

use super::locate::find_expr_at;
use crate::compile::diagnose::DiagnoseOutput;
use crate::span::Span;
use crate::tooling::doc_comments::extract_for_item;

/// Type (plus doc comment, if the hovered name resolves to a documented
/// definition) and span at `offset`, or `None` if the cursor isn't over a
/// typed expression.
pub fn hover_at(output: &DiagnoseOutput, file_id: usize, offset: usize) -> Option<(String, Span)> {
    let program = output.program.as_ref()?;
    let expr = find_expr_at(program, file_id, offset)?;
    let ty = output.expr_types.get(&expr.id)?;
    let mut text = ty.to_string();

    if let Some(def_span) = output.def_sites.get(&expr.id)
        && let Some((_, content)) = output.sources.get(&def_span.file_id)
    {
        let lines: Vec<&str> = content.lines().collect();
        if let Some(doc) = extract_for_item(&lines, def_span.line) {
            text.push_str("\n\n");
            text.push_str(&doc);
        }
    }

    Some((text, expr.span))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::diagnose::diagnose;
    use std::io::Write;

    fn write_temp(name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("olive_hover_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn hover_on_local_int_shows_int() {
        let src = "fn main():\n    let count = 42\n    print(count)\n";
        let path = write_temp("hover_int.liv", src);
        let out = diagnose(path.to_str().unwrap());
        let file_id = out
            .sources
            .iter()
            .find(|(_, (p, _))| p == path.to_str().unwrap())
            .map(|(id, _)| *id)
            .unwrap();
        let offset = src.rfind("count)").unwrap() + 1; // inside the second `count`
        let (ty, _) = hover_at(&out, file_id, offset).expect("hover result");
        assert_eq!(ty, "int");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn hover_on_documented_function_call_includes_doc_comment() {
        let src = "/// Adds one to `x`.\nfn inc(x: int) -> int:\n    return x + 1\n\nfn main():\n    print(inc(1))\n";
        let path = write_temp("hover_doc.liv", src);
        let out = diagnose(path.to_str().unwrap());
        let file_id = out
            .sources
            .iter()
            .find(|(_, (p, _))| p == path.to_str().unwrap())
            .map(|(id, _)| *id)
            .unwrap();
        let offset = src.rfind("inc(1)").unwrap() + 1; // inside "inc"
        let (text, _) = hover_at(&out, file_id, offset).expect("hover result");
        assert!(text.contains("Adds one to `x`."), "hover text: {text}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn hover_outside_any_expr_is_none() {
        // Offset 0 hits the loader's synthetic __name__ const span; use whitespace instead.
        let src = "fn main():\n    let x = 1\n";
        let path = write_temp("hover_none.liv", src);
        let out = diagnose(path.to_str().unwrap());
        let file_id = out
            .sources
            .iter()
            .find(|(_, (p, _))| p == path.to_str().unwrap())
            .map(|(id, _)| *id)
            .unwrap();
        let whitespace_offset = src.find("    let").unwrap() + 1;
        assert!(hover_at(&out, file_id, whitespace_offset).is_none());
        std::fs::remove_file(&path).ok();
    }
}
