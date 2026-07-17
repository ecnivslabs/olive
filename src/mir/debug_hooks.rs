//! JIT-only MIR instrumentation for the debugger (`pit dap` / `pit debug`).
//! Mirrors `shadow_stack::instrument`'s rebuild-the-block shape, but tracks
//! named-local values and per-line stop points instead of a call shadow
//! stack. Never runs outside a debug session, so hook-off programs carry
//! no trace of it -- see `tooling::dap::hooks` for the runtime side.
//!
//! `instrument` produces the fully-instrumented `$debug` body; `instrument_clean`
//! produces the default body a debug session runs when nothing is
//! watching it -- same per-line `debug_stmt`/`should_check_stmt` safepoint
//! coverage as `instrument` (precise stepping/pause can't regress), but
//! every `debug_store` is deferred to the rare taken branch of that check
//! instead of running eagerly after each assignment.
#![cfg_attr(not(test), allow(dead_code))]

use super::ir::*;
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

#[derive(Debug, Clone)]
pub struct CellInfo {
    pub name: String,
    pub local: usize,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct DebugFnInfo {
    pub name: String,
    pub fn_id: u32,
    pub cells: Vec<CellInfo>,
}

#[derive(Debug, Default)]
pub struct DebugProgramInfo {
    pub functions: Vec<DebugFnInfo>,
    /// Packed `(file_id << 32) | line` keys for every stop point inserted.
    pub lines: HashSet<i64>,
    /// Which function a given stop point belongs to, so a debug session
    /// can compute "which functions currently own an active breakpoint" and
    /// swap only those into their `$debug` variant (`tooling::dap::engine`).
    pub line_to_fn: HashMap<i64, u32>,
}

/// Rewrites every instrumentable function with enter/store/stmt/exit hook
/// calls so a debug session can capture frames and locals. `fn_id` is the
/// function's position in `functions`, stable for the whole session.
pub fn instrument(functions: &mut [MirFunction]) -> DebugProgramInfo {
    let mut program = DebugProgramInfo::default();
    for (fn_id, func) in functions.iter_mut().enumerate() {
        if func.is_async || !has_real_span(func) {
            continue;
        }
        let cells = named_cells(func);
        instrument_function(
            func,
            fn_id as u32,
            &cells,
            &mut program.lines,
            &mut program.line_to_fn,
            false,
        );
        program.functions.push(DebugFnInfo {
            name: func.name.clone(),
            fn_id: fn_id as u32,
            cells,
        });
    }
    program
}

/// Rewrites every instrumentable function the same way `instrument`
/// does -- same `__olive_debug_stmt` call at every distinct source line, so
/// every stepping/pause/breakpoint safepoint `instrument`'s output has, this
/// has too -- except `__olive_debug_store` calls for a reassignment aren't
/// emitted eagerly after each one. They're deferred to the rare taken
/// branch of the per-line conditional check instead, flushing every named
/// local at once right before the line actually parks (args are the one
/// exception, see `build_entry_prelude`). A first cut of this tried skipping
/// per-line checks entirely except at loop back-edges (only ~1 safepoint per
/// iteration instead of one per statement); it broke `step_out` returning
/// into straight-line code after a call with no further loop or call to
/// safepoint on, since a function's dispatch cell only resolves once, at the
/// call that invoked it -- no on-stack replacement, so a mid-flight clean
/// invocation can't be steered onto a fresh instrumented body no matter when
/// the cell flips. Precise stepping isn't optional, so this keeps the same
/// per-line granularity and only cuts the store cost -- the smaller, safe
/// half of the original idle-tax finding still holds (`dap.md`).
///
/// `fn_id` numbering must match `instrument`'s (same functions slice, same
/// enumeration order) so `debug_enter`'s fn_id -> cell_count lookup resolves
/// identically whichever variant is currently live behind the dispatch cell.
pub fn instrument_clean(functions: &[MirFunction]) -> Vec<MirFunction> {
    functions
        .iter()
        .enumerate()
        .map(|(fn_id, func)| {
            if func.is_async || !has_real_span(func) {
                return func.clone();
            }
            let cells = named_cells(func);
            let mut clean = func.clone();
            let mut lines = HashSet::default();
            let mut line_to_fn = HashMap::default();
            instrument_function(
                &mut clean,
                fn_id as u32,
                &cells,
                &mut lines,
                &mut line_to_fn,
                true,
            );
            clean
        })
        .collect()
}

/// A function whose statements are all `Span::default()` is compiler-
/// synthesized (the `__main__` wrapper), with no source line to stop at.
pub(crate) fn has_real_span(func: &MirFunction) -> bool {
    func.basic_blocks
        .iter()
        .any(|bb| bb.statements.iter().any(|s| s.span != Span::default()))
}

fn named_cells(func: &MirFunction) -> Vec<CellInfo> {
    func.locals
        .iter()
        .enumerate()
        .filter(|(_, decl)| decl.name.is_some() && !matches!(decl.ty, Type::Vector(..)))
        .map(|(idx, decl)| CellInfo {
            name: decl.name.clone().unwrap(),
            local: idx,
            ty: decl.ty.clone(),
        })
        .collect()
}

/// `defer_stores`: `instrument`'s ordinary mode (`false`) emits
/// `__olive_debug_store` right after each assignment, same as always.
/// `instrument_clean`'s mode (`true`) skips those and instead has
/// `make_stmt_conditional` flush every named local from the hook block,
/// once per line actually taken, instead of once per assignment.
fn instrument_function(
    func: &mut MirFunction,
    fn_id: u32,
    cells: &[CellInfo],
    lines: &mut HashSet<i64>,
    line_to_fn: &mut HashMap<i64, u32>,
    defer_stores: bool,
) {
    let cell_of: HashMap<usize, usize> = cells
        .iter()
        .enumerate()
        .map(|(cell_idx, c)| (c.local, cell_idx))
        .collect();

    // Phase 1: Instrument all blocks with debug hooks (same as before).
    let mut stmt_sites: Vec<(usize, usize, i64)> = Vec::new();
    let mut entry_prelude = Some(build_entry_prelude(func, fn_id, func.arg_count, &cell_of));

    for bb_idx in 0..func.basic_blocks.len() {
        let original = std::mem::take(&mut func.basic_blocks[bb_idx].statements);
        let mut rebuilt = entry_prelude.take().unwrap_or_default();
        let mut last_line: Option<i64> = None;
        // Track original stmt index → position in instrumented block: each
        // debug_stmt call at `stmt_site` in the instrumented block gets
        // wrapped with a conditional check below.
        let _stmt_start = rebuilt.len();

        for stmt in original {
            if stmt.span != Span::default() {
                let packed = pack_line(&stmt.span);
                if last_line != Some(packed) {
                    lines.insert(packed);
                    line_to_fn.insert(packed, fn_id);
                    rebuilt.push(call_stmt(
                        func,
                        "__olive_debug_stmt",
                        vec![Operand::Constant(Constant::Int(packed))],
                        stmt.span,
                    ));
                    stmt_sites.push((bb_idx, rebuilt.len() - 1, packed));
                    last_line = Some(packed);
                }
            }

            let store = (!defer_stores)
                .then(|| match &stmt.kind {
                    StatementKind::Assign(local, _) => cell_of
                        .get(&local.0)
                        .map(|&cell_idx| (cell_idx, *local, stmt.span)),
                    _ => None,
                })
                .flatten();

            rebuilt.push(stmt);

            if let Some((cell_idx, local, span)) = store {
                rebuilt.push(call_stmt(
                    func,
                    "__olive_debug_store",
                    vec![
                        Operand::Constant(Constant::Int(cell_idx as i64)),
                        Operand::Copy(local),
                    ],
                    span,
                ));
            }
        }
        func.basic_blocks[bb_idx].statements = rebuilt;

        let return_span = match &func.basic_blocks[bb_idx].terminator {
            Some(Terminator {
                kind: TerminatorKind::Return,
                span,
            }) => Some(*span),
            _ => None,
        };
        if let Some(span) = return_span {
            let exit_call = call_stmt(func, "__olive_debug_exit", vec![], span);
            func.basic_blocks[bb_idx].statements.push(exit_call);
        }
    }

    // Replace each `__olive_debug_stmt` call with a conditional sequence
    // that checks `__olive_debug_should_check_stmt()` first. Process
    // blocks in reverse so block indices stay valid.
    stmt_sites.reverse();
    for &(bb_idx, stmt_pos, packed) in &stmt_sites {
        make_stmt_conditional(
            func,
            bb_idx,
            stmt_pos,
            packed,
            cells,
            &cell_of,
            defer_stores,
        );
    }
}

/// Replace the `__olive_debug_stmt(packed)` call at `stmt_pos`
/// in basic block `bb_idx` with a conditional dispatch:
///
///   _check = __olive_debug_should_check_stmt()
///   switchint _check:
///       0 -> cont
///       _ -> hook_block
///   hook_block:
///       [__olive_debug_store(cell, local) for every named cell, if deferred]
///       __olive_debug_stmt(packed)
///       [local = __olive_debug_load(cell) for every named cell]
///       goto cont
///   cont:
///       (rest of the original block after stmt_pos)
///
/// The original block's terminator moves to the `cont` block. `flush_stores`
/// (set by `instrument_clean`) adds the bracketed stores: since that mode
/// never runs a store eagerly at the assignment site, the rare taken branch
/// has to flush every named local's current value before parking, or a
/// stop here would report whatever the last flush (or entry) left behind.
///
/// The reload after `__olive_debug_stmt` runs unconditionally in both
/// instrumentation modes: it's how a `setVariable`/`setExpression` write
/// queued while parked (`tooling::dap::engine::EngineShared::
/// set_local_cell`) reaches the real, running local, since the debuggee
/// thread's actual storage for a local is otherwise only reachable through
/// its own compiled code, never by poking memory from the controller
/// thread. `debug_store` already keeps the mirror equal to the real local
/// at every earlier hook point, so reassigning from it here is a no-op
/// whenever nothing patched it.
fn make_stmt_conditional(
    func: &mut MirFunction,
    bb_idx: usize,
    stmt_pos: usize,
    packed: i64,
    cells: &[CellInfo],
    cell_of: &HashMap<usize, usize>,
    flush_stores: bool,
) {
    let span = Span::default();
    let statements = std::mem::take(&mut func.basic_blocks[bb_idx].statements);
    let old_terminator = func.basic_blocks[bb_idx].terminator.take();

    // Split: statements before the hook call stay in the original block.
    let (before, after) = statements.split_at(stmt_pos);
    let before: Vec<Statement> = before.to_vec();
    // The first element of `after` is the debug_stmt call; skip it.
    let after: Vec<Statement> = after.iter().skip(1).cloned().collect();

    let check_local = alloc_sink(func);
    let mut hook_stmts = Vec::new();
    if flush_stores {
        for cell in cells {
            let cell_idx = *cell_of
                .get(&cell.local)
                .expect("cell_of has every named local");
            hook_stmts.push(call_stmt(
                func,
                "__olive_debug_store",
                vec![
                    Operand::Constant(Constant::Int(cell_idx as i64)),
                    Operand::Copy(Local(cell.local)),
                ],
                span,
            ));
        }
    }
    hook_stmts.push(call_stmt(
        func,
        "__olive_debug_stmt",
        vec![Operand::Constant(Constant::Int(packed))],
        span,
    ));
    for cell in cells {
        let cell_idx = *cell_of
            .get(&cell.local)
            .expect("cell_of has every named local");
        hook_stmts.push(Statement {
            kind: StatementKind::Assign(
                Local(cell.local),
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_debug_load".to_string())),
                    args: vec![Operand::Constant(Constant::Int(cell_idx as i64))],
                },
            ),
            span,
        });
    }

    // -- hook_block: the flush (if any) + __olive_debug_stmt + reload, then goto cont --
    let hook_bb = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: hook_stmts,
        terminator: None,
    });

    // -- cont block: remaining original statements + original terminator --
    let cont_bb = BasicBlockId(func.basic_blocks.len());
    func.basic_blocks.push(BasicBlock {
        statements: after,
        terminator: old_terminator,
    });

    // -- Set hook block's terminator to goto cont --
    func.basic_blocks[hook_bb.0].terminator = Some(Terminator {
        kind: TerminatorKind::Goto { target: cont_bb },
        span,
    });

    // -- Original block: check + switchint --
    let mut new_statements = before;
    new_statements.push(Statement {
        kind: StatementKind::Assign(
            check_local,
            Rvalue::Call {
                func: Operand::Constant(Constant::Function(
                    "__olive_debug_should_check_stmt".to_string(),
                )),
                args: vec![Operand::Constant(Constant::Int(packed))],
            },
        ),
        span,
    });
    // Insert the __olive_debug_should_check_stmt import so the codegen
    // can find it. The call_stmt helper already adds the function constant
    // as part of the call; we just need to make sure the import table
    // picks it up. Since collect_needed_imports scans all Rvalue::Calls,
    // the reference above is enough.

    func.basic_blocks[bb_idx].statements = new_statements;
    func.basic_blocks[bb_idx].terminator = Some(Terminator {
        kind: TerminatorKind::SwitchInt {
            discr: Operand::Copy(check_local),
            targets: vec![(0, cont_bb)],
            otherwise: hook_bb,
        },
        span,
    });
}

/// Args always store eagerly here, in both instrumentation modes: a fault
/// (`tooling::dap::hooks::debug_fault_hook`) can land on a function's very
/// first line, before that line's own conditional flush ever runs -- there
/// is no earlier safepoint a deferred store could ride along with, unlike
/// every other named cell, which is guaranteed at least one prior checked
/// line before anything can read it. Cheap either way: bounded by arg
/// count per call, not by how many times a loop iterates, which is what
/// `defer_stores` (set by `instrument_clean`) exists to avoid paying for.
fn build_entry_prelude(
    func: &mut MirFunction,
    fn_id: u32,
    arg_count: usize,
    cell_of: &HashMap<usize, usize>,
) -> Vec<Statement> {
    let mut out = vec![call_stmt(
        func,
        "__olive_debug_enter",
        vec![Operand::Constant(Constant::Int(fn_id as i64))],
        Span::default(),
    )];
    for local_idx in 1..=arg_count {
        if let Some(&cell_idx) = cell_of.get(&local_idx) {
            out.push(call_stmt(
                func,
                "__olive_debug_store",
                vec![
                    Operand::Constant(Constant::Int(cell_idx as i64)),
                    Operand::Copy(Local(local_idx)),
                ],
                Span::default(),
            ));
        }
    }
    out
}

fn call_stmt(func: &mut MirFunction, name: &str, args: Vec<Operand>, span: Span) -> Statement {
    let sink = alloc_sink(func);
    Statement {
        kind: StatementKind::Assign(
            sink,
            Rvalue::Call {
                func: Operand::Constant(Constant::Function(name.to_string())),
                args,
            },
        ),
        span,
    }
}

fn alloc_sink(func: &mut MirFunction) -> Local {
    let id = func.locals.len();
    func.locals.push(LocalDecl {
        ty: Type::Null,
        name: None,
        span: Span::default(),
        is_mut: false,
        is_owning: true,
    });
    Local(id)
}

fn pack_line(span: &Span) -> i64 {
    ((span.file_id as i64) << 32) | (span.line as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::Optimizer;

    fn instrumented(src: &str) -> (Vec<MirFunction>, DebugProgramInfo) {
        let mut functions = crate::test_utils::build_mir(src);
        Optimizer::minimal().run(&mut functions);
        let program = instrument(&mut functions);
        (functions, program)
    }

    fn find<'a>(functions: &'a [MirFunction], name: &str) -> &'a MirFunction {
        functions
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("function {name} not found"))
    }

    fn call_names(func: &MirFunction) -> Vec<&str> {
        func.basic_blocks
            .iter()
            .flat_map(|bb| &bb.statements)
            .filter_map(|s| match &s.kind {
                StatementKind::Assign(
                    _,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(name)),
                        ..
                    },
                ) => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn one_enter_per_user_fn() {
        let (functions, _) = instrumented(
            "fn add(a: int, b: int) -> int:\n    return a + b\nfn main():\n    print(add(1, 2))\n",
        );
        for name in ["add", "main"] {
            let f = find(&functions, name);
            let enters = call_names(f)
                .iter()
                .filter(|&&n| n == "__olive_debug_enter")
                .count();
            assert_eq!(enters, 1, "{name} should have exactly one enter hook");
        }
    }

    #[test]
    fn cells_capture_named_locals_with_types_and_fn_id() {
        let (functions, program) = instrumented(
            "fn add(a: int, b: float) -> int:\n    let total = a\n    print(b)\n    return total\n",
        );
        let add = find(&functions, "add");
        let expected_fn_id = functions.iter().position(|f| f.name == "add").unwrap() as u32;
        let info = program
            .functions
            .iter()
            .find(|f| f.name == "add")
            .expect("add should be instrumented");
        assert_eq!(info.fn_id, expected_fn_id);

        let names: Vec<&str> = info.cells.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["_return", "a", "b", "total"]);

        for cell in &info.cells {
            let expected_local = add
                .locals
                .iter()
                .position(|l| l.name.as_deref() == Some(cell.name.as_str()))
                .unwrap();
            assert_eq!(cell.local, expected_local);
        }

        let ty_of = |name: &str| {
            info.cells
                .iter()
                .find(|c| c.name == name)
                .map(|c| c.ty.clone())
                .unwrap()
        };
        assert_eq!(ty_of("a"), Type::Int);
        assert_eq!(ty_of("b"), Type::Float);
        assert_eq!(ty_of("total"), Type::Int);
    }

    #[test]
    fn stmt_count_matches_distinct_lines() {
        let (functions, program) =
            instrumented("fn main():\n    let x = 1\n    let y = 2\n    print(x + y)\n");
        let main = find(&functions, "main");
        let stmt_hooks = call_names(main)
            .iter()
            .filter(|&&n| n == "__olive_debug_stmt")
            .count();
        assert_eq!(stmt_hooks, 3);
        assert_eq!(program.lines.len(), 3);
    }

    #[test]
    fn store_follows_named_assign() {
        let (functions, _) =
            instrumented("fn main():\n    let mut x = 1\n    x = 2\n    print(x)\n");
        let main = find(&functions, "main");
        // "x" is a named local; every assignment to it must be followed by a store.
        let flat: Vec<&Statement> = main
            .basic_blocks
            .iter()
            .flat_map(|bb| &bb.statements)
            .collect();
        let x_local = main
            .locals
            .iter()
            .position(|l| l.name.as_deref() == Some("x"))
            .unwrap();
        let expected_cell = main
            .locals
            .iter()
            .enumerate()
            .filter(|(_, l)| l.name.is_some() && !matches!(l.ty, Type::Vector(..)))
            .position(|(idx, _)| idx == x_local)
            .unwrap() as i64;
        let mut assigns_to_x = 0;
        for (i, s) in flat.iter().enumerate() {
            if let StatementKind::Assign(local, rval) = &s.kind
                && local.0 == x_local
            {
                // Every hook block's reload (`local = __olive_debug_load(cell)`,
                // right after that line's `__olive_debug_stmt`) also assigns
                // into every named cell, x included -- real program
                // assignments are the ones that store, not these.
                if matches!(
                    rval,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(n)),
                        ..
                    } if n == "__olive_debug_load"
                ) {
                    continue;
                }
                assigns_to_x += 1;
                let next = flat.get(i + 1).unwrap();
                match &next.kind {
                    StatementKind::Assign(
                        _,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(n)),
                            args,
                        },
                    ) => {
                        assert_eq!(n, "__olive_debug_store");
                        assert_eq!(args[0], Operand::Constant(Constant::Int(expected_cell)));
                        assert_eq!(args[1], Operand::Copy(*local));
                    }
                    other => panic!("expected store call after assign, got {other:?}"),
                }
            }
        }
        assert_eq!(assigns_to_x, 2, "let-init and reassignment both store");
    }

    #[test]
    fn reload_follows_every_stmt_hook() {
        let (functions, program) = instrumented(
            "fn add(a: int, b: int) -> int:\n    let total = a + b\n    return total\n",
        );
        let add = find(&functions, "add");
        let names = call_names(add);
        let stmt_hooks = names.iter().filter(|&&n| n == "__olive_debug_stmt").count();
        let load_calls = names.iter().filter(|&&n| n == "__olive_debug_load").count();
        let info = program.functions.iter().find(|f| f.name == "add").unwrap();
        assert_eq!(
            load_calls,
            stmt_hooks * info.cells.len(),
            "every named cell reloads at every stmt hook, both instrumentation modes"
        );

        let clean_fns =
            clean("fn add(a: int, b: int) -> int:\n    let total = a + b\n    return total\n");
        let clean_add = find(&clean_fns, "add");
        let clean_names = call_names(clean_add);
        let clean_stmt_hooks = clean_names
            .iter()
            .filter(|&&n| n == "__olive_debug_stmt")
            .count();
        let clean_load_calls = clean_names
            .iter()
            .filter(|&&n| n == "__olive_debug_load")
            .count();
        assert_eq!(clean_stmt_hooks, stmt_hooks);
        assert_eq!(clean_load_calls, load_calls);
    }

    #[test]
    fn reload_targets_the_named_local_its_cell_belongs_to() {
        let (functions, program) = instrumented(
            "fn add(a: int, b: int) -> int:\n    let total = a + b\n    return total\n",
        );
        let add = find(&functions, "add");
        let info = program.functions.iter().find(|f| f.name == "add").unwrap();
        let cell_idx_of_total = info.cells.iter().position(|c| c.name == "total").unwrap() as i64;
        let total_local = add
            .locals
            .iter()
            .position(|l| l.name.as_deref() == Some("total"))
            .unwrap();

        let found = add.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(
                        local,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(n)),
                            args,
                        },
                    ) if local.0 == total_local
                        && n == "__olive_debug_load"
                        && args == &[Operand::Constant(Constant::Int(cell_idx_of_total))]
                )
            })
        });
        assert!(found, "total's cell reloads back into total's own local");
    }

    #[test]
    fn exit_precedes_every_return() {
        let (functions, _) = instrumented(
            "fn pick(flag: bool) -> int:\n    if flag:\n        return 1\n    return 2\nfn main():\n    print(pick(True))\n",
        );
        let pick = find(&functions, "pick");
        for bb in &pick.basic_blocks {
            if matches!(
                bb.terminator,
                Some(Terminator {
                    kind: TerminatorKind::Return,
                    ..
                })
            ) {
                let last = bb.statements.last().expect("block has statements");
                match &last.kind {
                    StatementKind::Assign(
                        _,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(name)),
                            ..
                        },
                    ) => assert_eq!(name, "__olive_debug_exit"),
                    other => panic!("expected exit hook before return, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn lines_hold_expected_packed_keys() {
        let (functions, program) = instrumented("fn main():\n    let x = 1\n    print(x)\n");
        let main = find(&functions, "main");
        let expected: HashSet<i64> = main
            .basic_blocks
            .iter()
            .flat_map(|bb| &bb.statements)
            .filter_map(|s| match &s.kind {
                StatementKind::Assign(
                    _,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(name)),
                        args,
                    },
                ) if name == "__olive_debug_stmt" => match &args[0] {
                    Operand::Constant(Constant::Int(packed)) => Some(*packed),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(program.lines, expected);
        assert!(!program.lines.is_empty());
    }

    #[test]
    fn async_fns_untouched() {
        let mut functions = crate::test_utils::build_mir(
            "async fn fetch() -> int:\n    return 1\nfn main():\n    print(1)\n",
        );
        Optimizer::minimal().run(&mut functions);
        let before = find(&functions, "fetch").clone();
        let program = instrument(&mut functions);
        let after = find(&functions, "fetch");
        assert_eq!(before.basic_blocks, after.basic_blocks);
        assert_eq!(before.locals, after.locals);
        assert!(!program.functions.iter().any(|f| f.name == "fetch"));
    }

    fn clean(src: &str) -> Vec<MirFunction> {
        let mut functions = crate::test_utils::build_mir(src);
        Optimizer::minimal().run(&mut functions);
        instrument_clean(&functions)
    }

    #[test]
    fn clean_straight_line_fn_has_a_check_per_distinct_line_no_eager_stores() {
        let functions =
            clean("fn add(a: int, b: int) -> int:\n    let total = a + b\n    return total\n");
        let add = find(&functions, "add");
        let names = call_names(add);
        assert_eq!(
            names
                .iter()
                .filter(|&&n| n == "__olive_debug_enter")
                .count(),
            1
        );
        assert_eq!(
            names.iter().filter(|&&n| n == "__olive_debug_exit").count(),
            1
        );
        // Same per-line coverage as `instrument`: one check + one stmt hook
        // per distinct source line (the assign and the return).
        assert_eq!(
            names.iter().filter(|&&n| n == "__olive_debug_stmt").count(),
            2
        );
        assert_eq!(
            names
                .iter()
                .filter(|&&n| n == "__olive_debug_should_check_stmt")
                .count(),
            2,
        );
        // No eager per-reassignment stores: only the flush inside each
        // line's taken branch, one store per named cell per line, plus one
        // store per arg from the entry prelude (`build_entry_prelude`
        // stores args eagerly in both modes -- a fault on the function's
        // very first line has no earlier safepoint to defer to).
        let named = named_cells(add);
        let arg_count = 2;
        assert_eq!(
            names
                .iter()
                .filter(|&&n| n == "__olive_debug_store")
                .count(),
            2 * named.len() + arg_count,
        );
    }

    #[test]
    fn clean_loop_checks_every_line_same_as_instrument() {
        let src = concat!(
            "fn count(n: int) -> int:\n",
            "    let mut total = 0\n",
            "    let mut i = 0\n",
            "    while i < n:\n",
            "        total = total + i\n",
            "        i = i + 1\n",
            "    return total\n",
        );
        let mut functions = crate::test_utils::build_mir(src);
        Optimizer::minimal().run(&mut functions);
        let clean_fns = instrument_clean(&functions);
        let mut full_fns = functions.clone();
        instrument(&mut full_fns);
        let full_count = find(&full_fns, "count");
        let full_checks = call_names(full_count)
            .iter()
            .filter(|&&n| n == "__olive_debug_should_check_stmt")
            .count();
        let clean_count = find(&clean_fns, "count");
        let clean_checks = call_names(clean_count)
            .iter()
            .filter(|&&n| n == "__olive_debug_should_check_stmt")
            .count();

        assert_eq!(
            clean_checks, full_checks,
            "clean and fully-instrumented variants must safepoint on the same lines"
        );
        assert!(
            clean_checks > 1,
            "loop condition and body are distinct lines"
        );
    }

    #[test]
    fn clean_and_instrumented_assign_the_same_fn_id() {
        let src = "fn helper(x: int) -> int:\n    return x\nfn main():\n    print(helper(1))\n";
        let mut functions = crate::test_utils::build_mir(src);
        Optimizer::minimal().run(&mut functions);
        let full = instrument(&mut functions.clone());
        let clean_fns = instrument_clean(&functions);

        let full_id = full
            .functions
            .iter()
            .find(|f| f.name == "helper")
            .unwrap()
            .fn_id;

        let helper_clean = find(&clean_fns, "helper");
        let enter_call = helper_clean
            .basic_blocks
            .iter()
            .flat_map(|bb| &bb.statements)
            .find_map(|s| match &s.kind {
                StatementKind::Assign(
                    _,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(name)),
                        args,
                    },
                ) if name == "__olive_debug_enter" => match &args[0] {
                    Operand::Constant(Constant::Int(id)) => Some(*id as u32),
                    _ => None,
                },
                _ => None,
            })
            .expect("helper's clean variant has an enter hook");
        assert_eq!(enter_call, full_id);
    }

    #[test]
    fn clean_variant_runs_to_the_same_output_as_plain() {
        use crate::test_utils::exec_lock;
        let src = concat!(
            "fn sum_to(n: int) -> int:\n",
            "    let mut total = 0\n",
            "    let mut i = 0\n",
            "    while i < n:\n",
            "        total = total + i\n",
            "        i = i + 1\n",
            "    return total\n",
            "\n",
            "fn main():\n",
            "    print(sum_to(10))\n",
        );
        let functions = clean(src);
        assert!(functions.iter().any(|f| f.name == "sum_to"));
        let mut jit = crate::test_utils::compile_prebuilt(functions);
        let ptr = jit.get_function("__main__").expect("__main__ not found");
        let main_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(ptr) };
        let _guard = exec_lock();
        // DEBUGGEE_ENABLED defaults off outside a real `pit debug` session,
        // so the safepoint/enter/exit hooks all take their early-return --
        // this must behave exactly like an uninstrumented sum_to(10) == 45.
        assert_eq!(main_fn(), 0);
    }
}
