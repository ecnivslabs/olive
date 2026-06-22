#[cfg(test)]
mod parser_proptests {
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use proptest::prelude::*;

    fn lex_and_parse(input: &str) {
        if let Ok(tokens) = Lexer::new(input, 0).tokenise() {
            let _ = Parser::new(tokens).parse_program();
        }
    }

    proptest! {
        #[test]
        fn no_panic_on_arbitrary_string(s in "\\PC{0,200}") {
            lex_and_parse(&s);
        }

        #[test]
        fn no_panic_on_many_newlines(n in 0..50usize) {
            let s = "\n".repeat(n);
            lex_and_parse(&s);
        }

        #[test]
        fn no_panic_on_numeric_edge_cases(s in "[-+]?[0-9]{0,20}") {
            let src = format!("{s}\n");
            lex_and_parse(&src);
        }

        #[test]
        fn no_panic_on_deeply_nested_parens(depth in 0..10usize) {
            let mut s = String::new();
            for _ in 0..depth {
                s.push('(');
            }
            s.push('0');
            for _ in 0..depth {
                s.push(')');
            }
            s.push('\n');
            lex_and_parse(&s);
        }

        #[test]
        fn no_panic_on_deeply_nested_braces(depth in 0..10usize) {
            let mut s = "fn f() -> i64:\n    return ".to_string();
            for _ in 0..depth {
                s.push('[');
            }
            s.push('0');
            for _ in 0..depth {
                s.push(']');
            }
            s.push('\n');
            lex_and_parse(&s);
        }

        #[test]
        fn no_panic_on_whitespace_variations(s in "\\s{0,200}") {
            lex_and_parse(&s);
        }

        #[test]
        fn no_panic_on_repeated_keywords(kw in prop_oneof![
            Just("fn"), Just("let"), Just("if"), Just("else"),
            Just("while"), Just("for"), Just("return"), Just("struct"),
            Just("enum"), Just("impl"), Just("trait"), Just("match"),
            Just("true"), Just("false"), Just("None"), Just("pass"),
        ].prop_map(|s| s.to_string())) {
            let src = format!("{kw}\n");
            lex_and_parse(&src);
        }

        #[test]
        fn no_panic_on_ascii_control_bytes(s in proptest::collection::vec(0u8..128u8, 0..100)) {
            let s = String::from_utf8_lossy(&s).into_owned();
            lex_and_parse(&s);
        }

        #[test]
        fn no_panic_on_identifier_formats(prefix in proptest::option::of("[a-zA-Z_]"),
                                          body in "[a-zA-Z0-9_]{0,20}") {
            let s = match prefix {
                Some(p) => format!("{p}{body}\n"),
                None => format!("{body}\n"),
            };
            lex_and_parse(&s);
        }
    }
}
