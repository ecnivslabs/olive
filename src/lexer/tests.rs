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

    fn first_number_value(src: &str) -> String {
        Lexer::new(src, 0)
            .tokenise()
            .unwrap()
            .into_iter()
            .find(|t| matches!(t.kind, TokenKind::Integer | TokenKind::Float))
            .map(|t| t.value)
            .unwrap()
    }

    #[test]
    fn integer_underscore_separators_are_stripped() {
        assert_eq!(first_number_value("1_000_000\n"), "1000000");
    }

    #[test]
    fn float_underscore_separators_are_stripped() {
        assert_eq!(first_number_value("1_234.567_8\n"), "1234.5678");
    }

    #[test]
    fn hex_octal_binary_underscore_separators_are_stripped() {
        assert_eq!(first_number_value("0xFF_FF\n"), "0xFFFF");
        assert_eq!(first_number_value("0o17_17\n"), "0o1717");
        assert_eq!(first_number_value("0b1010_1010\n"), "0b10101010");
    }

    #[test]
    fn exponent_underscore_separators_are_stripped() {
        assert_eq!(first_number_value("1_0e1_0\n"), "10e10");
    }

    #[test]
    fn underscore_span_covers_original_source_length() {
        // Span math must stay char-correct so fmt can re-slice the original
        // lexeme (including underscores) rather than the cleaned token value.
        let toks = Lexer::new("1_000_000\n", 0).tokenise().unwrap();
        let tok = toks.iter().find(|t| t.kind == TokenKind::Integer).unwrap();
        assert_eq!(tok.span, (0, 9));
    }

    #[test]
    fn leading_underscore_is_not_consumed_into_literal() {
        // `_123` lexes as an identifier, not an integer with a leading separator.
        let kinds = tokenise_kinds("_123\n");
        assert!(!kinds.contains(&TokenKind::Integer));
        assert!(kinds.contains(&TokenKind::Identifier));
    }

    #[test]
    fn trailing_underscore_is_not_consumed_into_literal() {
        assert_eq!(first_number_value("123_\n"), "123");
    }

    #[test]
    fn doubled_underscore_stops_the_literal() {
        assert_eq!(first_number_value("1__000\n"), "1");
    }

    #[test]
    fn underscore_adjacent_to_dot_stops_the_fraction() {
        // `1_.5`: trailing `_` right before `.` is not consumed.
        assert_eq!(first_number_value("1_.5\n"), "1");
    }
}
