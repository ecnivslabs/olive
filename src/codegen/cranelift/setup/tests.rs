use crate::test_utils::{call_i64, compile};

#[test]
fn test_generate_simple_function() {
    let mut cg = compile("fn f() -> i64:\n    return 42\n");
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn test_generate_with_string_constant() {
    let _cg = compile("fn f() -> i64:\n    return len(\"hello\")\n");
}

#[test]
fn test_generate_with_global_var() {
    let mut cg = compile("fn f() -> i64:\n    return 0\n");
    assert_eq!(call_i64(&mut cg, "f"), 0);
}

#[test]
fn test_generate_with_struct() {
    let mut cg = compile(
        "struct Point:\n    x: i64\n    y: i64\n\nfn f() -> i64:\n    let p = Point(10, 32)\n    return p.x + p.y\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}

#[test]
fn test_generate_with_enum() {
    let mut cg = compile(
        "enum Opt:\n    Some(i64)\n    Nil\n\nfn f() -> i64:\n    let o = Some(42)\n    match o:\n        case Some(v):\n            return v\n        case Nil:\n            return 0\n",
    );
    assert_eq!(call_i64(&mut cg, "f"), 42);
}
