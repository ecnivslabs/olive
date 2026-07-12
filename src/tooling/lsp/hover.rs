//! `textDocument/hover`: type of the smallest AST node covering the cursor.

use super::locate::find_expr_at;
use crate::compile::diagnose::DiagnoseOutput;
use crate::span::Span;

/// Type and span at `offset`, or `None` if the cursor isn't over a typed expression.
pub fn hover_at(output: &DiagnoseOutput, file_id: usize, offset: usize) -> Option<(String, Span)> {
    let program = output.program.as_ref()?;
    let expr = find_expr_at(program, file_id, offset)?;
    let ty = output.expr_types.get(&expr.id)?;
    Some((ty.to_string(), expr.span))
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
