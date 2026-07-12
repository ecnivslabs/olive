use super::super::MirBuilder;
use crate::lexer::Lexer;
use crate::mir::ir::{Constant, Operand, Rvalue, StatementKind, TerminatorKind};
use crate::parser::Parser;
use crate::semantic::{Resolver, TypeChecker};
use rustc_hash::FxHashSet;

fn build(src: &str) -> Vec<super::super::super::ir::MirFunction> {
    let tokens = Lexer::new(src, 0).tokenise().unwrap();
    let prog = Parser::new(tokens).parse_program().unwrap();
    let mut r = Resolver::new();
    r.resolve_program(&prog);
    let mut tc = TypeChecker::new();
    tc.check_program(&prog);
    let mut builder = MirBuilder::new(
        &tc.expr_types,
        &tc.expr_kwarg_maps,
        &tc.type_env[0],
        tc.struct_fields.clone(),
        &tc.traits,
        FxHashSet::default(),
        tc.enum_defs.clone(),
    );
    builder.build_program(&prog);
    builder.functions
}

#[test]
fn let_stmt_assigns_to_local() {
    let fns = build("let x = 42\n");
    let main = fns.iter().find(|f| f.name == "__main__").unwrap();
    let has_assign = main.basic_blocks.iter().any(|bb| {
        bb.statements
            .iter()
            .any(|s| matches!(&s.kind, StatementKind::Assign(_, _)))
    });
    assert!(has_assign);
}

#[test]
fn return_stmt_assigns_to_local_zero() {
    let fns = build("fn f() -> i64:\n    return 42\n");
    let f = fns.iter().find(|f| f.name == "f").unwrap();
    let has_return_assign = f.basic_blocks.iter().any(|bb| {
        bb.statements.iter().any(|s| {
            matches!(&s.kind, StatementKind::Assign(l, Rvalue::Use(Operand::Constant(Constant::Int(42)))) if l.0 == 0)
        })
    });
    assert!(has_return_assign);
}

#[test]
fn return_terminates_block() {
    let fns = build("fn f() -> i64:\n    return 1\n    let dead = 2\n");
    let f = fns.iter().find(|f| f.name == "f").unwrap();
    assert!(f.basic_blocks[0].terminator.is_some());
    assert!(matches!(
        f.basic_blocks[0].terminator.as_ref().unwrap().kind,
        TerminatorKind::Return
    ));
}

#[test]
fn assignment_to_identifier() {
    let fns = build("fn f():\n    let x = 1\n    x = 2\n");
    let f = fns.iter().find(|f| f.name == "f").unwrap();
    let has_second_assign = f.basic_blocks.iter().any(|bb| {
        bb.statements.iter().any(|s| {
            matches!(
                &s.kind,
                StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Int(2))))
            )
        })
    });
    assert!(has_second_assign);
}

#[test]
fn struct_auto_init_generates_function() {
    let fns = build("struct Pt:\n    x: i64\n    y: i64\n");
    let has_init = fns.iter().any(|f| f.name.contains("__init__"));
    assert!(has_init);
}

#[test]
fn enum_decl_registers_variants() {
    let fns = build("enum Opt:\n    Some(i64)\n    Nil\n\nlet v = Some(42)\n");
    let main = fns.iter().find(|f| f.name == "__main__").unwrap();
    let has_aggregate = main.basic_blocks.iter().any(|bb| {
        bb.statements
            .iter()
            .any(|s| matches!(&s.kind, StatementKind::Assign(_, Rvalue::Aggregate(_, _))))
    });
    assert!(has_aggregate);
}

// A float stored into a PyObject subscript must reach Python exactly once.
// The general assign coerce already runs `py_from_float`; the subscript path
// must not wrap it a second time, which read the resulting pointer as f64 bits.
#[test]
fn pyobject_float_setitem_converts_once() {
    let src =
        "import py \"builtins\" as b\n\nfn main():\n    let d = b.dict()\n    d[\"f\"] = 0.5\n";
    let fns = build(src);
    let main = fns.iter().find(|f| f.name == "main").unwrap();
    let from_float = main
        .basic_blocks
        .iter()
        .flat_map(|bb| &bb.statements)
        .filter(|s| {
            matches!(
                &s.kind,
                StatementKind::Assign(_, Rvalue::Call { func, .. })
                    if matches!(func, Operand::Constant(Constant::Function(n)) if n == "__olive_py_from_float")
            )
        })
        .count();
    assert_eq!(
        from_float, 1,
        "float setitem must convert to Python exactly once"
    );
}
