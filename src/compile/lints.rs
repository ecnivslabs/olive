use super::errors::{Diagnostic, Sources};
use crate::mir::ir::AggregateKind;
use crate::mir::{Constant, MirFunction, Operand, Rvalue, StatementKind};
use rustc_hash::FxHashMap as HashMap;

/// Reports indexing past the end of a list whose length is known at compile
/// time, turning a guaranteed runtime panic into a compile error. Conservative
/// by construction: a list is only considered fixed-length when it is built
/// once from a literal and never reassigned, written through, or handed to a
/// function that might grow it. Anything less certain is left to the runtime
/// bounds check. Returns true if any error was reported.
pub fn check_const_index_bounds(funcs: &[MirFunction], sources: &Sources) -> bool {
    let mut had_error = false;
    for func in funcs {
        let lens = fixed_length_lists(func);
        if lens.is_empty() {
            continue;
        }
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                let StatementKind::Assign(_, Rvalue::GetIndex(obj, idx, _)) = &stmt.kind else {
                    continue;
                };
                let Some(origin) = operand_local(obj).map(|l| resolve_origin(func, l)) else {
                    continue;
                };
                let Some(&len) = lens.get(&origin) else {
                    continue;
                };
                let Operand::Constant(Constant::Int(i)) = idx else {
                    continue;
                };
                let effective = if *i < 0 { len as i64 + *i } else { *i };
                if effective >= 0 && (effective as usize) < len {
                    continue;
                }
                had_error = true;
                Diagnostic::error(
                    "E0006",
                    format!("index {i} is out of bounds for a list of length {len}"),
                    stmt.span,
                )
                .label("index is known to be out of range here")
                .note(format!("the list always has exactly {len} elements"))
                .help("use a valid index, or guard the access with a length check")
                .emit(sources);
            }
        }
    }
    had_error
}

/// Follows single-step `Use`/`Copy`/`Move` assignment chains back to the local
/// that originally defined the value. Used so a list indexed through compiler
/// temporaries is still recognised as the same fixed-length list.
fn resolve_origin(func: &MirFunction, local: usize) -> usize {
    let defs = single_use_defs(func);
    let mut cur = local;
    let mut guard = 0;
    while let Some(&src) = defs.get(&cur) {
        if src == cur || guard > func.locals.len() {
            break;
        }
        cur = src;
        guard += 1;
    }
    cur
}

/// Maps a local to its source local when it is assigned exactly once from a
/// plain `Use`/`Copy`/`Move` of another local.
fn single_use_defs(func: &MirFunction) -> HashMap<usize, usize> {
    let mut assigns: HashMap<usize, usize> = HashMap::default();
    let mut src_of: HashMap<usize, usize> = HashMap::default();
    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            if let StatementKind::Assign(dst, rval) = &stmt.kind {
                *assigns.entry(dst.0).or_insert(0) += 1;
                if let Rvalue::Use(op) = rval
                    && let Some(src) = operand_local(op)
                {
                    src_of.insert(dst.0, src);
                }
            }
        }
    }
    src_of
        .into_iter()
        .filter(|(dst, _)| assigns.get(dst).copied().unwrap_or(0) == 1)
        .collect()
}

/// Locals that hold a list of statically known length: assigned exactly once
/// from a list literal, and never mutated, reassigned, or passed to a call.
fn fixed_length_lists(func: &MirFunction) -> HashMap<usize, usize> {
    let mut len_of: HashMap<usize, usize> = HashMap::default();
    let mut assigns: HashMap<usize, usize> = HashMap::default();
    let mut disqualified: std::collections::HashSet<usize> = std::collections::HashSet::default();
    let disq = |op: &Operand, set: &mut std::collections::HashSet<usize>| {
        if let Some(l) = operand_local(op) {
            set.insert(resolve_origin(func, l));
        }
    };

    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            match &stmt.kind {
                StatementKind::Assign(dst, rval) => {
                    *assigns.entry(dst.0).or_insert(0) += 1;
                    match rval {
                        Rvalue::Aggregate(AggregateKind::List, ops) => {
                            len_of.insert(dst.0, ops.len());
                        }
                        Rvalue::Call { args, .. } => {
                            for a in args {
                                disq(a, &mut disqualified);
                            }
                        }
                        // A reference into the list can be written through later,
                        // so a borrowed list is no longer provably fixed-length.
                        Rvalue::Ref(l) | Rvalue::MutRef(l) => {
                            disqualified.insert(resolve_origin(func, l.0));
                        }
                        _ => {}
                    }
                }
                StatementKind::SetIndex(obj, _, val, _) => {
                    disq(obj, &mut disqualified);
                    disq(val, &mut disqualified);
                }
                StatementKind::SetAttr(obj, _, val) => {
                    disq(obj, &mut disqualified);
                    disq(val, &mut disqualified);
                }
                _ => {}
            }
        }
    }

    len_of
        .into_iter()
        .filter(|(local, _)| {
            assigns.get(local).copied().unwrap_or(0) == 1 && !disqualified.contains(local)
        })
        .collect()
}

fn operand_local(op: &Operand) -> Option<usize> {
    match op {
        Operand::Copy(l) | Operand::Move(l) => Some(l.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::ir::{BasicBlock, Local, Statement};
    use crate::span::Span;

    fn func_with(stmts: Vec<Statement>) -> MirFunction {
        MirFunction {
            name: "t".into(),
            locals: vec![],
            basic_blocks: vec![BasicBlock {
                statements: stmts,
                terminator: None,
            }],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    fn list_assign(dst: usize, n: usize) -> Statement {
        Statement {
            kind: StatementKind::Assign(
                Local(dst),
                Rvalue::Aggregate(
                    AggregateKind::List,
                    (0..n)
                        .map(|i| Operand::Constant(Constant::Int(i as i64)))
                        .collect(),
                ),
            ),
            span: Span::default(),
        }
    }

    fn index_assign(dst: usize, src: usize, i: i64) -> Statement {
        Statement {
            kind: StatementKind::Assign(
                Local(dst),
                Rvalue::GetIndex(
                    Operand::Copy(Local(src)),
                    Operand::Constant(Constant::Int(i)),
                    false,
                ),
            ),
            span: Span::default(),
        }
    }

    #[test]
    fn flags_constant_oob() {
        let f = func_with(vec![list_assign(0, 3), index_assign(1, 0, 5)]);
        let lens = fixed_length_lists(&f);
        assert_eq!(lens.get(&0), Some(&3));
    }

    #[test]
    fn in_range_index_not_flagged() {
        let f = func_with(vec![list_assign(0, 3), index_assign(1, 0, 2)]);
        assert!(!check_const_index_bounds(&[f], &Sources::default()));
    }

    #[test]
    fn out_of_range_index_is_an_error() {
        let f = func_with(vec![list_assign(0, 3), index_assign(1, 0, 5)]);
        assert!(check_const_index_bounds(&[f], &Sources::default()));
    }

    #[test]
    fn negative_index_wraps() {
        let f = func_with(vec![list_assign(0, 3), index_assign(1, 0, -1)]);
        assert!(!check_const_index_bounds(&[f], &Sources::default()));
    }

    #[test]
    fn negative_index_too_small_is_error() {
        let f = func_with(vec![list_assign(0, 3), index_assign(1, 0, -4)]);
        assert!(check_const_index_bounds(&[f], &Sources::default()));
    }

    #[test]
    fn index_through_aliased_temp_is_resolved() {
        // origin (local 0) is the literal; local 2 is a `Use` copy of it, indexed OOB.
        let copy = Statement {
            kind: StatementKind::Assign(Local(2), Rvalue::Use(Operand::Copy(Local(0)))),
            span: Span::default(),
        };
        let f = func_with(vec![list_assign(0, 3), copy, index_assign(1, 2, 9)]);
        assert!(check_const_index_bounds(&[f], &Sources::default()));
    }

    #[test]
    fn reassigned_list_is_not_fixed() {
        let mut stmts = vec![list_assign(0, 3)];
        stmts.push(Statement {
            kind: StatementKind::Assign(Local(0), Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            span: Span::default(),
        });
        let f = func_with(stmts);
        assert!(fixed_length_lists(&f).is_empty());
    }

    #[test]
    fn list_passed_to_call_is_not_fixed() {
        let stmts = vec![
            list_assign(0, 3),
            Statement {
                kind: StatementKind::Assign(
                    Local(1),
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("f".into())),
                        args: vec![Operand::Copy(Local(0))],
                    },
                ),
                span: Span::default(),
            },
        ];
        let f = func_with(stmts);
        assert!(fixed_length_lists(&f).is_empty());
    }
}
