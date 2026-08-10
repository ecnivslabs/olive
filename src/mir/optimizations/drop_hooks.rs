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

/// (base struct name, monomorphized name) pairs for every `__drop__`-owning
/// struct reachable from any local's type (direct, union, and container
/// element positions alike) or any enum variant payload. The driver
/// registers both spellings in the program entries' prologues so the
/// runtime registry matches whatever name form a descriptor carries at the
/// free site.
pub fn collect_drop_registrations(
    functions: &[MirFunction],
    has_drop: &HashSet<String>,
    enum_defs: &rustc_hash::FxHashMap<String, Vec<(String, Vec<Type>)>>,
) -> Vec<(String, String)> {
    fn visit(ty: &Type, has_drop: &HashSet<String>, out: &mut HashSet<(String, String)>) {
        match ty {
            Type::Struct(name, args, _) => {
                let mono = monomorphized_name(name, args);
                if has_drop.contains(&mono) {
                    out.insert((name.clone(), mono));
                }
            }
            Type::Union(members) | Type::Tuple(members) => {
                for m in members {
                    visit(m, has_drop, out);
                }
            }
            Type::List(e) | Type::Set(e) => visit(e, has_drop, out),
            Type::Dict(k, v) => {
                visit(k, has_drop, out);
                visit(v, has_drop, out);
            }
            Type::Ref(e) | Type::MutRef(e) | Type::Ptr(e) => visit(e, has_drop, out),
            _ => {}
        }
    }
    let mut pairs = HashSet::new();
    for func in functions {
        for local in &func.locals {
            visit(&local.ty, has_drop, &mut pairs);
        }
    }
    // Variant payloads never appear in a local's own type, so a struct used
    // only there (e.g. a resource smuggled through an `Any`-typed slot)
    // would otherwise miss registration entirely.
    for variants in enum_defs.values() {
        for (_, payloads) in variants {
            for payload in payloads {
                visit(payload, has_drop, &mut pairs);
            }
        }
    }
    let mut sorted: Vec<(String, String)> = pairs.into_iter().collect();
    sorted.sort();
    sorted
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

struct TupleDropSite {
    bb: usize,
    idx: usize,
    local: Local,
    elems: Vec<TupleElem>,
}

struct TupleElem {
    pos: usize,
    drop_fn: String,
    is_union: bool,
    ty: Type,
}

/// Names the `__drop__` for a struct or single-struct-union element type:
/// `(drop_name, struct_name, is_union, struct_ty)`. A union follows the
/// single-struct rule (only one member's hook can be named); anything else
/// has no hook.
fn hook_target(ty: &Type) -> Option<(String, String, bool, Type)> {
    match ty {
        Type::Struct(name, args, _) => Some((
            monomorphized_name(name, args),
            name.clone(),
            false,
            ty.clone(),
        )),
        Type::Union(members) => {
            let struct_members: Vec<&Type> = members
                .iter()
                .filter(|m| matches!(m, Type::Struct(..)))
                .collect();
            let [Type::Struct(name, args, _)] = struct_members.as_slice() else {
                return None;
            };
            Some((
                monomorphized_name(name, args),
                name.clone(),
                true,
                (*struct_members[0]).clone(),
            ))
        }
        _ => None,
    }
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
    let mut sites: Vec<DropSite> = Vec::new();
    let mut union_sites: Vec<UnionDropSite> = Vec::new();
    let mut list_sites: Vec<ListDropSite> = Vec::new();
    let mut tuple_sites: Vec<TupleDropSite> = Vec::new();
    for (bb_idx, block) in func.basic_blocks.iter().enumerate() {
        for (idx, stmt) in block.statements.iter().enumerate() {
            let StatementKind::Drop(local) = &stmt.kind else {
                continue;
            };
            match &func.locals[local.0].ty {
                Type::Struct(..) | Type::Union(..) => {
                    let ty = func.locals[local.0].ty.clone();
                    let Some((drop_name, elem_name, is_union, struct_ty)) = hook_target(&ty) else {
                        continue;
                    };
                    if !has_drop.contains(&drop_name) || self_struct == Some(elem_name.as_str()) {
                        continue;
                    }
                    if is_union {
                        union_sites.push(UnionDropSite {
                            bb: bb_idx,
                            idx,
                            drop_fn: format!("{}::__drop__", drop_name),
                            local: *local,
                            struct_ty,
                        });
                    } else {
                        sites.push(DropSite {
                            bb: bb_idx,
                            idx,
                            drop_fn: format!("{}::__drop__", drop_name),
                            local: *local,
                        });
                    }
                }
                Type::List(elem) => {
                    // Elements with user cleanup get per-element hooks ahead
                    // of the container drop (which then frees the nulled
                    // shell): a union element follows the single-struct rule
                    // above, since only one member's hook can be named.
                    let Some((drop_name, elem_name, is_union, _)) = hook_target(elem) else {
                        continue;
                    };
                    if has_drop.contains(&drop_name) && self_struct != Some(elem_name.as_str()) {
                        list_sites.push(ListDropSite {
                            bb: bb_idx,
                            idx,
                            helper_fn: if is_union {
                                "__olive_list_drop_each_union"
                            } else {
                                "__olive_list_drop_each_struct"
                            }
                            .to_string(),
                            drop_fn: format!("{}::__drop__", drop_name),
                            local: *local,
                        });
                    }
                }
                Type::Tuple(items) => {
                    // Positions each carry their own static type (and hook),
                    // so hooks unroll per index instead of sharing one
                    // whole-container pass: struct positions call the hook
                    // directly, union positions route through the guarded
                    // single-word helper. The container `Drop` follows and
                    // frees the consumed slots through the generation guard.
                    let mut elems = Vec::new();
                    for (pos, item) in items.iter().enumerate() {
                        let Some((drop_name, elem_name, is_union, _)) = hook_target(item) else {
                            continue;
                        };
                        if has_drop.contains(&drop_name) && self_struct != Some(elem_name.as_str())
                        {
                            elems.push(TupleElem {
                                pos,
                                drop_fn: format!("{}::__drop__", drop_name),
                                is_union,
                                ty: item.clone(),
                            });
                        }
                    }
                    if !elems.is_empty() {
                        tuple_sites.push(TupleDropSite {
                            bb: bb_idx,
                            idx,
                            local: *local,
                            elems,
                        });
                    }
                }
                Type::Dict(_, val) => {
                    // Only values can own resources (keys are interned
                    // strings or scalars); hooked arms are zeroed in place so
                    // the dict drop that follows frees keys alone.
                    let Some((drop_name, elem_name, is_union, _)) = hook_target(val) else {
                        continue;
                    };
                    if has_drop.contains(&drop_name) && self_struct != Some(elem_name.as_str()) {
                        list_sites.push(ListDropSite {
                            bb: bb_idx,
                            idx,
                            helper_fn: if is_union {
                                "__olive_dict_drop_each_union"
                            } else {
                                "__olive_dict_drop_each_struct"
                            }
                            .to_string(),
                            drop_fn: format!("{}::__drop__", drop_name),
                            local: *local,
                        });
                    }
                }
                Type::Set(elem) => {
                    let Some((drop_name, elem_name, is_union, _)) = hook_target(elem) else {
                        continue;
                    };
                    if has_drop.contains(&drop_name) && self_struct != Some(elem_name.as_str()) {
                        list_sites.push(ListDropSite {
                            bb: bb_idx,
                            idx,
                            helper_fn: if is_union {
                                "__olive_set_drop_each_union"
                            } else {
                                "__olive_set_drop_each_struct"
                            }
                            .to_string(),
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
        Tuple(TupleDropSite),
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
    for s in tuple_sites {
        all.push((s.bb, s.idx, AnySite::Tuple(s)));
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
                func.basic_blocks[s.bb]
                    .statements
                    .insert(s.idx, helper_stmt);
            }
            AnySite::Tuple(s) => {
                insert_tuple_drop_hook(func, s.bb, s.idx, s.local, s.elems);
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

/// Unrolled per-position hooks for a tuple whose static element types carry
/// `__drop__`, ahead of the untouched original `Drop(local)` (which frees
/// the shell). Struct positions call the hook directly; union positions
/// route through the guarded single-word helper. The whole sequence is
/// skipped when the tuple word itself is null (a moved-from temp): indexing
/// null would fault, while every other free path no-ops on it.
fn insert_tuple_drop_hook(
    func: &mut MirFunction,
    bb_idx: usize,
    drop_idx: usize,
    local: Local,
    elems: Vec<TupleElem>,
) {
    let span = func.basic_blocks[bb_idx].statements[drop_idx].span;
    let tail = func.basic_blocks[bb_idx].statements.split_off(drop_idx);
    let term = func.basic_blocks[bb_idx].terminator.take();
    let cont_id = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: tail,
        terminator: term,
    });
    let mut hook_stmts = Vec::new();
    for elem in &elems {
        let elem_tmp = push_local(func, elem.ty.clone());
        hook_stmts.push(Statement {
            kind: StatementKind::Assign(
                elem_tmp,
                Rvalue::GetIndex(
                    Operand::Copy(local),
                    Operand::Constant(Constant::Int(elem.pos as i64)),
                    false,
                ),
            ),
            span,
        });
        let sink = push_local(func, Type::Any);
        if elem.is_union {
            hook_stmts.push(Statement {
                kind: StatementKind::Assign(
                    sink,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_hook_union_word".to_string(),
                        )),
                        args: vec![
                            Operand::Move(elem_tmp),
                            Operand::Constant(Constant::Function(elem.drop_fn.clone())),
                        ],
                    },
                ),
                span,
            });
        } else {
            hook_stmts.push(Statement {
                kind: StatementKind::Assign(
                    sink,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(elem.drop_fn.clone())),
                        args: vec![Operand::Move(elem_tmp)],
                    },
                ),
                span,
            });
        }
    }
    let hook_id = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: hook_stmts,
        terminator: Some(Terminator {
            kind: TerminatorKind::Goto { target: cont_id },
            span,
        }),
    });
    func.basic_blocks[bb_idx].terminator = Some(Terminator {
        kind: TerminatorKind::SwitchInt {
            discr: Operand::Copy(local),
            targets: vec![(0, cont_id)],
            otherwise: hook_id,
        },
        span,
    });
}
