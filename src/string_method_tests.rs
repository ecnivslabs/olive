//! E3.5 string method regression tests (roadmap.md Phase E3.5).
#[cfg(test)]
use crate::test_utils::{call_i64, compile};

#[test]
fn count_occurrences() {
    let mut cg = compile("fn f() -> int:\n    \"ababab\".count(\"ab\")\n");
    assert_eq!(call_i64(&mut cg, "f"), 3);
}

#[test]
fn rfind_last_occurrence() {
    let mut cg = compile("fn f() -> int:\n    \"hello world\".rfind(\"o\")\n");
    assert_eq!(call_i64(&mut cg, "f"), 7);
}

#[test]
fn rfind_absent_is_negative_one() {
    let mut cg = compile("fn f() -> int:\n    \"hello\".rfind(\"z\")\n");
    assert_eq!(call_i64(&mut cg, "f"), -1);
}

#[test]
fn splitlines_basic() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let ls = \"a\\nb\\nc\".splitlines()\n",
        "    if len(ls) == 3 and ls[0] == \"a\" and ls[2] == \"c\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn title_case() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    if \"hello world\".title() == \"Hello World\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn capitalize_lowercases_rest() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    if \"HELLO world\".capitalize() == \"Hello world\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn zfill_pads_after_sign() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    if \"42\".zfill(5) == \"00042\" and \"-42\".zfill(5) == \"-0042\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn ljust_rjust_center_default_fill() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    if \"ab\".ljust(5) == \"ab   \" and \"ab\".rjust(5) == \"   ab\" and \"ab\".center(6) == \"  ab  \":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn ljust_rjust_center_custom_fill() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    if \"ab\".ljust(5, \"*\") == \"ab***\" and \"ab\".rjust(5, \"*\") == \"***ab\" and \"ab\".center(6, \"*\") == \"**ab**\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn partition_found() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let p = \"key=value\".partition(\"=\")\n",
        "    if p[0] == \"key\" and p[1] == \"=\" and p[2] == \"value\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn partition_not_found() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let p = \"hello\".partition(\"=\")\n",
        "    if p[0] == \"hello\" and p[1] == \"\" and p[2] == \"\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn removeprefix_removesuffix() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    if \"hello.txt\".removeprefix(\"hello\") == \".txt\" and \"hello.txt\".removesuffix(\".txt\") == \"hello\" and \"nope\".removeprefix(\"xyz\") == \"nope\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn is_family() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    if \"123\".isdigit() and not \"12a\".isdigit() and \"abc\".isalpha() and not \"ab1\".isalpha() and \"   \".isspace() and \"ABC\".isupper() and \"abc\".islower() and not \"Abc\".isupper():\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn strip_with_chars() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    if \"xxhixx\".strip(\"x\") == \"hi\" and \"xxhixx\".lstrip(\"x\") == \"hixx\" and \"xxhixx\".rstrip(\"x\") == \"xxhi\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn strip_no_args_still_trims_whitespace() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    if \"  xx  \".strip() == \"xx\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn split_no_args_splits_whitespace_runs() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let ps = \"a b  c\".split()\n",
        "    if len(ps) == 3 and ps[0] == \"a\" and ps[1] == \"b\" and ps[2] == \"c\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}

#[test]
fn split_with_sep_still_works() {
    let mut cg = compile(concat!(
        "fn f() -> int:\n",
        "    let ps = \"a,b,c\".split(\",\")\n",
        "    if len(ps) == 3 and ps[1] == \"b\":\n",
        "        return 1\n",
        "    return 0\n",
    ));
    assert_eq!(call_i64(&mut cg, "f"), 1);
}
