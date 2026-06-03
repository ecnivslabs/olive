//! Roadmap E13.4: `///` above a `fn`/`struct`/`enum`/module binds
//! documentation to it. Comments never enter the token stream (the lexer
//! collects them on the side as trivia, see `lexer::Comment`), and every
//! item's `Span` already carries a source line -- so matching a doc comment
//! to the item below it is a plain line-number scan over the raw source
//! text, not a second pass through the lexer's comment list or a new AST
//! field. Simpler given the trivia model, and exactly what `pit doc`
//! (`commands::doc`) and hover (`tooling::lsp::hover`) both need.

/// The doc comment immediately above `item_line` (1-indexed, matching
/// `Span::line`): consecutive `///` lines, decorator-tolerant (a
/// `#[test]`/`@memo` between the comment and the item doesn't break the
/// association), reversed back into source order and joined with `\n`.
/// `None` if the line directly above (skipping decorators) isn't a `///`
/// line.
pub fn extract_for_item(source_lines: &[&str], item_line: usize) -> Option<String> {
    if item_line < 2 {
        return None;
    }
    let mut doc_lines = Vec::new();
    let mut i = item_line - 2; // 0-indexed line directly above item_line
    loop {
        let Some(line) = source_lines.get(i) else {
            break;
        };
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("///") {
            doc_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else if !(trimmed.starts_with('#') || trimmed.starts_with('@')) {
            break;
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    if doc_lines.is_empty() {
        return None;
    }
    doc_lines.reverse();
    Some(doc_lines.join("\n"))
}

/// A doc comment at the very top of the file, used as the module's own
/// description when a blank line separates it from whatever comes next --
/// otherwise it reads as the first item's own doc (`extract_for_item`
/// already covers that case), not the module's.
pub fn extract_module_doc(source_lines: &[&str]) -> Option<String> {
    let mut doc_lines = Vec::new();
    let mut i = 0;
    while let Some(line) = source_lines.get(i) {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("///") else {
            break;
        };
        doc_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        i += 1;
    }
    if doc_lines.is_empty() {
        return None;
    }
    if source_lines.get(i).is_some_and(|l| !l.trim().is_empty()) {
        return None;
    }
    Some(doc_lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<&str> {
        s.lines().collect()
    }

    #[test]
    fn extracts_single_line_doc() {
        let src =
            lines("/// Adds two numbers.\nfn add(a: int, b: int) -> int:\n    return a + b\n");
        assert_eq!(
            extract_for_item(&src, 2),
            Some("Adds two numbers.".to_string())
        );
    }

    #[test]
    fn extracts_multi_line_doc_in_order() {
        let src = lines("/// Line one.\n/// Line two.\nfn f():\n    pass\n");
        assert_eq!(
            extract_for_item(&src, 3),
            Some("Line one.\nLine two.".to_string())
        );
    }

    #[test]
    fn skips_decorator_between_doc_and_item() {
        let src = lines("/// Cached.\n#[test]\nfn f():\n    pass\n");
        assert_eq!(extract_for_item(&src, 3), Some("Cached.".to_string()));
    }

    #[test]
    fn no_doc_when_blank_line_separates() {
        let src = lines("/// Orphaned.\n\nfn f():\n    pass\n");
        assert_eq!(extract_for_item(&src, 3), None);
    }

    #[test]
    fn no_doc_when_plain_comment() {
        let src = lines("// not a doc comment\nfn f():\n    pass\n");
        assert_eq!(extract_for_item(&src, 2), None);
    }

    #[test]
    fn item_on_first_line_has_no_doc() {
        let src = lines("fn f():\n    pass\n");
        assert_eq!(extract_for_item(&src, 1), None);
    }

    #[test]
    fn module_doc_needs_blank_line_before_first_item() {
        let src = lines("/// Module description.\n\nfn f():\n    pass\n");
        assert_eq!(
            extract_module_doc(&src),
            Some("Module description.".to_string())
        );
    }

    #[test]
    fn no_module_doc_when_it_belongs_to_first_item() {
        let src = lines("/// This is f's doc, not the module's.\nfn f():\n    pass\n");
        assert_eq!(extract_module_doc(&src), None);
    }
}
