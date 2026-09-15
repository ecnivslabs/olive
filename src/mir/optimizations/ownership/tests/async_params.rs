use super::*;

#[test]
fn async_wrappers_do_not_transfer_caller_arguments() {
    let mut function = func_of(
        vec![decl(heap_ty(), true), decl(heap_ty(), false)],
        vec![assign(
            0,
            Rvalue::Aggregate(AggregateKind::List, vec![Operand::Copy(Local(1))]),
        )],
    );
    function.arg_count = 1;
    assert_eq!(compute_param_escapes(&[function.clone()])["f"], [true]);
    function.is_async = true;
    assert_eq!(compute_param_escapes(&[function])["f"], [false]);
}

#[test]
fn async_results_are_not_caller_borrows() {
    let mut function = func_of(
        vec![decl(heap_ty(), true), decl(heap_ty(), false)],
        vec![assign(0, Rvalue::Use(Operand::Copy(Local(1))))],
    );
    function.arg_count = 1;
    assert!(compute_borrowed_returns(&[function.clone()]).contains("f"));
    function.is_async = true;
    assert!(!compute_borrowed_returns(&[function]).contains("f"));
}

#[test]
fn foreign_async_parameters_keep_the_existing_escape_contract() {
    let mut function = func_of(
        vec![
            decl(heap_ty(), true),
            decl(Type::Struct("Foreign".into(), vec![], true), false),
        ],
        vec![assign(
            0,
            Rvalue::Aggregate(AggregateKind::List, vec![Operand::Copy(Local(1))]),
        )],
    );
    function.arg_count = 1;
    function.is_async = true;
    assert_eq!(compute_param_escapes(&[function])["f"], [true]);
}

#[test]
fn reassigned_owned_parameter_starts_initialized() {
    let mut function = func_of(
        vec![decl(Type::Int, true), decl(heap_ty(), true)],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            drop_stmt(1),
        ],
    );
    function.arg_count = 1;
    function.is_async = true;
    pass().run(&mut function);
    let drops = function
        .basic_blocks
        .iter()
        .flat_map(|bb| &bb.statements)
        .filter(|stmt| matches!(stmt.kind, StatementKind::Drop(Local(1))))
        .count();
    assert_eq!(drops, 2);
    assert!(function.basic_blocks[0].statements.iter().any(|stmt| {
        matches!(
            stmt.kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Bool(true))))
        )
    }));
}
