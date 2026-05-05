use super::super::{Parser, ast::*};

fn make_parser(src: &str) -> Parser {
    let tokens = crate::lexer::Lexer::new(src, 0)
        .tokenise()
        .expect("lex error");
    Parser::new(tokens)
}

#[test]
fn parse_pass() {
    let mut p = make_parser("pass\n");
    let stmt = p.parse_stmt().expect("parse failed");
    assert!(matches!(stmt.kind, StmtKind::Pass));
}

#[test]
fn parse_break() {
    let mut p = make_parser("break\n");
    let stmt = p.parse_stmt().expect("parse failed");
    assert!(matches!(stmt.kind, StmtKind::Break));
}

#[test]
fn parse_continue() {
    let mut p = make_parser("continue\n");
    let stmt = p.parse_stmt().expect("parse failed");
    assert!(matches!(stmt.kind, StmtKind::Continue));
}

#[test]
fn parse_let_basic() {
    let mut p = make_parser("let x = 42\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::Let {
            name,
            value,
            is_mut,
            ..
        } => {
            assert_eq!(name, "x");
            assert!(!is_mut);
            assert!(matches!(value.kind, ExprKind::Integer(42)));
        }
        _ => panic!("expected Let"),
    }
}

#[test]
fn parse_let_mut() {
    let mut p = make_parser("let mut x = 42\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::Let { is_mut, .. } => assert!(*is_mut),
        _ => panic!("expected Let with mut"),
    }
}

#[test]
fn parse_let_with_type() {
    let mut p = make_parser("let x: i64 = 42\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::Let { type_ann, .. } => {
            assert!(
                matches!(type_ann, Some(TypeExpr { kind: TypeExprKind::Name(t), .. }) if t == "i64")
            );
        }
        _ => panic!("expected Let with type"),
    }
}

#[test]
fn parse_multi_let() {
    let mut p = make_parser("let x, y = 1, 2\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::MultiLet { names, .. } => {
            assert_eq!(names.len(), 2);
        }
        _ => panic!("expected MultiLet"),
    }
}

#[test]
fn parse_const() {
    let mut p = make_parser("const X = 10\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::Const { name, .. } => assert_eq!(name, "X"),
        _ => panic!("expected Const"),
    }
}

#[test]
fn parse_multi_const() {
    let mut p = make_parser("const X, Y = 1, 2\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::MultiConst { names, .. } => assert_eq!(names.len(), 2),
        _ => panic!("expected MultiConst"),
    }
}

#[test]
fn parse_return_no_value() {
    let mut p = make_parser("return\n");
    let stmt = p.parse_stmt().expect("parse failed");
    assert!(matches!(stmt.kind, StmtKind::Return(None)));
}

#[test]
fn parse_return_with_value() {
    let mut p = make_parser("return 42\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::Return(Some(expr)) => {
            assert!(matches!(expr.kind, ExprKind::Integer(42)));
        }
        _ => panic!("expected Return with value"),
    }
}

#[test]
fn parse_if_elif_else() {
    let mut p = make_parser("if x:\n    pass\nelif y:\n    pass\nelse:\n    pass\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::If {
            elif_clauses,
            else_body,
            ..
        } => {
            assert_eq!(elif_clauses.len(), 1);
            assert!(else_body.is_some());
        }
        _ => panic!("expected If"),
    }
}

#[test]
fn parse_while() {
    let mut p = make_parser("while x:\n    pass\n");
    let stmt = p.parse_stmt().expect("parse failed");
    assert!(matches!(stmt.kind, StmtKind::While { .. }));
}

#[test]
fn parse_for_single_target() {
    let mut p = make_parser("for i in items:\n    pass\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::For { target, .. } => {
            assert!(matches!(target, ForTarget::Name(n, _) if n == "i"));
        }
        _ => panic!("expected For"),
    }
}

#[test]
fn parse_for_tuple_target() {
    let mut p = make_parser("for a, b in pairs:\n    pass\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::For { target, .. } => {
            assert!(matches!(target, ForTarget::Tuple(..)));
        }
        _ => panic!("expected For"),
    }
}

#[test]
fn parse_assert_no_msg() {
    let mut p = make_parser("assert x > 0\n");
    let stmt = p.parse_stmt().expect("parse failed");
    assert!(matches!(stmt.kind, StmtKind::Assert { msg: None, .. }));
}

#[test]
fn parse_assert_with_msg() {
    let mut p = make_parser("assert x > 0, \"bad x\"\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::Assert { msg, .. } => assert!(msg.is_some()),
        _ => panic!("expected Assert with msg"),
    }
}

#[test]
fn parse_import() {
    let mut p = make_parser("import math\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::Import { module, alias } => {
            assert_eq!(module, &["math"]);
            assert!(alias.is_none());
        }
        _ => panic!("expected Import"),
    }
}

#[test]
fn parse_from_import() {
    let mut p = make_parser("from math import sin, cos\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::FromImport { names, is_star, .. } => {
            assert!(!is_star);
            assert_eq!(names.len(), 2);
        }
        _ => panic!("expected FromImport"),
    }
}

#[test]
fn parse_from_import_star() {
    let mut p = make_parser("from math import *\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::FromImport { is_star, .. } => assert!(*is_star),
        _ => panic!("expected FromImport"),
    }
}

#[test]
fn parse_defer() {
    let mut p = make_parser("defer f.close()\n");
    let stmt = p.parse_stmt().expect("parse failed");
    assert!(matches!(stmt.kind, StmtKind::Defer(_)));
}

#[test]
fn parse_unsafe() {
    let mut p = make_parser("unsafe:\n    *p = 0\n");
    let stmt = p.parse_stmt().expect("parse failed");
    assert!(matches!(stmt.kind, StmtKind::UnsafeBlock(_)));
}

#[test]
fn parse_with_single() {
    let mut p = make_parser("with open(\"f\"):\n    pass\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::With { items, .. } => {
            assert_eq!(items.len(), 1);
            assert!(items[0].alias.is_none());
        }
        _ => panic!("expected With"),
    }
}

#[test]
fn parse_with_alias() {
    let mut p = make_parser("with open(\"f\") as f:\n    pass\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::With { items, .. } => {
            assert!(items[0].alias.is_some());
        }
        _ => panic!("expected With"),
    }
}

#[test]
fn parse_block_single_line() {
    let mut p = make_parser(": pass\n");
    let block = p.parse_block().expect("parse failed");
    assert_eq!(block.len(), 1);
    assert!(matches!(block[0].kind, StmtKind::Pass));
}

#[test]
fn parse_block_indented() {
    let mut p = make_parser(":\n    pass\n    break\n");
    let block = p.parse_block().expect("parse failed");
    assert_eq!(block.len(), 2);
}

#[test]
fn parse_decorated_fn() {
    let mut p = make_parser("@dec\nfn f():\n    pass\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::Fn {
            decorators, name, ..
        } => {
            assert_eq!(decorators.len(), 1);
            assert_eq!(decorators[0].name, "dec");
            assert_eq!(name, "f");
        }
        _ => panic!("expected Fn with decorator"),
    }
}

#[test]
fn parse_assign() {
    let mut p = make_parser("x = 42\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::Assign { target, value } => {
            assert!(matches!(target.kind, ExprKind::Identifier(_)));
            assert!(matches!(value.kind, ExprKind::Integer(42)));
        }
        _ => panic!("expected Assign"),
    }
}

#[test]
fn parse_aug_assign() {
    let mut p = make_parser("x += 1\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::AugAssign { op, .. } => assert_eq!(*op, AugOp::Add),
        _ => panic!("expected AugAssign"),
    }
}

#[test]
fn parse_expr_stmt() {
    let mut p = make_parser("f()\n");
    let stmt = p.parse_stmt().expect("parse failed");
    assert!(matches!(stmt.kind, StmtKind::ExprStmt(_)));
}

#[test]
fn parse_import_with_alias() {
    let mut p = make_parser("import math as m\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::Import { alias, .. } => {
            assert_eq!(alias.as_deref(), Some("m"));
        }
        _ => panic!("expected Import"),
    }
}

#[test]
fn parse_from_import_with_alias() {
    let mut p = make_parser("from math import sin as s\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::FromImport { names, .. } => {
            assert_eq!(names[0].1.as_deref(), Some("s"));
        }
        _ => panic!("expected FromImport"),
    }
}

#[test]
fn parse_py_import_simple() {
    let mut p = make_parser("import py \"glm\" as glm\n");
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::PyImport {
            module,
            alias,
            typed_types,
            typed_fns,
        } => {
            assert_eq!(module, "glm");
            assert_eq!(alias, "glm");
            assert!(typed_types.is_empty());
            assert!(typed_fns.is_empty());
        }
        _ => panic!("expected PyImport"),
    }
}

#[test]
fn parse_py_import_typed_block() {
    let src = "import py \"glm\" as glm:\n    type vec3\n    type mat4\n    fn vec3(x: float, y: float, z: float) -> vec3\n";
    let mut p = make_parser(src);
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::PyImport {
            module,
            alias,
            typed_types,
            typed_fns,
        } => {
            assert_eq!(module, "glm");
            assert_eq!(alias, "glm");
            assert_eq!(typed_types.len(), 2);
            assert!(typed_types.contains(&"vec3".to_string()));
            assert!(typed_types.contains(&"mat4".to_string()));
            assert_eq!(typed_fns.len(), 1);
            assert_eq!(typed_fns[0].name, "vec3");
            assert_eq!(typed_fns[0].params.len(), 3);
        }
        _ => panic!("expected PyImport"),
    }
}

#[test]
fn parse_struct_with_dotted_field_type() {
    let src = "struct Camera:\n    position: glm.vec3\n    aspect: f64\n";
    let mut p = make_parser(src);
    let stmt = p.parse_struct().expect("parse failed");
    match &stmt.kind {
        StmtKind::Struct { fields, .. } => {
            assert_eq!(fields.len(), 2);
            let pos_ty = fields[0].type_ann.as_ref().unwrap();
            assert!(
                matches!(&pos_ty.kind, TypeExprKind::Qualified(parts) if parts == &["glm", "vec3"])
            );
        }
        _ => panic!("expected Struct"),
    }
}

#[test]
fn parse_py_import_overloaded_fn() {
    let src = "import py \"glm\" as glm:\n    type vec3\n    fn vec3(x: float, y: float, z: float) -> vec3\n    fn vec3(x: float) -> vec3\n";
    let mut p = make_parser(src);
    let stmt = p.parse_stmt().expect("parse failed");
    match &stmt.kind {
        StmtKind::PyImport { typed_fns, .. } => {
            assert_eq!(typed_fns.len(), 2);
            assert_eq!(typed_fns[0].params.len(), 3);
            assert_eq!(typed_fns[1].params.len(), 1);
        }
        _ => panic!("expected PyImport"),
    }
}
