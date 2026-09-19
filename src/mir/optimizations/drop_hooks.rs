use super::ownership::push_local;
use crate::mir::*;
use crate::semantic::types::Type;
use std::collections::HashSet;

/// Returns the set of struct names that define `__drop__`, derived by
/// scanning all function names for the `::__drop__` suffix.
pub fn collect_struct_has_drop(functions: &[MirFunction]) -> HashSet<String> {
    let mut result = HashSet::new();
    for func in functions {
        if let Some(name) = func.name.strip_suffix("::__drop__") {
            result.insert(name.to_string());
        }
    }
    result
}

/// Build the monomorphized name for a struct type, matching the naming
/// convention used by the generic monomorphizer.
pub fn monomorphized_name(struct_name: &str, type_args: &[Type]) -> String {
    if type_args.is_empty() {
        return struct_name.to_string();
    }
    let arg_str = type_args
        .iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join("_")
        .replace("[", "_")
        .replace("]", "_")
        .replace(",", "_")
        .replace(" ", "")
        .replace("->", "_to_")
        .replace("(", "_")
        .replace(")", "_")
        .replace("&", "ref_")
        .replace("*", "ptr_")
        .replace("|", "_or_")
        .replace(":", "_");
    format!("{}_{}", struct_name, arg_str)
}

/// The name of the struct whose `__drop__` we are currently inside, if any.
/// Derived from `func.name` (e.g. `"MyStruct::__drop__"`). Inside a drop
/// handler the struct's own drops are left as ordinary slab frees so that
/// dropping `self` at scope exit does not recurse.
fn drop_self_struct(func: &MirFunction) -> Option<&str> {
    func.name.strip_suffix("::__drop__")
}

/// After the ownership pass, replaces `Drop(local)` with a call to the
/// struct's `__drop__` method for structs that define one. The set of
/// such structs must be provided by `collect_struct_has_drop`.
///
/// A local whose *static* type is the union itself (`Struct | int`, the
/// stdlib's fallible-constructor idiom) needs a different treatment: which
/// arm is live is a runtime question, so the hook is guarded behind
/// `__olive_any_is_struct_box` rather than swapped in unconditionally, and
/// the original `Drop(local)` is left in place as the fallback for every
/// other arm (a scalar sentinel, `None`, or a differently-typed member).
pub fn lower_drop_hooks(func: &mut MirFunction, has_drop: &HashSet<String>) {
    if has_drop.is_empty() {
        return;
    }
    let self_struct = drop_self_struct(func);
    struct DropSite {
        bb: usize,
        idx: usize,
        drop_fn: String,
        local: Local,
    }
    struct UnionDropSite {
        bb: usize,
        idx: usize,
        drop_fn: String,
        local: Local,
        struct_ty: Type,
    }
    struct ListDropSite {
        bb: usize,
        idx: usize,
        helper_fn: String,
        drop_fn: String,
        local: Local,
    }
    let mut sites: Vec<DropSite> = Vec::new();
    let mut union_sites: Vec<UnionDropSite> = Vec::new();
    let mut list_sites: Vec<ListDropSite> = Vec::new();
    for (bb_idx, block) in func.basic_blocks.iter().enumerate() {
        for (idx, stmt) in block.statements.iter().enumerate() {
            let StatementKind::Drop(local) = &stmt.kind else {
                continue;
            };
            match &func.locals[local.0].ty {
                Type::Struct(name, args, _) => {
                    let drop_name = monomorphized_name(name, args);
                    if has_drop.contains(&drop_name) && self_struct != Some(name.as_str()) {
                        sites.push(DropSite {
                            bb: bb_idx,
                            idx,
                            drop_fn: format!("{}::__drop__", drop_name),
                            local: *local,
                        });
                    }
                }
                Type::Union(members) => {
                    let struct_members: Vec<&Type> = members
                        .iter()
                        .filter(|m| matches!(m, Type::Struct(..)))
                        .collect();
                    let [Type::Struct(name, args, _)] = struct_members.as_slice() else {
                        continue;
                    };
                    let drop_name = monomorphized_name(name, args);
                    if has_drop.contains(&drop_name) && self_struct != Some(name.as_str()) {
                        union_sites.push(UnionDropSite {
                            bb: bb_idx,
                            idx,
                            drop_fn: format!("{}::__drop__", drop_name),
                            local: *local,
                            struct_ty: (*struct_members[0]).clone(),
                        });
                    }
                }
                Type::List(elem) => {
                    // Elements with user cleanup get per-element hooks ahead
                    // of the container drop (which then frees the nulled
                    // shell): a union element follows the single-struct rule
                    // above, since only one member's hook can be named.
                    let (drop_name, elem_name, helper_fn) = match elem.as_ref() {
                        Type::Struct(name, args, _) => (
                            monomorphized_name(name, args),
                            name.clone(),
                            "__olive_list_drop_each_struct",
                        ),
                        Type::Union(members) => {
                            let struct_members: Vec<&Type> = members
                                .iter()
                                .filter(|m| matches!(m, Type::Struct(..)))
                                .collect();
                            let [Type::Struct(name, args, _)] = struct_members.as_slice()
                            else {
                                continue;
                            };
                            (
                                monomorphized_name(name, args),
                                name.clone(),
                                "__olive_list_drop_each_union",
                            )
                        }
                        _ => continue,
                    };
                    if has_drop.contains(&drop_name) && self_struct != Some(elem_name.as_str()) {
                        list_sites.push(ListDropSite {
                            bb: bb_idx,
                            idx,
                            helper_fn: helper_fn.to_string(),
                            drop_fn: format!("{}::__drop__", drop_name),
                            local: *local,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    enum AnySite {
        Struct(DropSite),
        Union(UnionDropSite),
        List(ListDropSite),
    }
    let mut all: Vec<(usize, usize, AnySite)> = Vec::new();
    for s in sites {
        all.push((s.bb, s.idx, AnySite::Struct(s)));
    }
    for s in union_sites {
        all.push((s.bb, s.idx, AnySite::Union(s)));
    }
    for s in list_sites {
        all.push((s.bb, s.idx, AnySite::List(s)));
    }
    all.sort_unstable_by_key(|(bb, idx, _)| std::cmp::Reverse((*bb, *idx)));
    for (_, _, site) in all {
        match site {
            AnySite::Struct(s) => {
                insert_struct_drop_hook(func, s.bb, s.idx, s.drop_fn, s.local);
            }
            AnySite::Union(s) => {
                insert_union_drop_hook(func, s.bb, s.idx, s.drop_fn, s.local, s.struct_ty);
            }
            AnySite::List(s) => {
                let tmp = push_local(func, Type::Any);
                let span = func.basic_blocks[s.bb].statements[s.idx].span;
                let helper_stmt = Statement {
                    kind: StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(s.helper_fn)),
                            args: vec![
                                Operand::Copy(s.local),
                                Operand::Constant(Constant::Function(s.drop_fn)),
                            ],
                        },
                    ),
                    span,
                };
                func.basic_blocks[s.bb].statements.insert(s.idx, helper_stmt);
            }
        }
    }
}

/// Splits `bb` at `drop_idx` and inserts, ahead of the untouched original
/// `Drop(local)` now at the head of the continuation block, a branch that
/// peels the struct out (`__olive_struct_unbox_take`, which also frees the
/// box shell), runs its `__drop__`, and zeroes `local` so the fallback
/// `Drop` -- still reached on both paths -- is a safe no-op for the struct
/// case and does its ordinary job for every other arm.
fn insert_union_drop_hook(
    func: &mut MirFunction,
    bb_idx: usize,
    drop_idx: usize,
    drop_fn: String,
    local: Local,
    struct_ty: Type,
) {
    let span = func.basic_blocks[bb_idx].statements[drop_idx].span;

    let tail = func.basic_blocks[bb_idx].statements.split_off(drop_idx);
    let term = func.basic_blocks[bb_idx].terminator.take();

    let cont_id = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: tail,
        terminator: term,
    });

    let inner = push_local(func, struct_ty);
    let drop_sink = push_local(func, Type::Any);
    let struct_stmts = vec![
        Statement {
            kind: StatementKind::Assign(
                inner,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(
                        "__olive_struct_unbox_take".to_string(),
                    )),
                    args: vec![Operand::Copy(local)],
                },
            ),
            span,
        },
        Statement {
            kind: StatementKind::Assign(
                drop_sink,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(drop_fn)),
                    args: vec![Operand::Move(inner)],
                },
            ),
            span,
        },
        Statement {
            kind: StatementKind::Assign(local, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            span,
        },
    ];
    let struct_id = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: struct_stmts,
        terminator: Some(Terminator {
            kind: TerminatorKind::Goto { target: cont_id },
            span,
        }),
    });

    let is_struct = push_local(func, Type::Bool);
    func.basic_blocks[bb_idx].statements.push(Statement {
        kind: StatementKind::Assign(
            is_struct,
            Rvalue::Call {
                func: Operand::Constant(Constant::Function(
                    "__olive_any_is_struct_box".to_string(),
                )),
                args: vec![Operand::Copy(local)],
            },
        ),
        span,
    });
    func.basic_blocks[bb_idx].terminator = Some(Terminator {
        kind: TerminatorKind::SwitchInt {
            discr: Operand::Copy(is_struct),
            targets: vec![(1, struct_id)],
            otherwise: cont_id,
        },
        span,
    });
}

fn insert_struct_drop_hook(
    func: &mut MirFunction,
    bb_idx: usize,
    drop_idx: usize,
    drop_fn: String,
    local: Local,
) {
    let span = func.basic_blocks[bb_idx].statements[drop_idx].span;
    let mut tail = func.basic_blocks[bb_idx].statements.split_off(drop_idx);
    tail.remove(0);
    let term = func.basic_blocks[bb_idx].terminator.take();
    let cont_id = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: tail,
        terminator: term,
    });
    let tmp = push_local(func, Type::Any);
    let drop_stmts = vec![Statement {
        kind: StatementKind::Assign(
            tmp,
            Rvalue::Call {
                func: Operand::Constant(Constant::Function(drop_fn)),
                args: vec![Operand::Move(local)],
            },
        ),
        span,
    }];
    let drop_id = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: drop_stmts,
        terminator: Some(Terminator {
            kind: TerminatorKind::Goto { target: cont_id },
            span,
        }),
    });
    func.basic_blocks[bb_idx].terminator = Some(Terminator {
        kind: TerminatorKind::SwitchInt {
            discr: Operand::Copy(local),
            targets: vec![(0, cont_id)],
            otherwise: drop_id,
        },
        span,
    });
}
