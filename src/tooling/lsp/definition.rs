//! `textDocument/definition`, via `Resolver::def_sites` (SymbolTable is transient).

use super::locate::find_expr_at;
use crate::compile::diagnose::DiagnoseOutput;
use crate::span::Span;

/// Definition span of the name at `offset`, or `None` if unresolved.
pub fn definition_at(output: &DiagnoseOutput, file_id: usize, offset: usize) -> Option<Span> {
    let program = output.program.as_ref()?;
    let expr = find_expr_at(program, file_id, offset)?;
    output.def_sites.get(&expr.id).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::diagnose::diagnose;
    use std::io::Write;

    fn write_temp(name: &str, content: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("olive_definition_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn definition_of_function_call_points_at_its_fn_stmt() {
        let src = "fn helper() -> int:\n    return 1\n\nfn main():\n    print(helper())\n";
        let path = write_temp("def_fn.liv", src);
        let out = diagnose(path.to_str().unwrap());
        let file_id = out
            .sources
            .iter()
            .find(|(_, (p, _))| p == path.to_str().unwrap())
            .map(|(id, _)| *id)
            .unwrap();
        let call_offset = src.rfind("helper()").unwrap() + 1;
        let def_span = definition_at(&out, file_id, call_offset).expect("definition found");
        let fn_decl_offset = src.find("fn helper").unwrap();
        assert!(
            def_span.start <= fn_decl_offset && fn_decl_offset <= def_span.end,
            "definition span {:?} should cover the `fn helper` declaration at {}",
            def_span,
            fn_decl_offset
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn definition_on_undefined_name_is_none() {
        let src = "print(nope)\n";
        let path = write_temp("def_none.liv", src);
        let out = diagnose(path.to_str().unwrap());
        let file_id = out
            .sources
            .iter()
            .find(|(_, (p, _))| p == path.to_str().unwrap())
            .map(|(id, _)| *id)
            .unwrap();
        let offset = src.find("nope").unwrap() + 1;
        assert!(definition_at(&out, file_id, offset).is_none());
        std::fs::remove_file(&path).ok();
    }
}
