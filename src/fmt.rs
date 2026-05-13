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

    for tok in tokens {
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
                continue;
            }
            TokenKind::Eof => break,
            _ => {
                if at_start_of_line {
                    formatted.push_str(&"    ".repeat(indent_level));
                    at_start_of_line = false;
                } else {
                    match tok.kind {
                        TokenKind::LParen
                        | TokenKind::LBracket
                        | TokenKind::LBrace
                        | TokenKind::Colon
                        | TokenKind::Comma
                        | TokenKind::RParen
                        | TokenKind::RBracket
                        | TokenKind::RBrace
                        | TokenKind::Dot => {}
                        _ => {
                            if !matches!(
                                last_kind,
                                TokenKind::LParen
                                    | TokenKind::LBracket
                                    | TokenKind::LBrace
                                    | TokenKind::Dot
                                    | TokenKind::At
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

                last_kind = tok.kind;
            }
        }
    }

    fs::write(filename, formatted).unwrap();
    println!("\x1b[1;32mFormatted\x1b[0m {}", filename);
}

pub fn walk_and_format(path: &Path) {
    if path.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            walk_and_format(&entry.path());
        }
    } else if path.extension().is_some_and(|ext| ext == "liv") {
        format_file(path.to_str().unwrap());
    }
}
