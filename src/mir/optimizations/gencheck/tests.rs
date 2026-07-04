use super::*;
use crate::mir::optimizations::Transform;

fn sp() -> Span {
    Span::default()
}

fn heap_ty() -> Type {
    Type::List(Box::new(Type::Int))
}

fn decl(ty: Type) -> LocalDecl {
    LocalDecl {
        ty,
        name: Some("x".into()),
        span: sp(),
        is_mut: true,
        is_owning: true,
    }
}

fn assign(l: usize, rv: Rvalue) -> Statement {
    Statement {
        kind: StatementKind::Assign(Local(l), rv),
        span: sp(),
    }
}

fn drop_stmt(l: usize) -> Statement {
    Statement {
        kind: StatementKind::Drop(Local(l)),
        span: sp(),
    }
}

fn use_stmt(obj: usize) -> Statement {
    Statement {
        kind: StatementKind::SetIndex(
            Operand::Copy(Local(obj)),
            Operand::Constant(Constant::Int(0)),
            Operand::Constant(Constant::Int(1)),
            false,
        ),
        span: sp(),
    }
}

fn func_of(locals: Vec<LocalDecl>, stmts: Vec<Statement>) -> MirFunction {
    MirFunction {
        name: "f".into(),
        locals,
        basic_blocks: vec![BasicBlock {
            statements: stmts,
            terminator: Some(Terminator {
                kind: TerminatorKind::Return,
                span: sp(),
            }),
        }],
        arg_count: 0,
        vararg_idx: None,
        kwarg_idx: None,
        param_names: vec![],
        is_async: false,
    }
}

fn pass() -> GenCheckInsertion {
    GenCheckInsertion {
        borrowed_returns: HashSet::default(),
        param_escapes: HashMap::default(),
    }
}

fn count_checks(f: &MirFunction) -> usize {
    f.basic_blocks
        .iter()
        .flat_map(|b| &b.statements)
        .filter(|s| matches!(s.kind, StatementKind::GenCheck { .. }))
        .count()
}

#[test]
fn owner_only_function_gets_no_checks() {
    let mut f = func_of(
        vec![decl(Type::Int), decl(heap_ty())],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            use_stmt(1),
            drop_stmt(1),
        ],
    );
    assert!(!pass().run(&mut f));
    assert_eq!(count_checks(&f), 0);
}

#[test]
fn view_use_after_root_drop_is_checked() {
    // _2 = _1; drop(_1); use _2  -- proof dies at the drop.
    let mut f = func_of(
        vec![decl(Type::Int), decl(heap_ty()), decl(heap_ty())],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
            drop_stmt(1),
            use_stmt(2),
        ],
    );
    assert!(pass().run(&mut f));
    let stmts = &f.basic_blocks[0].statements;
    let check_pos = stmts
        .iter()
        .position(|s| matches!(s.kind, StatementKind::GenCheck { value, .. } if value == Local(2)))
        .expect("check for _2");
    let use_pos = stmts
        .iter()
        .position(|s| matches!(&s.kind, StatementKind::SetIndex(Operand::Copy(l), _, _, _) if *l == Local(2)))
        .unwrap();
    assert!(check_pos < use_pos, "check must precede the stale use");
}

#[test]
fn str_view_after_root_drop_is_checked() {
    // A string view outliving its owner's drop is a suspect too, and its
    // capture goes through the literal-tolerant helper (via codegen).
    let mut f = func_of(
        vec![decl(Type::Int), decl(Type::Str), decl(Type::Str)],
        vec![
            assign(1, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
            drop_stmt(1),
            use_stmt(2),
        ],
    );
    assert!(pass().run(&mut f));
    let stmts = &f.basic_blocks[0].statements;
    assert!(
        stmts
            .iter()
            .any(|s| matches!(s.kind, StatementKind::GenCheck { value, .. } if value == Local(2))),
        "string view gets a stale check"
    );
    assert!(
        stmts
            .iter()
            .any(|s| matches!(&s.kind, StatementKind::Assign(_, Rvalue::GenOf(_)))),
        "string view captures a generation"
    );
}

#[test]
fn view_use_before_root_drop_is_elided() {
    let mut f = func_of(
        vec![decl(Type::Int), decl(heap_ty()), decl(heap_ty())],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
            use_stmt(2),
            drop_stmt(1),
        ],
    );
    pass().run(&mut f);
    assert_eq!(count_checks(&f), 0, "use while owner lives needs no check");
}

#[test]
fn unrelated_drop_does_not_kill_proof() {
    let mut f = func_of(
        vec![
            decl(Type::Int),
            decl(heap_ty()),
            decl(heap_ty()),
            decl(heap_ty()),
        ],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(3, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
            drop_stmt(3),
            use_stmt(2),
            drop_stmt(1),
        ],
    );
    pass().run(&mut f);
    assert_eq!(count_checks(&f), 0);
}

#[test]
fn element_borrow_validates_at_birth_only_when_shared() {
    let stmts = |_: ()| {
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(
                2,
                Rvalue::GetIndex(
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(0)),
                    false,
                ),
            ),
            use_stmt(2),
            drop_stmt(1),
        ]
    };
    let locals = vec![
        decl(Type::Int),
        decl(Type::List(Box::new(heap_ty()))),
        decl(heap_ty()),
    ];

    let mut clean = func_of(locals.clone(), stmts(()));
    pass().run(&mut clean);
    assert_eq!(count_checks(&clean), 0, "no sharing, container alive");
}

#[test]
fn unsafe_call_kills_unknown_rooted_proof() {
    // _2 = borrowed-return call; g(); use _2 -> checked.
    let mut br = HashSet::default();
    br.insert("borrower".to_string());
    let p = GenCheckInsertion {
        borrowed_returns: br,
        param_escapes: HashMap::default(),
    };
    let mut f = func_of(
        vec![decl(Type::Int), decl(heap_ty()), decl(heap_ty())],
        vec![
            assign(
                2,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("borrower".into())),
                    args: vec![],
                },
            ),
            assign(
                1,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("other".into())),
                    args: vec![],
                },
            ),
            use_stmt(2),
        ],
    );
    assert!(p.run(&mut f));
    assert_eq!(count_checks(&f), 1);
}

#[test]
fn safe_runtime_call_preserves_proof() {
    let mut br = HashSet::default();
    br.insert("borrower".to_string());
    let p = GenCheckInsertion {
        borrowed_returns: br,
        param_escapes: HashMap::default(),
    };
    let mut f = func_of(
        vec![decl(Type::Int), decl(heap_ty()), decl(heap_ty())],
        vec![
            assign(
                2,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("borrower".into())),
                    args: vec![],
                },
            ),
            use_stmt(2),
            assign(
                1,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_append".into())),
                    args: vec![Operand::Copy(Local(2)), Operand::Constant(Constant::Int(1))],
                },
            ),
            use_stmt(2),
        ],
    );
    p.run(&mut f);
    assert_eq!(count_checks(&f), 0, "runtime helpers never free headers");
}

#[test]
fn loop_reproof_via_back_edge() {
    // Preheader defines the view; loop body uses it with no kills: the
    // must-analysis keeps it proven, so the body stays check-free.
    let mut f = MirFunction {
        name: "f".into(),
        locals: vec![decl(Type::Int), decl(heap_ty()), decl(heap_ty())],
        basic_blocks: vec![
            BasicBlock {
                statements: vec![
                    assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
                    assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
                ],
                terminator: Some(Terminator {
                    kind: TerminatorKind::Goto {
                        target: BasicBlockId(1),
                    },
                    span: sp(),
                }),
            },
            BasicBlock {
                statements: vec![use_stmt(2)],
                terminator: Some(Terminator {
                    kind: TerminatorKind::SwitchInt {
                        discr: Operand::Constant(Constant::Int(0)),
                        targets: vec![(1, BasicBlockId(1))],
                        otherwise: BasicBlockId(2),
                    },
                    span: sp(),
                }),
            },
            BasicBlock {
                statements: vec![drop_stmt(1)],
                terminator: Some(Terminator {
                    kind: TerminatorKind::Return,
                    span: sp(),
                }),
            },
        ],
        arg_count: 0,
        vararg_idx: None,
        kwarg_idx: None,
        param_names: vec![],
        is_async: false,
    };
    pass().run(&mut f);
    assert_eq!(count_checks(&f), 0, "owner outlives the loop");
}

#[test]
fn escaped_param_is_checked_after_call() {
    // p passed to an escaping callee, then used: the callee may have
    // stored and freed it.
    let mut pe = HashMap::default();
    pe.insert("sink".to_string(), vec![true]);
    let p = GenCheckInsertion {
        borrowed_returns: HashSet::default(),
        param_escapes: pe,
    };
    let mut f = func_of(
        vec![decl(Type::Int), decl(heap_ty())],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(
                0,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("sink".into())),
                    args: vec![Operand::Copy(Local(1))],
                },
            ),
            use_stmt(1),
        ],
    );
    assert!(p.run(&mut f));
    assert_eq!(count_checks(&f), 1, "escaped value re-verified after call");
}

#[test]
fn non_ffi_struct_is_checked() {
    let mut f = func_of(
        vec![
            decl(Type::Int),
            decl(Type::Struct("MyStruct".into(), vec![], false)),
            decl(Type::Struct("MyStruct".into(), vec![], false)),
        ],
        vec![
            assign(1, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
            drop_stmt(1),
            use_stmt(2),
        ],
    );
    assert!(pass().run(&mut f));
    assert_eq!(count_checks(&f), 1, "user struct gets a check");
}

#[test]
fn ffi_struct_is_not_checked() {
    let mut f = func_of(
        vec![
            decl(Type::Int),
            decl(Type::Struct("CStruct".into(), vec![], true)),
            decl(Type::Struct("CStruct".into(), vec![], true)),
        ],
        vec![
            assign(1, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
            drop_stmt(1),
            use_stmt(2),
        ],
    );
    assert!(!pass().run(&mut f));
    assert_eq!(count_checks(&f), 0, "ffi struct has no generation checking");
}
