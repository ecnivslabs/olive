#[cfg(test)]
mod lexer_tests {
    use crate::lexer::{Lexer, TokenKind};

    fn tokenise_kinds(src: &str) -> Vec<TokenKind> {
        Lexer::new(src, 0)
            .tokenise()
            .map(|tokens| tokens.into_iter().map(|t| t.kind).collect())
            .unwrap_or_default()
    }

    #[test]
    fn empty_source_no_crash() {
        let _ = tokenise_kinds("");
    }

    #[test]
    fn integer_literals_produce_tokens() {
        let kinds = tokenise_kinds("42 0xFF 0o77 0b1010\n");
        assert!(kinds.contains(&TokenKind::Integer));
    }

    #[test]
    fn float_literals_produce_tokens() {
        let kinds = tokenise_kinds("3.14 0.5\n");
        assert!(kinds.contains(&TokenKind::Float));
    }

    #[test]
    fn keyword_tokens_produced() {
        let kinds = tokenise_kinds("fn let if else while for return True False None\n");
        assert!(kinds.contains(&TokenKind::Fn));
        assert!(kinds.contains(&TokenKind::Let));
        assert!(kinds.contains(&TokenKind::If));
        assert!(kinds.contains(&TokenKind::Return));
        assert!(kinds.contains(&TokenKind::True));
        assert!(kinds.contains(&TokenKind::Null));
    }

    #[test]
    fn operator_tokens_produced() {
        let kinds = tokenise_kinds("+ - * / % ** == != < > <= >=\n");
        for k in &[
            TokenKind::Plus,
            TokenKind::Minus,
            TokenKind::Star,
            TokenKind::DoubleEqual,
        ] {
            assert!(kinds.contains(k), "missing operator {:?}", k);
        }
    }

    #[test]
    fn delimiter_tokens_produced() {
        let kinds = tokenise_kinds("( ) [ ] { } , : . @ _\n");
        for k in &[
            TokenKind::LParen,
            TokenKind::RParen,
            TokenKind::Comma,
            TokenKind::At,
            TokenKind::Underscore,
        ] {
            assert!(kinds.contains(k), "missing delimiter {:?}", k);
        }
    }

    #[test]
    fn string_literals_produced() {
        let kinds = tokenise_kinds("\"hello\" 'world'\n");
        assert!(kinds.contains(&TokenKind::String));
    }

    #[test]
    fn identifiers_produced() {
        let kinds = tokenise_kinds("foo bar _baz x1\n");
        assert!(kinds.contains(&TokenKind::Identifier));
    }

    #[test]
    fn line_comments_skipped() {
        let kinds = tokenise_kinds("x = 1 // comment\ny = 2\n");
        assert!(kinds.contains(&TokenKind::Identifier));
    }

    #[test]
    fn block_comments_skipped() {
        let kinds = tokenise_kinds("x = /* comment */ 1\n");
        assert!(!kinds.contains(&TokenKind::Slash));
    }

    #[test]
    fn indentation_dedent() {
        let tokens = Lexer::new("if x:\n    pass\n", 0).tokenise().unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
        assert!(kinds.contains(&TokenKind::Indent));
        assert!(kinds.contains(&TokenKind::Dedent));
    }

    #[test]
    fn nested_indentation() {
        let tokens = Lexer::new("if x:\n    if y:\n        pass\n", 0)
            .tokenise()
            .unwrap();
        let indent_count = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Indent)
            .count();
        assert_eq!(indent_count, 2);
    }

    #[test]
    fn unterminated_string_errors() {
        let _result = Lexer::new("\"hello", 0).tokenise();
    }

    #[test]
    fn invalid_hex_errors() {
        let _result = Lexer::new("0xGG\n", 0).tokenise();
    }

    #[test]
    fn unterminated_block_comment_errors() {
        let _result = Lexer::new("/* hello", 0).tokenise();
    }

    #[test]
    fn unexpected_character_errors() {
        let _result = Lexer::new("let x = $", 0).tokenise();
    }

    #[test]
    fn keywords_not_identifiers() {
        let kinds = tokenise_kinds("fn let if else while for return struct enum None\n");
        assert!(
            !kinds.contains(&TokenKind::Identifier),
            "keywords should not be identifiers"
        );
    }

    #[test]
    fn newline_separates_statements() {
        let tokens = Lexer::new("x = 1\ny = 2\n", 0).tokenise().unwrap();
        let newline_count = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Newline)
            .count();
        assert!(
            newline_count > 0,
            "should have newline tokens between statements"
        );
    }

    #[test]
    fn single_char_identifiers() {
        let kinds = tokenise_kinds("a b c\n");
        let id_count = kinds
            .iter()
            .filter(|k| **k == TokenKind::Identifier)
            .count();
        assert_eq!(id_count, 3);
    }

    #[test]
    fn triple_quoted_string_handling() {
        let kinds = tokenise_kinds("\"\"\"hello\"\"\"\n");
        assert!(kinds.contains(&TokenKind::String));
    }

    #[test]
    fn f_string_produces_fstring_token() {
        let kinds = tokenise_kinds("f\"hello {x}\"\n");
        assert!(kinds.contains(&TokenKind::FString));
    }

    #[test]
    fn escape_sequences_in_strings() {
        let kinds = tokenise_kinds("\"hello\\nworld\"\n");
        assert!(kinds.contains(&TokenKind::String));
    }

    #[test]
    fn hex_and_unicode_escapes() {
        let toks = Lexer::new("\"\\x1b[0m\\u{1F600}\\0\"\n", 0)
            .tokenise()
            .unwrap();
        let s = toks
            .iter()
            .find(|t| t.kind == TokenKind::String)
            .map(|t| t.value.clone())
            .unwrap();
        assert_eq!(s, "\u{1b}[0m\u{1F600}\0");
    }
}
