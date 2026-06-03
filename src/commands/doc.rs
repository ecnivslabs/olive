use crate::compile::diagnose::diagnose;
use crate::parser::{Param, Stmt, StmtKind};
use crate::tooling::doc_blocks::{extract_blocks, is_elided_context_error};
use crate::tooling::doc_comments::{extract_for_item, extract_module_doc};
use std::path::Path;

/// `pit doc [file]`: renders one module's public signatures and `///`
/// doc comments as markdown into `target/doc/<module>.md`. No file given
/// defaults to the current project's pod entry, matching `pit fmt`.
pub fn execute_doc(file: Option<&str>) {
    let target = match file {
        Some(f) => f.to_string(),
        None => match super::utils::load_config().pod {
            Some(pod) => pod.entry,
            None => {
                eprintln!("error: no file given and no pod defined in pit.toml");
                std::process::exit(1);
            }
        },
    };

    let out = diagnose(&target);
    let errors: Vec<_> = out.diagnostics.iter().filter(|d| d.is_error()).collect();
    if !errors.is_empty() {
        eprintln!("error: `{target}` has compile errors; fix them before generating docs\n");
        for d in &errors {
            d.emit(&out.sources);
        }
        std::process::exit(1);
    }
    let Some(program) = &out.program else {
        eprintln!("error: could not parse `{target}`");
        std::process::exit(1);
    };

    let source = std::fs::read_to_string(&target).unwrap_or_else(|e| {
        eprintln!("error: could not read `{target}`: {e}");
        std::process::exit(1);
    });
    let lines: Vec<&str> = source.lines().collect();

    let module_name = Path::new(&target)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module")
        .to_string();

    let md = render_module(&module_name, program, &lines);
    check_doc_examples(&md, &module_name);

    let out_dir = Path::new("target/doc");
    std::fs::create_dir_all(out_dir).unwrap_or_else(|e| {
        eprintln!("error: could not create {}: {e}", out_dir.display());
        std::process::exit(1);
    });
    let out_path = out_dir.join(format!("{module_name}.md"));
    std::fs::write(&out_path, &md).unwrap_or_else(|e| {
        eprintln!("error: could not write {}: {e}", out_path.display());
        std::process::exit(1);
    });
    println!(
        "\x1b[1;32m    Docs\x1b[0m written to {}",
        out_path.display()
    );
}

/// A leading `_` is Olive's own privacy marker (`resolver::define_sym`); an
/// internal helper has no business in a module's public docs.
fn is_public(name: &str) -> bool {
    !name.starts_with('_')
}

fn render_module(module_name: &str, program: &crate::parser::Program, lines: &[&str]) -> String {
    let mut md = String::new();
    md.push_str(&format!("# {module_name}\n\n"));
    if let Some(doc) = extract_module_doc(lines) {
        md.push_str(&doc);
        md.push_str("\n\n");
    }

    let mut fns = Vec::new();
    let mut structs = Vec::new();
    let mut enums = Vec::new();
    for stmt in &program.stmts {
        match &stmt.kind {
            StmtKind::Fn { name, .. } if is_public(name) => fns.push(stmt),
            StmtKind::Struct { name, .. } if is_public(name) => structs.push(stmt),
            StmtKind::Enum { name, .. } if is_public(name) => enums.push(stmt),
            _ => {}
        }
    }

    if !fns.is_empty() {
        md.push_str("## Functions\n\n");
        for stmt in &fns {
            render_fn(&mut md, stmt, lines);
        }
    }
    if !structs.is_empty() {
        md.push_str("## Structs\n\n");
        for stmt in &structs {
            render_struct(&mut md, stmt, lines);
        }
    }
    if !enums.is_empty() {
        md.push_str("## Enums\n\n");
        for stmt in &enums {
            render_enum(&mut md, stmt, lines);
        }
    }
    md
}

/// Roadmap E13.4: a fenced Olive block inside a doc comment compiles under
/// the same machinery E10.3 already uses for `docs/*.md` (`tooling::
/// doc_blocks`, shared with `tests/doc_blocks.rs`) -- typecheck only, not a
/// full JIT run: a `pit doc` invocation renders documentation, it doesn't
/// execute arbitrary snippets embedded in it as a side effect. A broken
/// example is a warning, not a hard failure -- one bad snippet in a doc
/// comment shouldn't block docs for every other item in the module.
fn check_doc_examples(md: &str, module_name: &str) {
    for (i, block) in extract_blocks(md)
        .into_iter()
        .filter(|b| b.lang == "olive" || b.lang == "rust")
        .enumerate()
    {
        if block.code.trim().is_empty() {
            continue;
        }
        let tmp = std::env::temp_dir().join(format!(
            "olive_doc_check_{}_{module_name}_{i}.liv",
            std::process::id()
        ));
        if std::fs::write(&tmp, &block.code).is_err() {
            continue;
        }
        let out = diagnose(tmp.to_str().unwrap());
        let errors: Vec<_> = out.diagnostics.iter().filter(|d| d.is_error()).collect();
        if !errors.is_empty() {
            let fake_stderr = errors
                .iter()
                .map(|d| format!("[{}]", d.code().unwrap_or("")))
                .collect::<Vec<_>>()
                .join("\n");
            if !is_elided_context_error(&block.code, &fake_stderr) {
                let headlines = errors
                    .iter()
                    .map(|d| d.headline().to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                eprintln!(
                    "warning: doc example #{i} in `{module_name}` does not compile: {headlines}"
                );
            }
        }
        std::fs::remove_file(&tmp).ok();
    }
}

fn format_param(p: &Param) -> String {
    let ty = match &p.type_ann {
        Some(t) => format!(": {t}"),
        None => String::new(),
    };
    let default = if p.default.is_some() { " = .." } else { "" };
    format!("{}{ty}{default}", p.name)
}

fn push_doc_or_placeholder(md: &mut String, stmt: &Stmt, lines: &[&str]) {
    match extract_for_item(lines, stmt.span.line) {
        Some(doc) => {
            md.push_str(&doc);
            md.push_str("\n\n");
        }
        None => md.push_str("_undocumented_\n\n"),
    }
}

fn render_fn(md: &mut String, stmt: &Stmt, lines: &[&str]) {
    let StmtKind::Fn {
        name,
        params,
        return_type,
        is_async,
        ..
    } = &stmt.kind
    else {
        return;
    };
    let params_str = params
        .iter()
        .map(format_param)
        .collect::<Vec<_>>()
        .join(", ");
    let ret_str = match return_type {
        Some(t) => format!(" -> {t}"),
        None => String::new(),
    };
    let async_prefix = if *is_async { "async " } else { "" };
    md.push_str(&format!(
        "### `{async_prefix}fn {name}({params_str}){ret_str}`\n\n"
    ));
    push_doc_or_placeholder(md, stmt, lines);
}

fn render_struct(md: &mut String, stmt: &Stmt, lines: &[&str]) {
    let StmtKind::Struct { name, fields, .. } = &stmt.kind else {
        return;
    };
    md.push_str(&format!("### `struct {name}`\n\n"));
    push_doc_or_placeholder(md, stmt, lines);
    if !fields.is_empty() {
        md.push_str("**Fields:**\n\n");
        for f in fields {
            md.push_str(&format!("- `{}`\n", format_param(f)));
        }
        md.push('\n');
    }
}

fn render_enum(md: &mut String, stmt: &Stmt, lines: &[&str]) {
    let StmtKind::Enum { name, variants, .. } = &stmt.kind else {
        return;
    };
    md.push_str(&format!("### `enum {name}`\n\n"));
    push_doc_or_placeholder(md, stmt, lines);
    if !variants.is_empty() {
        md.push_str("**Variants:**\n\n");
        for v in variants {
            if v.types.is_empty() {
                md.push_str(&format!("- `{}`\n", v.name));
            } else {
                let types = v
                    .types
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                md.push_str(&format!("- `{}({types})`\n", v.name));
            }
        }
        md.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn parse(src: &str) -> crate::parser::Program {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        Parser::new(tokens).parse_program().unwrap()
    }

    #[test]
    fn renders_documented_function_signature() {
        let src = "/// Adds two numbers.\nfn add(a: int, b: int) -> int:\n    return a + b\n";
        let program = parse(src);
        let lines: Vec<&str> = src.lines().collect();
        let md = render_module("math", &program, &lines);
        assert!(md.contains("# math"));
        assert!(md.contains("### `fn add(a: int, b: int) -> int`"));
        assert!(md.contains("Adds two numbers."));
    }

    #[test]
    fn undocumented_function_gets_placeholder() {
        let src = "fn add(a: int, b: int) -> int:\n    return a + b\n";
        let program = parse(src);
        let lines: Vec<&str> = src.lines().collect();
        let md = render_module("math", &program, &lines);
        assert!(md.contains("_undocumented_"));
    }

    #[test]
    fn private_function_is_excluded() {
        let src = "fn _helper() -> int:\n    return 1\n\nfn add() -> int:\n    return 2\n";
        let program = parse(src);
        let lines: Vec<&str> = src.lines().collect();
        let md = render_module("math", &program, &lines);
        assert!(!md.contains("_helper"));
        assert!(md.contains("fn add()"));
    }

    #[test]
    fn renders_struct_fields_and_doc() {
        let src = "/// A point in space.\nstruct Point:\n    x: float\n    y: float\n";
        let program = parse(src);
        let lines: Vec<&str> = src.lines().collect();
        let md = render_module("geom", &program, &lines);
        assert!(md.contains("### `struct Point`"));
        assert!(md.contains("A point in space."));
        assert!(md.contains("- `x: float`"));
        assert!(md.contains("- `y: float`"));
    }

    #[test]
    fn renders_enum_variants_with_payloads() {
        let src = "enum Shape:\n    Circle(float)\n    Square\n";
        let program = parse(src);
        let lines: Vec<&str> = src.lines().collect();
        let md = render_module("geom", &program, &lines);
        assert!(md.contains("### `enum Shape`"));
        assert!(md.contains("- `Circle(float)`"));
        assert!(md.contains("- `Square`"));
    }

    #[test]
    fn module_doc_appears_before_functions() {
        let src = "/// Math helpers.\n\nfn add() -> int:\n    return 1\n";
        let program = parse(src);
        let lines: Vec<&str> = src.lines().collect();
        let md = render_module("math", &program, &lines);
        let module_doc_pos = md.find("Math helpers.").unwrap();
        let fn_pos = md.find("### `fn add").unwrap();
        assert!(module_doc_pos < fn_pos);
    }
}
