//! Numeric underscore literal regression tests. Lexer-level tokenisation
//! edge cases live in `src/lexer/tests.rs`; these exercise the same
//! literals end to end through the full pipeline.
#[cfg(test)]
use crate::test_utils::{call_i64, compile};

#[test]
fn integer_underscore_grouping() {
    let mut cg = compile("fn main() -> int:\n    return 1_000_000\n");
    assert_eq!(call_i64(&mut cg, "main"), 1_000_000);
}

#[test]
fn float_underscore_grouping() {
    let mut cg = compile("fn main() -> int:\n    let f = 1_234.567_8\n    return int(f)\n");
    assert_eq!(call_i64(&mut cg, "main"), 1234);
}

#[test]
fn hex_octal_binary_underscore_grouping() {
    let mut cg = compile(
        "fn main() -> int:\n    let h = 0xFF_FF\n    let o = 0o17_17\n    let b = 0b1010_1010\n    return h + o + b\n",
    );
    assert_eq!(call_i64(&mut cg, "main"), 0xFFFF + 0o1717 + 0b10101010);
}
