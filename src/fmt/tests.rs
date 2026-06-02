use super::format_source;
use crate::lexer::Lexer;
use crate::parser::{Parser, Program};

const W: usize = 100;

fn fmt(src: &str) -> String {
    format_source(src, W).expect("format failed")
}

fn parse(src: &str) -> Program {
    let tokens = Lexer::new(src, 0).tokenise().expect("lex failed");
    Parser::new(tokens).parse_program().expect("parse failed")
}

/// A debug fingerprint of the AST with spans and node ids removed, so two programs
/// compare equal exactly when they have the same structure regardless of layout.
/// This is the guarantee a formatter must uphold: it may move bytes, never meaning.
fn canonical(src: &str) -> String {
    strip_ids(&strip_spans(&format!("{:?}", parse(src))))
}

fn strip_spans(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if s[i..].starts_with("Span {") {
            out.push_str("Span");
            i += "Span {".len();
            while i < bytes.len() && bytes[i] != b'}' {
                i += 1;
            }
            i += 1; // skip '}'
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn strip_ids(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find("id: ") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 4..];
        let digits_end = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        let mut tail = &after[digits_end..];
        if let Some(stripped) = tail.strip_prefix(", ") {
            tail = stripped;
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

fn comment_count(src: &str) -> usize {
    let mut lex = Lexer::new(src, 0);
    lex.tokenise().unwrap();
    lex.comments().len()
}

#[test]
fn empty_file() {
    assert_eq!(fmt(""), "");
    assert_eq!(fmt("   \n\n"), "");
}

#[test]
fn binary_op_spacing() {
    assert_eq!(fmt("x = 1+2*3"), "x = 1 + 2 * 3\n");
}

#[test]
fn unary_minus_has_no_space() {
    assert_eq!(fmt("x = -5"), "x = -5\n");
    assert_eq!(fmt("y = -a + b"), "y = -a + b\n");
}

#[test]
fn call_and_list_spacing() {
    assert_eq!(fmt("f(1,2,3)"), "f(1, 2, 3)\n");
    assert_eq!(fmt("x = [1,2,3]"), "x = [1, 2, 3]\n");
}

#[test]
fn precedence_parens_preserved() {
    assert_eq!(fmt("x = (1 + 2) * 3"), "x = (1 + 2) * 3\n");
    assert_eq!(fmt("x = 1 + 2 * 3"), "x = 1 + 2 * 3\n");
}

#[test]
fn redundant_parens_dropped() {
    assert_eq!(fmt("x = (((5)))"), "x = 5\n");
}

#[test]
fn line_comment_preserved() {
    let out = fmt("// header\nx = 1\n");
    assert!(out.contains("// header"));
    assert!(out.contains("x = 1"));
}

#[test]
fn trailing_comment_preserved() {
    let out = fmt("x = 1  // note\n");
    assert!(out.contains("x = 1"));
    assert!(out.contains("// note"));
}

#[test]
fn block_comment_preserved() {
    let out = fmt("/* a */\nx = 1\n");
    assert!(out.contains("/* a */"));
}

#[test]
fn function_reflows_long_call() {
    let src = "fn main():\n    foo(aaaaaaaaaaaa, bbbbbbbbbbbb, cccccccccccc, dddddddddddd, eeeeeeeeeeee, ffffffffffff, gggggggggggg, hhhhhhhhhhhh)\n";
    let out = fmt(src);
    assert!(out.contains("foo(\n"), "expected broken call, got:\n{out}");
    assert!(
        out.contains(",\n    )") || out.contains(",\n)"),
        "trailing comma on break"
    );
}

#[test]
fn short_call_stays_flat() {
    let out = fmt("fn main():\n    foo(a, b, c)\n");
    assert!(out.contains("foo(a, b, c)"));
}

#[test]
fn blank_lines_collapse_to_one() {
    let out = fmt("x = 1\n\n\n\ny = 2\n");
    assert_eq!(out, "x = 1\n\ny = 2\n");
}

#[test]
fn string_escapes_round_trip() {
    let src = "s = \"a\\tb\\nc\"\n";
    assert_eq!(fmt(src), src);
    // The escape sequence text survives, not the interpreted bytes.
    assert!(fmt(src).contains("\\t"));
}

#[test]
fn numeric_literals_round_trip() {
    assert_eq!(fmt("x = 0xFF"), "x = 0xFF\n");
    assert_eq!(fmt("x = 0b1010"), "x = 0b1010\n");
    assert_eq!(fmt("x = 1.5e10"), "x = 1.5e10\n");
}

#[test]
fn idempotent_on_control_flow() {
    let src =
        "fn f(x: i64) -> i64:\n    if x > 0:\n        return x\n    else:\n        return 0\n";
    let once = fmt(src);
    assert_eq!(fmt(&once), once, "second pass changed output");
}

#[test]
fn struct_fields_and_methods() {
    let src = "struct Point:\n    x: i64\n    y: i64\n    fn sum(self) -> i64:\n        return self.x + self.y\n";
    let out = fmt(src);
    assert_eq!(fmt(&out), out);
    assert_eq!(canonical(src), canonical(&out));
}

#[test]
fn match_expression() {
    let src = "fn f(c: Color):\n    match c:\n        Red:\n            return 1\n        _:\n            return 0\n";
    let out = fmt(src);
    assert_eq!(canonical(src), canonical(&out));
    assert_eq!(fmt(&out), out);
}

#[test]
fn ast_is_preserved_on_complex_expr() {
    let src = "z = a and b or not c == d + e * f ** g\n";
    assert_eq!(canonical(src), canonical(&fmt(src)));
}

#[test]
fn opt_chain_round_trips() {
    let src = "struct User:\n    name: str\n\nfn f(u: User | None) -> str:\n    return u?.name ?? \"anon\"\n";
    let out = fmt(src);
    assert_eq!(out, src, "?. should print unchanged");
    assert_eq!(canonical(src), canonical(&out));
    assert_eq!(fmt(&out), out, "not idempotent");
}

#[test]
fn opt_chain_chained() {
    let src = "z = a?.b?.c\n";
    let out = fmt(src);
    assert_eq!(canonical(src), canonical(&out));
    assert_eq!(fmt(&out), out, "not idempotent");
}

#[test]
fn type_alias_round_trips() {
    let src = "type IntList = [int]\n";
    let out = fmt(src);
    assert_eq!(canonical(src), canonical(&out));
    assert_eq!(fmt(&out), out, "not idempotent");
}

#[test]
fn type_alias_union_round_trips() {
    let src = "type ParseResult = int | ParseError\n";
    let out = fmt(src);
    assert_eq!(canonical(src), canonical(&out));
    assert_eq!(fmt(&out), out, "not idempotent");
}

#[test]
fn power_operator_round_trips() {
    let src = "z = -2 ** 2 + 2 ** 3 ** 2\n";
    let out = fmt(src);
    assert_eq!(out, src);
    assert_eq!(canonical(src), canonical(&out));
    assert_eq!(fmt(&out), out, "not idempotent");
}

#[test]
fn fstring_debug_form_round_trips() {
    let src = "z = f\"{x=}\"\n";
    let out = fmt(src);
    assert_eq!(out, src);
    assert_eq!(canonical(src), canonical(&out));
    assert_eq!(fmt(&out), out, "not idempotent");
}

#[test]
fn in_range_round_trips() {
    let src = "z = 5 in 0..10\n";
    let out = fmt(src);
    assert_eq!(out, src);
    assert_eq!(canonical(src), canonical(&out));
    assert_eq!(fmt(&out), out, "not idempotent");
}

#[test]
fn numeric_underscore_literal_round_trips() {
    // fmt re-slices the original lexeme, so the separators themselves --
    // not just the numeric value -- must survive formatting unchanged.
    let src = "z = 1_000_000 + 0xFF_FF\n";
    let out = fmt(src);
    assert_eq!(out, src);
    assert_eq!(canonical(src), canonical(&out));
    assert_eq!(fmt(&out), out, "not idempotent");
}

/// The core guarantee, exercised over every shipped `.liv` file: formatting must
/// preserve the AST, be idempotent, and drop no comments.
#[test]
fn corpus_is_preserved() {
    let root = env!("CARGO_MANIFEST_DIR");
    let dirs = ["lib", "std_lib", "examples", "benchmark/src/olive", "grove"];
    let mut checked = 0;
    for dir in dirs {
        let base = std::path::Path::new(root).join(dir);
        if !base.exists() {
            continue;
        }
        for path in liv_files(&base) {
            let src = std::fs::read_to_string(&path).unwrap();
            if parse_fails(&src) {
                continue; // not all corpus files are guaranteed to parse standalone
            }
            let display = path.display();
            let out =
                format_source(&src, W).unwrap_or_else(|e| panic!("{display}: format error: {e}"));
            assert_eq!(
                canonical(&src),
                canonical(&out),
                "{display}: AST changed after formatting"
            );
            assert_eq!(
                format_source(&out, W).unwrap(),
                out,
                "{display}: not idempotent"
            );
            assert_eq!(
                comment_count(&src),
                comment_count(&out),
                "{display}: comments lost"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no corpus files were checked");
}

fn parse_fails(src: &str) -> bool {
    let tokens = match Lexer::new(src, 0).tokenise() {
        Ok(t) => t,
        Err(_) => return true,
    };
    Parser::new(tokens).parse_program().is_err()
}

fn liv_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(liv_files(&p));
            } else if p.extension().is_some_and(|x| x == "liv") {
                out.push(p);
            }
        }
    }
    out
}
