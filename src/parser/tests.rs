#[cfg(test)]
mod tests {
    use crate::Parser;
    use crate::lexer::Lexer;
    use crate::parser::ast::*;

    fn parse(src: &str) -> Program {
        let tokens = Lexer::new(src, 0).tokenise().expect("lex error");
        Parser::new(tokens).parse_program().expect("parse error")
    }

    fn first(p: &Program) -> &StmtKind {
        &p.stmts.first().expect("empty program").kind
    }

    fn expr_stmt(p: &Program) -> &ExprKind {
        match first(p) {
            StmtKind::ExprStmt(e) => &e.kind,
            _ => panic!("expected ExprStmt"),
        }
    }

    #[test]
    fn integer_literal() {
        assert!(matches!(expr_stmt(&parse("42\n")), ExprKind::Integer(42)));
    }

    #[test]
    fn hex_oct_bin_literals() {
        assert!(matches!(
            expr_stmt(&parse("0xFF\n")),
            ExprKind::Integer(255)
        ));
        assert!(matches!(expr_stmt(&parse("0o77\n")), ExprKind::Integer(63)));
        assert!(matches!(
            expr_stmt(&parse("0b1010\n")),
            ExprKind::Integer(10)
        ));
    }

    #[test]
    fn float_literal() {
        assert!(matches!(expr_stmt(&parse("3.14\n")), ExprKind::Float(_)));
    }

    #[test]
    fn string_literal() {
        assert!(matches!(expr_stmt(&parse("\"hello\"\n")), ExprKind::Str(s) if s == "hello"));
    }

    #[test]
    fn bool_null_literals() {
        assert!(matches!(expr_stmt(&parse("True\n")), ExprKind::Bool(true)));
        assert!(matches!(
            expr_stmt(&parse("False\n")),
            ExprKind::Bool(false)
        ));
    }

    #[test]
    fn additive_left_assoc() {
        match expr_stmt(&parse("1 + 2 - 3\n")) {
            ExprKind::BinOp {
                op: BinOp::Sub,
                left,
                ..
            } => assert!(matches!(left.kind, ExprKind::BinOp { op: BinOp::Add, .. })),
            _ => panic!(),
        }
    }

    #[test]
    fn mul_over_add() {
        match expr_stmt(&parse("1 + 2 * 3\n")) {
            ExprKind::BinOp {
                op: BinOp::Add,
                right,
                ..
            } => assert!(matches!(right.kind, ExprKind::BinOp { op: BinOp::Mul, .. })),
            _ => panic!(),
        }
    }

    #[test]
    fn power_right_assoc() {
        match expr_stmt(&parse("2**3**2\n")) {
            ExprKind::BinOp {
                op: BinOp::Pow,
                right,
                ..
            } => assert!(matches!(right.kind, ExprKind::BinOp { op: BinOp::Pow, .. })),
            _ => panic!(),
        }
    }

    #[test]
    fn unary_neg_over_power() {
        match expr_stmt(&parse("-2**2\n")) {
            ExprKind::UnaryOp {
                op: UnaryOp::Neg,
                operand,
            } => assert!(matches!(
                operand.kind,
                ExprKind::BinOp { op: BinOp::Pow, .. }
            )),
            _ => panic!(),
        }
    }

    #[test]
    fn unary_pos() {
        assert!(matches!(
            expr_stmt(&parse("+x\n")),
            ExprKind::UnaryOp {
                op: UnaryOp::Pos,
                ..
            }
        ));
    }

    #[test]
    fn line_comment_ignored() {
        let stmts = parse("x = 1 // this is a comment\nx = 2\n");
        assert_eq!(stmts.stmts.len(), 2);
    }

    #[test]
    fn comparison_ops() {
        for src in [
            "a == b\n", "a != b\n", "a < b\n", "a <= b\n", "a > b\n", "a >= b\n",
        ] {
            parse(src);
        }
    }

    #[test]
    fn in_not_in_operators() {
        assert!(matches!(
            expr_stmt(&parse("x in [1,2]\n")),
            ExprKind::BinOp { op: BinOp::In, .. }
        ));
        assert!(matches!(
            expr_stmt(&parse("x not in [1,2]\n")),
            ExprKind::BinOp {
                op: BinOp::NotIn,
                ..
            }
        ));
    }

    #[test]
    fn logical_not_in_combination() {
        match expr_stmt(&parse("not x in y\n")) {
            ExprKind::UnaryOp {
                op: UnaryOp::Not,
                operand,
            } => assert!(matches!(
                operand.kind,
                ExprKind::BinOp { op: BinOp::In, .. }
            )),
            _ => panic!(),
        }
    }

    #[test]
    fn logical_and_or_precedence() {
        match expr_stmt(&parse("a and b or c\n")) {
            ExprKind::BinOp {
                op: BinOp::Or,
                left,
                ..
            } => assert!(matches!(left.kind, ExprKind::BinOp { op: BinOp::And, .. })),
            _ => panic!(),
        }
    }

    #[test]
    fn call_no_args() {
        assert!(matches!(expr_stmt(&parse("f()\n")),
            ExprKind::Call { args, .. } if args.is_empty()));
    }

    #[test]
    fn call_splat_kwsplat() {
        let p = parse("f(*a, **b)\n");
        match expr_stmt(&p) {
            ExprKind::Call { args, .. } => {
                assert!(matches!(args[0], CallArg::Splat(_)));
                assert!(matches!(args[1], CallArg::KwSplat(_)));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn call_keyword_arg() {
        match expr_stmt(&parse("f(x=1)\n")) {
            ExprKind::Call { args, .. } => {
                assert!(matches!(&args[0], CallArg::Keyword(n, _) if n == "x"))
            }
            _ => panic!(),
        }
    }

    #[test]
    fn attribute_chain() {
        match expr_stmt(&parse("a.b.c\n")) {
            ExprKind::Attr { obj, attr } => {
                assert_eq!(attr, "c");
                assert!(matches!(obj.kind, ExprKind::Attr { .. }));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn index_expr() {
        assert!(matches!(
            expr_stmt(&parse("a[0]\n")),
            ExprKind::Index { .. }
        ));
    }

    #[test]
    fn list_literal() {
        match expr_stmt(&parse("[1,2,3]\n")) {
            ExprKind::List(v) => assert_eq!(v.len(), 3),
            _ => panic!(),
        }
    }

    #[test]
    fn set_literal() {
        match expr_stmt(&parse("{1,2,3}\n")) {
            ExprKind::Set(v) => assert_eq!(v.len(), 3),
            _ => panic!(),
        }
    }

    #[test]
    fn tuple_forms() {
        assert!(matches!(expr_stmt(&parse("()\n")), ExprKind::Tuple(v) if v.is_empty()));
        assert!(matches!(expr_stmt(&parse("(1,)\n")), ExprKind::Tuple(v) if v.len() == 1));
        assert!(matches!(expr_stmt(&parse("(1)\n")), ExprKind::Integer(1)));
    }

    #[test]
    fn dict_empty_and_literal() {
        assert!(matches!(expr_stmt(&parse("{}\n")), ExprKind::Dict(p) if p.is_empty()));
        match expr_stmt(&parse("{\"a\":1,\"b\":2}\n")) {
            ExprKind::Dict(pairs) => assert_eq!(pairs.len(), 2),
            _ => panic!(),
        }
    }

    #[test]
    fn list_comprehension() {
        match expr_stmt(&parse("[x * 2 for x in items]\n")) {
            ExprKind::ListComp { clauses, .. } => assert_eq!(clauses.len(), 1),
            _ => panic!(),
        }
    }

    #[test]
    fn list_comp_with_condition() {
        match expr_stmt(&parse("[x for x in items if x > 0]\n")) {
            ExprKind::ListComp { clauses, .. } => assert!(clauses[0].condition.is_some()),
            _ => panic!(),
        }
    }

    #[test]
    fn nested_list_comp() {
        match expr_stmt(&parse("[x for row in grid for x in row]\n")) {
            ExprKind::ListComp { clauses, .. } => assert_eq!(clauses.len(), 2),
            _ => panic!(),
        }
    }

    #[test]
    fn set_comprehension() {
        assert!(matches!(
            expr_stmt(&parse("{x for x in items}\n")),
            ExprKind::SetComp { .. }
        ));
    }

    #[test]
    fn dict_comprehension() {
        match expr_stmt(&parse("{k: v for k, v in pairs}\n")) {
            ExprKind::DictComp { clauses, .. } => {
                assert!(matches!(clauses[0].target, ForTarget::Tuple(..)))
            }
            _ => panic!(),
        }
    }

    #[test]
    fn let_stmt() {
        assert!(matches!(first(&parse("let x = 42\n")), StmtKind::Let { name, .. } if name == "x"));
    }

    #[test]
    fn let_with_type_ann() {
        match first(&parse("let x: i64 = 42\n")) {
            StmtKind::Let { name, type_ann, .. } => {
                assert_eq!(name, "x");
                assert!(
                    matches!(type_ann, Some(TypeExpr { kind: TypeExprKind::Name(t), .. }) if t == "i64")
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn assign_stmt() {
        assert!(matches!(first(&parse("x = 1\n")), StmtKind::Assign { .. }));
    }

    #[test]
    fn tuple_unpack_assign() {
        match first(&parse("a, b = 1, 2\n")) {
            StmtKind::Assign { target, value } => {
                assert!(matches!(target.kind, ExprKind::Tuple(ref lhs) if lhs.len() == 2));
                assert!(matches!(value.kind, ExprKind::Tuple(ref rhs) if rhs.len() == 2));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn aug_assign_all_ops() {
        for (src, expected) in [
            ("x += 1\n", AugOp::Add),
            ("x -= 1\n", AugOp::Sub),
            ("x *= 1\n", AugOp::Mul),
            ("x /= 1\n", AugOp::Div),
            ("x %= 1\n", AugOp::Mod),
            ("x **= 1\n", AugOp::Pow),
            ("x <<= 1\n", AugOp::Shl),
            ("x >>= 1\n", AugOp::Shr),
        ] {
            match first(&parse(src)) {
                StmtKind::AugAssign { op, .. } => assert_eq!(*op, expected, "src={src}"),
                _ => panic!("not AugAssign for {src}"),
            }
        }
    }

    #[test]
    fn pass_break_continue() {
        assert!(matches!(first(&parse("pass\n")), StmtKind::Pass));
        assert!(matches!(first(&parse("break\n")), StmtKind::Break));
        assert!(matches!(first(&parse("continue\n")), StmtKind::Continue));
    }

    #[test]
    fn return_stmt() {
        assert!(matches!(first(&parse("fn f():\n    return 1\n")),
            StmtKind::Fn { body, .. } if matches!(body[0].kind, StmtKind::Return(Some(_)))));
        assert!(matches!(first(&parse("fn f():\n    return\n")),
            StmtKind::Fn { body, .. } if matches!(body[0].kind, StmtKind::Return(None))));
    }

    #[test]
    fn single_line_if() {
        match first(&parse("if x: pass\n")) {
            StmtKind::If { then_body, .. } => assert!(matches!(then_body[0].kind, StmtKind::Pass)),
            _ => panic!(),
        }
    }

    #[test]
    fn for_loop_simple() {
        match first(&parse("for i in items:\n    pass\n")) {
            StmtKind::For {
                target: ForTarget::Name(n, _),
                ..
            } => assert_eq!(n, "i"),
            _ => panic!(),
        }
    }

    #[test]
    fn try_expr_basic() {
        match expr_stmt(&parse("try parse()\n")) {
            ExprKind::Try(inner) => {
                assert!(matches!(inner.kind, ExprKind::Call { .. }));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn fn_vararg_kwarg() {
        match first(&parse("fn f(a, *args, **kwargs):\n    pass\n")) {
            StmtKind::Fn { params, .. } => {
                assert_eq!(params[0].kind, ParamKind::Regular);
                assert_eq!(params[1].kind, ParamKind::VarArg);
                assert_eq!(params[2].kind, ParamKind::KwArg);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn bitwise_ops() {
        match expr_stmt(&parse("x << 1 >> 2\n")) {
            ExprKind::BinOp {
                op: BinOp::Shr,
                left,
                ..
            } => assert!(matches!(left.kind, ExprKind::BinOp { op: BinOp::Shl, .. })),
            _ => panic!(),
        }
    }

    #[test]
    fn f_string_basic() {
        match expr_stmt(&parse("f\"x is {x}\"\n")) {
            ExprKind::FStr(parts) => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(parts[0].kind, ExprKind::Str(_)));
                assert!(matches!(parts[1].kind, ExprKind::Identifier(_)));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn match_stmt_basic() {
        match expr_stmt(&parse("match x:\n    case 1: pass\n    case _: pass\n")) {
            ExprKind::Match { cases, .. } => {
                assert_eq!(cases.len(), 2);
                assert!(matches!(cases[0].pattern, MatchPattern::Literal(_)));
                assert!(matches!(cases[1].pattern, MatchPattern::Wildcard));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn borrow_exprs() {
        assert!(matches!(expr_stmt(&parse("&x\n")), ExprKind::Borrow(_)));
        assert!(matches!(
            expr_stmt(&parse("&mut x\n")),
            ExprKind::MutBorrow(_)
        ));
    }

    #[test]
    fn async_block_expr() {
        match expr_stmt(&parse("async:\n    pass\n")) {
            ExprKind::AsyncBlock(stmts) => assert!(matches!(stmts[0].kind, StmtKind::Pass)),
            _ => panic!(),
        }
    }

    #[test]
    fn impl_block_parsing() {
        match first(&parse("impl MyStruct:\n    fn f(): pass\n")) {
            StmtKind::Impl { type_name, body } => {
                assert_eq!(type_name, "MyStruct");
                assert!(matches!(body[0].kind, StmtKind::Fn { .. }));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn span_line_col() {
        let p = parse("let x = 42\n");
        let stmt = p.stmts.first().unwrap();
        assert_eq!(stmt.span.line, 1);
        assert_eq!(stmt.span.col, 1);
    }
    #[test]
    fn shift_precedence() {
        match expr_stmt(&parse("x + y << z\n")) {
            ExprKind::BinOp {
                op: BinOp::Shl,
                left,
                ..
            } => {
                assert!(matches!(left.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn nested_async_blocks() {
        let src = "async:\n    let x = await async:\n        return 1\n    return x\n";
        match first(&parse(src)) {
            StmtKind::ExprStmt(e) => match &e.kind {
                ExprKind::AsyncBlock(body) => {
                    assert_eq!(body.len(), 2);
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }
}
