use super::super::{Parser, ast::*};

fn make_parser(src: &str) -> Parser {
    let tokens = crate::lexer::Lexer::new(src, 0)
        .tokenise()
        .expect("lex error");
    Parser::new(tokens)
}

#[test]
fn parse_integer() {
    let mut p = make_parser("42\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Integer(42)));
}

#[test]
fn parse_float() {
    let mut p = make_parser("3.14\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Float(_)));
}

#[test]
fn parse_string() {
    let mut p = make_parser("\"hello\"\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Str(s) if s == "hello"));
}

#[test]
fn parse_bool() {
    let mut p = make_parser("True\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Bool(true)));
}

#[test]
fn parse_identifier() {
    let mut p = make_parser("myVar\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Identifier(s) if s == "myVar"));
}

#[test]
fn parse_unary_neg() {
    let mut p = make_parser("-42\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(
        expr.kind,
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            ..
        }
    ));
}

#[test]
fn parse_unary_not() {
    let mut p = make_parser("not x\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(
        expr.kind,
        ExprKind::UnaryOp {
            op: UnaryOp::Not,
            ..
        }
    ));
}

#[test]
fn parse_binary_add() {
    let mut p = make_parser("1 + 2\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
}

#[test]
fn parse_binary_mul_precedence() {
    let mut p = make_parser("1 + 2 * 3\n");
    let expr = p.parse_expr().expect("parse failed");
    match &expr.kind {
        ExprKind::BinOp {
            op: BinOp::Add,
            right,
            ..
        } => {
            assert!(matches!(right.kind, ExprKind::BinOp { op: BinOp::Mul, .. }));
        }
        _ => panic!("expected Add with Mul as right operand"),
    }
}

#[test]
fn parse_comparison_eq() {
    let mut p = make_parser("a == b\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Eq, .. }));
}

#[test]
fn parse_comparison_lt() {
    let mut p = make_parser("a < b\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Lt, .. }));
}

#[test]
fn parse_logical_or() {
    let mut p = make_parser("a or b\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Or, .. }));
}

#[test]
fn parse_logical_and() {
    let mut p = make_parser("a and b\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::And, .. }));
}

#[test]
fn parse_bitwise_or() {
    let mut p = make_parser("a | b\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(
        expr.kind,
        ExprKind::BinOp {
            op: BinOp::BitOr,
            ..
        }
    ));
}

#[test]
fn parse_bitwise_xor() {
    let mut p = make_parser("a ^ b\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(
        expr.kind,
        ExprKind::BinOp {
            op: BinOp::BitXor,
            ..
        }
    ));
}

#[test]
fn parse_power_right_assoc() {
    let mut p = make_parser("2 ** 3 ** 2\n");
    let expr = p.parse_expr().expect("parse failed");
    match &expr.kind {
        ExprKind::BinOp {
            op: BinOp::Pow,
            right,
            ..
        } => {
            assert!(matches!(right.kind, ExprKind::BinOp { op: BinOp::Pow, .. }));
        }
        _ => panic!("expected Pow with Pow as right operand"),
    }
}

#[test]
fn parse_borrow() {
    let mut p = make_parser("&x\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Borrow(_)));
}

#[test]
fn parse_mut_borrow() {
    let mut p = make_parser("&mut x\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::MutBorrow(_)));
}

#[test]
fn parse_deref() {
    let mut p = make_parser("*p\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Deref(_)));
}

#[test]
fn parse_try_expr() {
    let mut p = make_parser("try f()\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Try(_)));
}

#[test]
fn parse_expr_list_single() {
    let mut p = make_parser("42\n");
    let expr = p.parse_expr_list().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Integer(42)));
}

#[test]
fn parse_expr_list_tuple() {
    let mut p = make_parser("1, 2\n");
    let expr = p.parse_expr_list().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Tuple(v) if v.len() == 2));
}

#[test]
fn parse_call_no_args() {
    let mut p = make_parser("f()\n");
    let expr = p.parse_expr().expect("parse failed");
    match &expr.kind {
        ExprKind::Call { args, .. } => assert!(args.is_empty()),
        _ => panic!("expected Call"),
    }
}

#[test]
fn parse_call_positional() {
    let mut p = make_parser("f(1, 2)\n");
    let expr = p.parse_expr().expect("parse failed");
    match &expr.kind {
        ExprKind::Call { args, .. } => {
            assert_eq!(args.len(), 2);
            assert!(matches!(&args[0], CallArg::Positional(_)));
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn parse_call_keyword() {
    let mut p = make_parser("f(x = 1)\n");
    let expr = p.parse_expr().expect("parse failed");
    match &expr.kind {
        ExprKind::Call { args, .. } => {
            assert!(matches!(&args[0], CallArg::Keyword(n, _) if n == "x"));
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn parse_call_splat() {
    let mut p = make_parser("f(*args)\n");
    let expr = p.parse_expr().expect("parse failed");
    match &expr.kind {
        ExprKind::Call { args, .. } => {
            assert!(matches!(&args[0], CallArg::Splat(_)));
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn parse_call_kw_splat() {
    let mut p = make_parser("f(**kwargs)\n");
    let expr = p.parse_expr().expect("parse failed");
    match &expr.kind {
        ExprKind::Call { args, .. } => {
            assert!(matches!(&args[0], CallArg::KwSplat(_)));
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn parse_attr_chain() {
    let mut p = make_parser("a.b.c\n");
    let expr = p.parse_expr().expect("parse failed");
    match &expr.kind {
        ExprKind::Attr { obj, attr } => {
            assert_eq!(attr, "c");
            assert!(matches!(obj.kind, ExprKind::Attr { .. }));
        }
        _ => panic!("expected Attr"),
    }
}

#[test]
fn parse_index() {
    let mut p = make_parser("a[0]\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Index { .. }));
}

#[test]
fn parse_slice() {
    let mut p = make_parser("a[1:10]\n");
    let expr = p.parse_expr().expect("parse failed");
    match &expr.kind {
        ExprKind::Index { index, .. } => {
            assert!(matches!(
                index.kind,
                ExprKind::Slice {
                    start: Some(_),
                    stop: Some(_),
                    ..
                }
            ));
        }
        _ => panic!("expected Index with Slice"),
    }
}

#[test]
fn parse_tuple() {
    let mut p = make_parser("(1, 2, 3)\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Tuple(v) if v.len() == 3));
}

#[test]
fn parse_empty_tuple() {
    let mut p = make_parser("()\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Tuple(v) if v.is_empty()));
}

#[test]
fn parse_list() {
    let mut p = make_parser("[1, 2, 3]\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::List(v) if v.len() == 3));
}

#[test]
fn parse_empty_list() {
    let mut p = make_parser("[]\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::List(v) if v.is_empty()));
}

#[test]
fn parse_dict() {
    let mut p = make_parser("{\"a\": 1, \"b\": 2}\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Dict(v) if v.len() == 2));
}

#[test]
fn parse_empty_dict() {
    let mut p = make_parser("{}\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Dict(v) if v.is_empty()));
}

#[test]
fn parse_set() {
    let mut p = make_parser("{1, 2, 3}\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Set(v) if v.len() == 3));
}

#[test]
fn parse_list_comp() {
    let mut p = make_parser("[x for x in items]\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::ListComp { .. }));
}

#[test]
fn parse_list_comp_with_condition() {
    let mut p = make_parser("[x for x in items if x > 0]\n");
    let expr = p.parse_expr().expect("parse failed");
    match &expr.kind {
        ExprKind::ListComp { clauses, .. } => assert!(clauses[0].condition.is_some()),
        _ => panic!("expected ListComp"),
    }
}

#[test]
fn parse_cast() {
    let mut p = make_parser("x as i64\n");
    let expr = p.parse_expr().expect("parse failed");
    assert!(matches!(expr.kind, ExprKind::Cast(_, _)));
}

#[test]
fn parse_is_valid_assign_target_identifier() {
    let expr = Expr::new(
        ExprKind::Identifier("x".into()),
        crate::span::Span::default(),
    );
    assert!(Parser::is_valid_assign_target(&expr));
}

#[test]
fn parse_is_valid_assign_target_tuple() {
    let inner = Expr::new(
        ExprKind::Identifier("x".into()),
        crate::span::Span::default(),
    );
    let tuple = Expr::new(ExprKind::Tuple(vec![inner]), crate::span::Span::default());
    assert!(Parser::is_valid_assign_target(&tuple));
}

#[test]
fn parse_is_valid_assign_target_invalid() {
    let expr = Expr::new(ExprKind::Integer(42), crate::span::Span::default());
    assert!(!Parser::is_valid_assign_target(&expr));
}

#[test]
fn parse_pattern_wildcard() {
    let mut p = make_parser("_\n");
    let pat = p.parse_pattern().expect("parse failed");
    assert!(matches!(pat, MatchPattern::Wildcard));
}

#[test]
fn parse_pattern_literal() {
    let mut p = make_parser("42\n");
    let pat = p.parse_pattern().expect("parse failed");
    assert!(matches!(pat, MatchPattern::Literal(_)));
}

#[test]
fn parse_pattern_identifier() {
    let mut p = make_parser("x\n");
    let pat = p.parse_pattern().expect("parse failed");
    assert!(matches!(pat, MatchPattern::Identifier(n) if n == "x"));
}

#[test]
fn parse_pattern_variant() {
    let mut p = make_parser("Some(x)\n");
    let pat = p.parse_pattern().expect("parse failed");
    assert!(matches!(pat, MatchPattern::Variant(n, p) if n == "Some" && p.len() == 1));
}
