use crate::lexer::{Lexer, TokenKind};
use std::{fs, path::Path};

pub fn format_file(filename: &str) {
    let source = match fs::read_to_string(filename) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error reading {}: {}", filename, e);
            return;
        }
    };
    let mut lexer = Lexer::new(&source, 0);
    let tokens = match lexer.tokenise() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error formatting {}: {}", filename, e.message);
            return;
        }
    };

    let mut formatted = String::new();
    let mut indent_level = 0;
    let mut at_start_of_line = true;
    let mut last_kind = TokenKind::Eof;
    let mut needs_blank_check = true;
    let mut prev_line_first_kind: Option<TokenKind> = None;

    for (i, tok) in tokens.iter().enumerate() {
        match tok.kind {
            TokenKind::Indent => {
                indent_level += 1;
                continue;
            }
            TokenKind::Dedent => {
                indent_level -= 1;
                continue;
            }
            TokenKind::Newline => {
                formatted.push('\n');
                at_start_of_line = true;
                last_kind = TokenKind::Newline;
                needs_blank_check = true;
                continue;
            }
            TokenKind::Eof => break,
            _ => {
                if at_start_of_line {
                    if needs_blank_check && i > 0 {
                        match tok.kind {
                            TokenKind::Struct
                            | TokenKind::Impl
                            | TokenKind::Fn
                            | TokenKind::Const
                            | TokenKind::Enum
                            | TokenKind::Trait
                                if indent_level <= 1 =>
                            {
                                let skip = matches!(
                                    (&prev_line_first_kind, &tok.kind),
                                    (Some(TokenKind::Const), TokenKind::Const)
                                );
                                if !skip {
                                    formatted.push('\n');
                                }
                            }
                            _ => {}
                        }
                    }
                    needs_blank_check = false;

                    formatted.push_str(&"    ".repeat(indent_level));
                    at_start_of_line = false;
                    prev_line_first_kind = Some(tok.kind.clone());
                } else {
                    match tok.kind {
                        TokenKind::RParen
                        | TokenKind::RBracket
                        | TokenKind::RBrace
                        | TokenKind::Colon
                        | TokenKind::Comma
                        | TokenKind::Dot => {}
                        TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                            if !matches!(
                                last_kind,
                                TokenKind::LParen
                                    | TokenKind::LBracket
                                    | TokenKind::LBrace
                                    | TokenKind::Dot
                                    | TokenKind::At
                                    | TokenKind::Identifier
                                    | TokenKind::RParen
                                    | TokenKind::RBracket
                                    | TokenKind::RBrace
                            ) {
                                formatted.push(' ');
                            }
                        }
                        _ => {
                            if !matches!(
                                last_kind,
                                TokenKind::LParen
                                    | TokenKind::LBracket
                                    | TokenKind::LBrace
                                    | TokenKind::Dot
                                    | TokenKind::At
                                    | TokenKind::Ampersand
                            ) {
                                formatted.push(' ');
                            }
                        }
                    }
                }

                match tok.kind {
                    TokenKind::String => {
                        formatted.push('"');
                        formatted.push_str(&tok.value);
                        formatted.push('"');
                    }
                    TokenKind::FString => {
                        formatted.push('f');
                        formatted.push('"');
                        formatted.push_str(&tok.value);
                        formatted.push('"');
                    }
                    _ => formatted.push_str(&tok.value),
                }

                last_kind = tok.kind.clone();
            }
        }
    }

    if let Err(e) = fs::write(filename, formatted) {
        eprintln!("error writing {}: {}", filename, e);
        return;
    }
    println!("\x1b[1;32mFormatted\x1b[0m {}", filename);
}

pub fn walk_and_format(path: &Path) {
    if path.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                walk_and_format(&entry.path());
            }
        }
    } else if path.extension().is_some_and(|ext| ext == "liv")
        && let Some(s) = path.to_str()
    {
        format_file(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn format_str(source: &str) -> String {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let filepath =
            std::env::temp_dir().join(format!("olive_fmt_{}__{}.liv", std::process::id(), id));
        let path = filepath.to_str().unwrap().to_string();

        let mut f = fs::File::create(&filepath).unwrap();
        f.write_all(source.as_bytes()).unwrap();

        format_file(&path);
        let result = fs::read_to_string(&filepath).unwrap();
        let _ = fs::remove_file(&filepath);
        result
    }

    fn assert_formatted_eq(input: &str, expected: &str) {
        let result = format_str(input);
        assert_eq!(
            result, expected,
            "format mismatch for:\n--- input ---\n{input}\n--- expected ---\n{expected}\n--- got ---\n{result}"
        );
    }

    #[test]
    fn empty_file_no_crash() {
        let result = format_str("");
        assert_eq!(result, "");
    }

    #[test]
    fn single_integer_expr() {
        assert_formatted_eq("42", "42");
    }

    #[test]
    fn let_binding() {
        let src = "let x=42";
        let expected = "let x = 42";
        assert_formatted_eq(src, expected);
    }

    #[test]
    fn binary_op_spacing() {
        assert_formatted_eq("1+2*3", "1 + 2 * 3");
    }

    #[test]
    fn call_spacing() {
        assert_formatted_eq("f(1,2,3)", "f(1, 2, 3)");
    }

    #[test]
    fn string_content_preserved() {
        let src = "let s = \"hello world\"";
        let expected = "let s = \"hello world\"";
        assert_formatted_eq(src, expected);
    }

    #[test]
    fn list_literal_spacing() {
        assert_formatted_eq("[1,2,3]", "[1, 2, 3]");
    }

    #[test]
    fn function_def() {
        let src = "fn add(a:i64,b:i64)->i64:\n    return a+b";
        let expected = "fn add(a: i64, b: i64) -> i64:\n    return a + b\n";
        assert_formatted_eq(src, expected);
    }

    #[test]
    fn if_else_structure() {
        let src = "fn f(x:i64)->i64:\n    if x>0:\n        return x\n    else:\n        return 0";
        let expected =
            "fn f(x: i64) -> i64:\n    if x > 0:\n        return x\n    else:\n        return 0\n";
        assert_formatted_eq(src, expected);
    }

    #[test]
    fn nested_blocks() {
        let src =
            "fn f(n:i64)->i64:\n    if n>0:\n        if n>10:\n            return n\n    return 0";
        let expected = "fn f(n: i64) -> i64:\n    if n > 0:\n        if n > 10:\n            return n\n    return 0\n";
        assert_formatted_eq(src, expected);
    }

    #[test]
    fn struct_definition() {
        let src = "struct Point:\n    x:i64\n    y:i64";
        let expected = "struct Point:\n    x: i64\n    y: i64\n";
        assert_formatted_eq(src, expected);
    }

    #[test]
    fn blank_lines_between_decls() {
        let src = "fn a() -> i64:\n    return 1\nfn b() -> i64:\n    return 2";
        let result = format_str(src);
        assert!(
            result.contains("return 1\n\nfn b"),
            "expected blank line between fn decls, got: {result:?}"
        );
    }

    #[test]
    fn while_loop_format() {
        let src = "let mut i=0\nwhile i<10:\n    i=i+1";
        let expected = "let mut i = 0\nwhile i < 10:\n    i = i + 1\n";
        assert_formatted_eq(src, expected);
    }

    #[test]
    fn walk_and_format_no_panic() {
        let dir = std::env::temp_dir().join(format!(
            "olive_fmt_walk_{}",
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::create_dir_all(&dir);
        let filepath = dir.join("test.liv");
        let mut f = fs::File::create(&filepath).unwrap();
        f.write_all(b"let x = 1\n").unwrap();
        walk_and_format(&dir);
        let content = fs::read_to_string(&filepath).unwrap();
        assert!(!content.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
