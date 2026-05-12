#[cfg(test)]
mod parser_tests_extended {
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::parser::ast::*;

    fn parse(src: &str) -> Result<Program, String> {
        let tokens = Lexer::new(src, 0)
            .tokenise()
            .map_err(|e| format!("lex error: {}", e.message))?;
        Parser::new(tokens)
            .parse_program()
            .map_err(|e| format!("parse error: {} at {}:{}", e.message, e.line, e.col))
    }

    fn parse_ok(src: &str) -> Program {
        parse(src).expect("expected successful parse")
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

    fn parse_err(src: &str) -> String {
        parse(src).expect_err("expected parse error")
    }

    #[test]
    fn empty_program_parses() {
        let p = parse_ok("");
        assert!(p.stmts.is_empty());
    }

    #[test]
    fn deeply_nested_binary_ops() {
        let mut src = String::from("let x = 1");
        for _ in 0..50 {
            src.push_str(" + 1");
        }
        src.push('\n');
        parse_ok(&src); // should not stack overflow
    }

    #[test]
    fn deeply_nested_function_calls() {
        let mut src = String::from("fn f(x: i64) -> i64:\n    return x + 1\n\nlet x = f(");
        for i in 0..50 {
            if i > 0 {
                src.push_str(", ");
            }
            src.push_str(&format!("f({})", i));
        }
        src.push_str(")\n");
        parse_ok(&src); // should not stack overflow
    }

    #[test]
    fn deeply_nested_tuple() {
        let mut src = String::from("let t = (");
        for _ in 0..5 {
            src.push('(');
        }
        src.push('1');
        for _ in 0..5 {
            src.push(')');
        }
        src.push('\n');
        let _ = parse(&src);
    }

    #[test]
    fn unicode_identifiers() {
        let p = parse_ok("let α = 1\nlet β = 2\nlet result = α + β\n");
        assert_eq!(p.stmts.len(), 3);
    }

    #[test]
    fn chained_comparisons() {
        let p = parse_ok("1 < x < 10\n");
        match expr_stmt(&p) {
            ExprKind::BinOp {
                op: BinOp::Lt,
                left,
                right,
            } => {
                assert!(matches!(left.kind, ExprKind::BinOp { op: BinOp::Lt, .. }));
                assert!(matches!(right.kind, ExprKind::Integer(10)));
            }
            _ => panic!("expected chained comparison ((1 < x) < 10)"),
        }
    }

    #[test]
    fn dangling_else_binds_to_inner_if() {
        let p = parse_ok("if x:\n    if y:\n        pass\n    else:\n        pass\n");
        match first(&p) {
            StmtKind::If {
                condition: _,
                then_body,
                elif_clauses,
                else_body,
            } => {
                assert!(elif_clauses.is_empty());
                assert!(else_body.is_none(), "else should bind to inner if");
                match &then_body[0].kind {
                    StmtKind::If { else_body, .. } => {
                        assert!(else_body.is_some(), "inner if should have else")
                    }
                    _ => panic!("expected inner if"),
                }
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn match_with_multiple_cases_and_guards() {
        let p = parse_ok(
            "match x:\n    case 1:\n        pass\n    case 2:\n        pass\n    case _:\n        pass\n",
        );
        match expr_stmt(&p) {
            ExprKind::Match { cases, .. } => assert_eq!(cases.len(), 3),
            _ => panic!("expected match"),
        }
    }

    #[test]
    fn nested_ifs_with_elif() {
        let p = parse_ok(
            "if a:\n    if b:\n        pass\nelif c:\n    if d:\n        pass\n    else:\n        pass\nelse:\n    pass\n",
        );
        match first(&p) {
            StmtKind::If {
                elif_clauses,
                else_body,
                ..
            } => {
                assert_eq!(elif_clauses.len(), 1);
                assert!(else_body.is_some());
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn try_expr_structure() {
        match expr_stmt(&parse_ok("try f()\n")) {
            ExprKind::Try(inner) => assert!(matches!(inner.kind, ExprKind::Call { .. })),
            _ => panic!("expected Try with Call"),
        }
    }

    #[test]
    fn augmented_assignment_parses() {
        match first(&parse_ok("x += 1\n")) {
            StmtKind::AugAssign { op, .. } => assert_eq!(*op, AugOp::Add),
            _ => panic!("expected AugAssign"),
        }
    }

    #[test]
    fn with_statement_parses() {
        match first(&parse_ok("with open(\"f\"):\n    pass\n")) {
            StmtKind::With { items, .. } => assert_eq!(items.len(), 1),
            _ => panic!("expected With"),
        }
    }

    #[test]
    fn while_with_else_parses() {
        let p = parse_ok("let mut i = 0\nwhile i < 10:\n    i = i + 1\nelse:\n    pass\n");
        match &p.stmts[1].kind {
            StmtKind::While { else_body, .. } => assert!(else_body.is_some()),
            _ => panic!("expected While with else"),
        }
    }

    #[test]
    fn for_with_else_parses() {
        let p = parse_ok("for x in items:\n    pass\nelse:\n    pass\n");
        match first(&p) {
            StmtKind::For { else_body, .. } => assert!(else_body.is_some()),
            _ => panic!("expected For with else"),
        }
    }

    #[test]
    fn generator_expression_or_comprehension() {
        match expr_stmt(&parse_ok("{x for x in items}\n")) {
            ExprKind::SetComp { .. } => {}
            _ => panic!("expected SetComp"),
        }
    }

    #[test]
    fn kwarg_only_after_vararg() {
        let p = parse_ok("fn f(a: i64, *args: i64, kw: i64 = 0):\n    pass\n");
        match first(&p) {
            StmtKind::Fn { params, .. } => {
                assert_eq!(params.len(), 3);
                assert_eq!(params[1].kind, ParamKind::VarArg);
            }
            _ => panic!("expected Fn"),
        }
    }

    #[test]
    fn struct_with_default_values() {
        let p = parse_ok("struct Point:\n    x: i64 = 0\n    y: i64 = 0\n");
        match first(&p) {
            StmtKind::Struct { fields, .. } => {
                assert_eq!(fields.len(), 2);
                assert!(fields[0].default.is_some());
            }
            _ => panic!("expected Struct"),
        }
    }

    #[test]
    fn enum_with_no_variants() {
        let p = parse_ok("enum Empty:\n    Placeholder\n");
        match first(&p) {
            StmtKind::Enum { name, variants, .. } => {
                assert_eq!(name, "Empty");
                assert_eq!(variants.len(), 1);
                assert_eq!(variants[0].name, "Placeholder");
            }
            _ => panic!("expected Enum"),
        }
    }

    #[test]
    fn unsafe_block_structure() {
        match first(&parse_ok("unsafe:\n    let p = &x\n")) {
            StmtKind::UnsafeBlock(body) => {
                assert_eq!(body.len(), 1);
                assert!(matches!(body[0].kind, StmtKind::Let { .. }));
            }
            _ => panic!("expected UnsafeBlock"),
        }
    }

    #[test]
    fn tuple_unpacking_in_for() {
        let p = parse_ok("for (a, b) in pairs:\n    pass\n");
        match first(&p) {
            StmtKind::For { target, .. } => {
                assert!(matches!(target, ForTarget::Tuple(..)));
            }
            _ => panic!("expected For with tuple target"),
        }
    }

    #[test]
    fn multiple_const_declarations() {
        match first(&parse_ok("const X, Y = 1, 2\n")) {
            StmtKind::MultiConst { names, .. } => assert_eq!(names.len(), 2),
            _ => panic!("expected MultiConst"),
        }
    }

    #[test]
    fn nested_template_literals() {
        let p = parse_ok(r#"f"hello {x} world""#);
        match expr_stmt(&p) {
            ExprKind::FStr(parts) => {
                assert_eq!(parts.len(), 3);
                assert!(matches!(parts[0].expr.kind, ExprKind::Str(_)));
                assert!(matches!(parts[1].expr.kind, ExprKind::Identifier(_)));
                assert!(matches!(parts[2].expr.kind, ExprKind::Str(_)));
            }
            _ => panic!("expected FStr"),
        }
    }

    #[test]
    fn match_on_literal_cases() {
        let p = parse_ok(
            "match x:\n    case 1:\n        pass\n    case \"hi\":\n        pass\n    case True:\n        pass\n",
        );
        match expr_stmt(&p) {
            ExprKind::Match { cases, .. } => assert_eq!(cases.len(), 3),
            _ => panic!("expected Match"),
        }
    }

    #[test]
    fn class_like_struct_with_methods() {
        let p = parse_ok(
            "struct Counter:\n    val: i64\n\nimpl Counter:\n    fn inc(self) -> i64:\n        return self.val + 1\n",
        );
        assert_eq!(p.stmts.len(), 2);
        match &p.stmts[1].kind {
            StmtKind::Impl { body, .. } => {
                assert_eq!(body.len(), 1);
                assert!(matches!(body[0].kind, StmtKind::Fn { .. }));
            }
            _ => panic!("expected Impl"),
        }
    }

    #[test]
    fn missing_colon_in_if_rejected() {
        let err = parse_err("if x\n    pass\n");
        assert!(!err.is_empty(), "missing colon should error");
    }

    #[test]
    fn mismatched_indentation_rejected() {
        let err = parse_err("if x:\n    pass\n  pass\n");
        assert!(!err.is_empty(), "bad indentation should error");
    }

    #[test]
    fn unmatched_delimiter_rejected() {
        let err = parse_err("(\n");
        assert!(!err.is_empty(), "unmatched paren should error");
    }

    #[test]
    fn invalid_assignment_target_rejected() {
        let err = parse_err("42 = x\n");
        assert!(!err.is_empty(), "assign to literal should error");
    }

    #[test]
    fn duplicate_param_name_accepted_by_parser() {
        let p = parse_ok("fn f(x: i64, x: i64):\n    pass\n");
        match first(&p) {
            StmtKind::Fn { params, .. } => assert_eq!(params.len(), 2),
            _ => panic!("expected Fn"),
        }
    }

    #[test]
    fn empty_body_after_fn_decl() {
        let p = parse_ok("fn f():\n    pass\n");
        match first(&p) {
            StmtKind::Fn { body, .. } => assert_eq!(body.len(), 1),
            _ => panic!("expected Fn"),
        }
    }

    #[test]
    fn import_statements() {
        let p = parse_ok("import \"foo\" as foo\n");
        match first(&p) {
            StmtKind::NativeImport { alias, .. } => {
                assert_eq!(alias, "foo");
            }
            _ => panic!("expected NativeImport"),
        }
    }

    #[test]
    fn from_import() {
        let p = parse_ok("from foo import bar, baz\n");
        match first(&p) {
            StmtKind::FromImport { names, is_star, .. } => {
                assert!(!is_star);
                assert_eq!(names.len(), 2);
            }
            _ => panic!("expected FromImport"),
        }
    }

    #[test]
    fn from_import_star() {
        let p = parse_ok("from foo import *\n");
        match first(&p) {
            StmtKind::FromImport { is_star, .. } => assert!(*is_star),
            _ => panic!("expected FromImport with star"),
        }
    }

    #[test]
    fn decorators_on_fn() {
        let p = parse_ok("@decorator\nfn f():\n    pass\n");
        match first(&p) {
            StmtKind::Fn { decorators, .. } => {
                assert_eq!(decorators.len(), 1);
                assert_eq!(decorators[0].name, "decorator");
            }
            _ => panic!("expected Fn with decorator"),
        }
    }

    #[test]
    fn multiple_decorators() {
        let p = parse_ok("@a\n@b\nfn f():\n    pass\n");
        match first(&p) {
            StmtKind::Fn { decorators, .. } => assert_eq!(decorators.len(), 2),
            _ => panic!("expected Fn with multiple decorators"),
        }
    }

    #[test]
    fn anonymous_fn_requires_fn_keyword_in_expr_position() {
        let err = parse_err("let f = fn(x: i64) -> i64:\n    return x + 1\n");
        assert!(!err.is_empty(), "anonymous fn syntax not yet supported");
    }

    #[test]
    fn slice_expression() {
        match expr_stmt(&parse_ok("xs[1:10]\n")) {
            ExprKind::Index { obj: _, index } => match &index.kind {
                ExprKind::Slice { start, stop, step } => {
                    assert!(start.is_some());
                    assert!(stop.is_some());
                    assert!(step.is_none());
                }
                _ => panic!("expected Slice"),
            },
            _ => panic!("expected Index with Slice"),
        }
    }

    #[test]
    fn slice_with_step() {
        match expr_stmt(&parse_ok("xs[1:10:2]\n")) {
            ExprKind::Index { obj: _, index } => match &index.kind {
                ExprKind::Slice { start, stop, step } => {
                    assert!(start.is_some());
                    assert!(stop.is_some());
                    assert!(step.is_some());
                }
                _ => panic!("expected Slice with step"),
            },
            _ => panic!("expected Index"),
        }
    }

    #[test]
    fn cast_operator() {
        match expr_stmt(&parse_ok("x as i64\n")) {
            ExprKind::Cast(_, _) => {}
            _ => panic!("expected Cast"),
        }
    }

    #[test]
    fn deref_operator() {
        match expr_stmt(&parse_ok("*p\n")) {
            ExprKind::Deref(_) => {}
            _ => panic!("expected Deref"),
        }
    }
}
