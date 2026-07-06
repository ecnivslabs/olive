use super::*;
use crate::mir::optimizations::Transform;

fn sp() -> Span {
    Span::default()
}

fn heap_ty() -> Type {
    Type::List(Box::new(Type::Int))
}

fn decl(ty: Type, owning: bool) -> LocalDecl {
    LocalDecl {
        ty,
        name: Some("x".into()),
        span: sp(),
        is_mut: true,
        is_owning: owning,
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

fn set_index(obj: usize, val: usize) -> Statement {
    Statement {
        kind: StatementKind::SetIndex(
            Operand::Copy(Local(obj)),
            Operand::Constant(Constant::Int(0)),
            Operand::Copy(Local(val)),
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

fn pass() -> OwnershipInference {
    OwnershipInference {
        borrowed_returns: HashSet::default(),
        param_escapes: HashMap::default(),
    }
}

fn copy_calls(f: &MirFunction) -> usize {
    f.basic_blocks
        .iter()
        .flat_map(|b| &b.statements)
        .filter(|s| {
            matches!(
                &s.kind,
                StatementKind::Assign(_, Rvalue::Call {
                    func: Operand::Constant(Constant::Function(n)),
                    ..
                }) if n == "__olive_copy_typed"
            )
        })
        .count()
}

fn mark_calls(f: &MirFunction) -> usize {
    f.basic_blocks
        .iter()
        .flat_map(|b| &b.statements)
        .filter(|s| {
            matches!(
                &s.kind,
                StatementKind::Assign(_, Rvalue::Call {
                    func: Operand::Constant(Constant::Function(n)),
                    ..
                }) if n == "__olive_alias_mark_cond"
            )
        })
        .count()
}

#[test]
fn alias_of_live_source_becomes_view() {
    // _1 = fresh; _2 = _1; drop(_2); drop(_1)  -- _1 stays live past the
    // alias (its drop is not a use, but the later SetIndex is).
    let mut f = func_of(
        vec![
            decl(Type::Int, true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
        ],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
            Statement {
                kind: StatementKind::SetIndex(
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(0)),
                    Operand::Constant(Constant::Int(5)),
                    false,
                ),
                span: sp(),
            },
            drop_stmt(2),
            drop_stmt(1),
        ],
    );
    pass().run(&mut f);
    assert!(!f.locals[2].is_owning, "alias should be a view");
    let drops: Vec<_> = f.basic_blocks[0]
        .statements
        .iter()
        .filter_map(|s| match &s.kind {
            StatementKind::Drop(l) => Some(*l),
            _ => None,
        })
        .collect();
    assert_eq!(drops, vec![Local(1)], "only the owner keeps its drop");
}

#[test]
fn alias_of_dead_source_transfers() {
    // _1 = fresh; _2 = _1; drop(_2); drop(_1) with no later use of _1.
    let mut f = func_of(
        vec![
            decl(Type::Int, true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
        ],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
            drop_stmt(2),
            drop_stmt(1),
        ],
    );
    pass().run(&mut f);
    assert!(
        matches!(
            &f.basic_blocks[0].statements[1].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Move(l))) if *l == Local(1)
        ),
        "dead source should transfer"
    );
    assert!(f.locals[2].is_owning);
    let drops: Vec<_> = f.basic_blocks[0]
        .statements
        .iter()
        .filter_map(|s| match &s.kind {
            StatementKind::Drop(l) => Some(*l),
            _ => None,
        })
        .collect();
    assert_eq!(
        drops,
        vec![Local(2)],
        "source's drop must not survive a transfer: it would free memory \
         the destination now owns"
    );
}

#[test]
fn str_concat_left_operand_transfer_drops_its_own_source() {
    // `str_concat_inplace` consumes `_1`'s slot, so its trailing drop must not survive.
    let mut f = func_of(
        vec![
            decl(Type::Str, true),
            decl(Type::Str, true),
            decl(Type::Str, true),
        ],
        vec![
            assign(1, Rvalue::Use(Operand::Constant(Constant::Str("a".into())))),
            assign(
                2,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Str("b".into())),
                ),
            ),
            drop_stmt(2),
            drop_stmt(1),
        ],
    );
    pass().run(&mut f);
    assert!(
        matches!(
            &f.basic_blocks[0].statements[1].kind,
            StatementKind::Assign(_, Rvalue::BinaryOp(_, Operand::Move(l), _)) if *l == Local(1)
        ),
        "dead left operand of str concat should move"
    );
    let drops: Vec<_> = f.basic_blocks[0]
        .statements
        .iter()
        .filter_map(|s| match &s.kind {
            StatementKind::Drop(l) => Some(*l),
            _ => None,
        })
        .collect();
    assert_eq!(
        drops,
        vec![Local(2)],
        "left operand's drop must not survive: str_concat_inplace already \
         consumed its slab slot, reused or freed"
    );
}

#[test]
fn element_read_is_view() {
    let mut f = func_of(
        vec![
            decl(Type::Int, true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
        ],
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
            drop_stmt(2),
            drop_stmt(1),
        ],
    );
    pass().run(&mut f);
    assert!(!f.locals[2].is_owning);
    let drop_count = f.basic_blocks[0]
        .statements
        .iter()
        .filter(|s| matches!(s.kind, StatementKind::Drop(_)))
        .count();
    assert_eq!(drop_count, 1);
}

#[test]
fn mixed_local_gets_flag_guard() {
    // _1 owns on one path, borrows from _2 on another.
    let mut f = MirFunction {
        name: "f".into(),
        locals: vec![
            decl(Type::Int, true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
            decl(Type::Bool, true),
        ],
        basic_blocks: vec![
            BasicBlock {
                statements: vec![assign(2, Rvalue::Aggregate(AggregateKind::List, vec![]))],
                terminator: Some(Terminator {
                    kind: TerminatorKind::SwitchInt {
                        discr: Operand::Copy(Local(3)),
                        targets: vec![(1, BasicBlockId(1))],
                        otherwise: BasicBlockId(2),
                    },
                    span: sp(),
                }),
            },
            BasicBlock {
                statements: vec![assign(1, Rvalue::Aggregate(AggregateKind::List, vec![]))],
                terminator: Some(Terminator {
                    kind: TerminatorKind::Goto {
                        target: BasicBlockId(3),
                    },
                    span: sp(),
                }),
            },
            BasicBlock {
                statements: vec![assign(1, Rvalue::Use(Operand::Copy(Local(2))))],
                terminator: Some(Terminator {
                    kind: TerminatorKind::Goto {
                        target: BasicBlockId(3),
                    },
                    span: sp(),
                }),
            },
            BasicBlock {
                statements: vec![
                    Statement {
                        kind: StatementKind::SetIndex(
                            Operand::Copy(Local(2)),
                            Operand::Constant(Constant::Int(0)),
                            Operand::Constant(Constant::Int(1)),
                            false,
                        ),
                        span: sp(),
                    },
                    drop_stmt(1),
                    drop_stmt(2),
                ],
                terminator: Some(Terminator {
                    kind: TerminatorKind::Return,
                    span: sp(),
                }),
            },
        ],
        arg_count: 1,
        vararg_idx: None,
        kwarg_idx: None,
        param_names: vec![],
        is_async: false,
    };
    pass().run(&mut f);
    // The drop of _1 must now be guarded: reachable only through a
    // SwitchInt on the shadow flag.
    let guarded = f.basic_blocks.iter().any(|bb| {
        matches!(
            &bb.terminator,
            Some(Terminator {
                kind: TerminatorKind::SwitchInt { discr: Operand::Copy(d), .. },
                ..
            }) if d.0 >= 4
        )
    });
    assert!(guarded, "mixed local drop should be flag-guarded");
}

#[test]
fn returned_view_root_drop_removed() {
    // _1 = fresh; _2 = _1 (view: _1 used later); _0 = _2; drop(_1); return
    let mut f = func_of(
        vec![
            decl(heap_ty(), true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
        ],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
            Statement {
                kind: StatementKind::SetIndex(
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(0)),
                    Operand::Constant(Constant::Int(1)),
                    false,
                ),
                span: sp(),
            },
            assign(0, Rvalue::Use(Operand::Copy(Local(2)))),
            drop_stmt(1),
        ],
    );
    pass().run(&mut f);
    let any_drop = f
        .basic_blocks
        .iter()
        .flat_map(|b| &b.statements)
        .any(|s| matches!(s.kind, StatementKind::Drop(_)));
    assert!(!any_drop, "returning the only view of _1 must not drop _1");
}

#[test]
fn returned_interior_element_is_copied_not_root_drop_elided() {
    // _2 is an interior view into _1; _1 must still drop, so the returned element needs its own copy.
    let mut f = func_of(
        vec![
            decl(heap_ty(), true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
        ],
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
            assign(0, Rvalue::Use(Operand::Copy(Local(2)))),
            drop_stmt(1),
        ],
    );
    pass().run(&mut f);
    assert_eq!(copy_calls(&f), 1, "interior return copies before root drop");
    let root_drop_survives = f
        .basic_blocks
        .iter()
        .flat_map(|b| &b.statements)
        .any(|s| matches!(s.kind, StatementKind::Drop(l) if l == Local(1)));
    assert!(root_drop_survives, "container must still be freed");
}

#[test]
fn lone_dead_store_keeps_source_a_plain_owner() {
    // _2 = fresh; _1[0] = _2; drop(_1)  -- _2 dead after the store, so move
    // elision transfers it; no flag, no mark.
    let mut f = func_of(
        vec![
            decl(Type::Int, true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
        ],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(2, Rvalue::Aggregate(AggregateKind::List, vec![])),
            set_index(1, 2),
            drop_stmt(2),
            drop_stmt(1),
        ],
    );
    pass().run(&mut f);
    assert_eq!(copy_calls(&f), 0, "dead-pure transfer must not copy");
    assert_eq!(mark_calls(&f), 0, "clean transfer must not mark");
    let guarded = f.basic_blocks.iter().any(|bb| {
        matches!(
            &bb.terminator,
            Some(Terminator {
                kind: TerminatorKind::SwitchInt { .. },
                ..
            })
        )
    });
    assert!(!guarded, "lone dead store needs no dynamic ownership");
}

#[test]
fn live_after_store_becomes_dynamic_and_copies() {
    // _2 = fresh; _1[0] = _2; _2[0] = 9; drop(_2); drop(_1) -- _2 lives past
    // the escape, so it gets a deep copy instead of a mark.
    let mut f = func_of(
        vec![
            decl(Type::Int, true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
        ],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(2, Rvalue::Aggregate(AggregateKind::List, vec![])),
            set_index(1, 2),
            Statement {
                kind: StatementKind::SetIndex(
                    Operand::Copy(Local(2)),
                    Operand::Constant(Constant::Int(0)),
                    Operand::Constant(Constant::Int(9)),
                    false,
                ),
                span: sp(),
            },
            drop_stmt(2),
            drop_stmt(1),
        ],
    );
    pass().run(&mut f);
    assert_eq!(copy_calls(&f), 1, "live-after store must deep-copy");
    assert_eq!(mark_calls(&f), 0, "no alias marks remain");
    let guarded = f.basic_blocks.iter().any(|bb| {
        matches!(
            &bb.terminator,
            Some(Terminator {
                kind: TerminatorKind::SwitchInt { discr: Operand::Copy(d), .. },
                ..
            }) if d.0 >= 3
        )
    });
    assert!(guarded, "escaped-then-used owner drop must be flag-guarded");
}

#[test]
fn lone_dead_call_arg_upgrades_to_move() {
    // _2 = fresh; append(_1, _2); drop(_2); drop(_1) -- the only escape of a
    // pure owner at its last use transfers outright.
    let mut f = func_of(
        vec![
            decl(Type::Int, true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
        ],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(2, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(
                0,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_append".into())),
                    args: vec![Operand::Copy(Local(1)), Operand::Copy(Local(2))],
                },
            ),
            drop_stmt(2),
            drop_stmt(1),
        ],
    );
    pass().run(&mut f);
    assert!(
        matches!(
            &f.basic_blocks[0].statements[2].kind,
            StatementKind::Assign(_, Rvalue::Call { args, .. })
                if matches!(args[1], Operand::Move(l) if l == Local(2))
        ),
        "lone dead escaping arg should become a move"
    );
    assert_eq!(mark_calls(&f), 0);
}

#[test]
fn twice_escaped_call_arg_gets_two_copies() {
    // append(_1, _3); append(_2, _3) with _3 dead only at the second: both
    // sites are dynamic escapes, each gets a deep copy.
    let mut f = func_of(
        vec![
            decl(Type::Int, true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
        ],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(2, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(3, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(
                0,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_append".into())),
                    args: vec![Operand::Copy(Local(1)), Operand::Copy(Local(3))],
                },
            ),
            assign(
                0,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_append".into())),
                    args: vec![Operand::Copy(Local(2)), Operand::Copy(Local(3))],
                },
            ),
            drop_stmt(3),
            drop_stmt(2),
            drop_stmt(1),
        ],
    );
    pass().run(&mut f);
    assert_eq!(
        copy_calls(&f),
        2,
        "both escapes of a doubly-stored value must deep-copy"
    );
    assert_eq!(mark_calls(&f), 0, "no alias marks remain");
}

#[test]
fn view_store_is_deep_copied() {
    // _2 = _1 (view); _3[0] = _2  -- the view never owns, so the store must
    // deep-copy it into the container instead of aliasing or marking.
    let mut f = func_of(
        vec![
            decl(Type::Int, true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
            decl(heap_ty(), true),
        ],
        vec![
            assign(1, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(3, Rvalue::Aggregate(AggregateKind::List, vec![])),
            assign(2, Rvalue::Use(Operand::Copy(Local(1)))),
            set_index(3, 2),
            Statement {
                kind: StatementKind::SetIndex(
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(0)),
                    Operand::Constant(Constant::Int(1)),
                    false,
                ),
                span: sp(),
            },
            drop_stmt(1),
            drop_stmt(3),
        ],
    );
    pass().run(&mut f);
    assert!(!f.locals[2].is_owning, "alias should stay a view");
    let stmts: Vec<_> = f.basic_blocks.iter().flat_map(|b| &b.statements).collect();

    let copy_tmp = stmts.iter().find_map(|s| match &s.kind {
        StatementKind::Assign(
            dst,
            Rvalue::Call {
                func: Operand::Constant(Constant::Function(n)),
                args,
            },
        ) if n == "__olive_copy_typed"
            && matches!(args.as_slice(), [Operand::Copy(l)] if *l == Local(2)) =>
        {
            Some(*dst)
        }
        _ => None,
    });
    let copy_tmp = copy_tmp.expect("view store must be deep-copied");

    assert!(
        stmts.iter().any(|s| matches!(
            &s.kind,
            StatementKind::SetIndex(Operand::Copy(c), _, Operand::Move(v), _)
                if *c == Local(3) && *v == copy_tmp
        )),
        "the container stores the copy, not the view"
    );
    assert!(
        !stmts.iter().any(|s| matches!(
            &s.kind,
            StatementKind::Assign(_, Rvalue::Call {
                func: Operand::Constant(Constant::Function(n)),
                ..
            }) if n == "__olive_alias_mark_cond"
        )),
        "a copied view needs no mark"
    );
}
