//! JIT-only MIR instrumentation for the debugger (`pit dap` / `pit debug`).
//! Mirrors `shadow_stack::instrument`'s rebuild-the-block shape, but tracks
//! named-local values and per-line stop points instead of a call shadow
//! stack. Never runs outside a debug session, so hook-off programs carry
//! no trace of it -- see `tooling::dap::hooks` for the runtime side.
//!
//! `main.rs` doesn't call into this yet, so the bin target sees the whole
//! module as unreachable; the tests below already exercise it in full.
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
        instrument_function(func, fn_id as u32, &cells, &mut program.lines);
        program.functions.push(DebugFnInfo {
            name: func.name.clone(),
            fn_id: fn_id as u32,
            cells,
        });
    }
    program
}

/// A function whose statements are all `Span::default()` is compiler-
/// synthesized (the `__main__` wrapper), with no source line to stop at.
fn has_real_span(func: &MirFunction) -> bool {
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

fn instrument_function(
    func: &mut MirFunction,
    fn_id: u32,
    cells: &[CellInfo],
    lines: &mut HashSet<i64>,
) {
    let cell_of: HashMap<usize, usize> = cells
        .iter()
        .enumerate()
        .map(|(cell_idx, c)| (c.local, cell_idx))
        .collect();

    let mut entry_prelude = Some(build_entry_prelude(func, fn_id, func.arg_count, &cell_of));

    for bb_idx in 0..func.basic_blocks.len() {
        let original = std::mem::take(&mut func.basic_blocks[bb_idx].statements);
        let mut rebuilt = entry_prelude.take().unwrap_or_default();
        let mut last_line: Option<i64> = None;

        for stmt in original {
            if stmt.span != Span::default() {
                let packed = pack_line(&stmt.span);
                if last_line != Some(packed) {
                    lines.insert(packed);
                    rebuilt.push(call_stmt(
                        func,
                        "__olive_debug_stmt",
                        vec![Operand::Constant(Constant::Int(packed))],
                        stmt.span,
                    ));
                    last_line = Some(packed);
                }
            }

            let store = match &stmt.kind {
                StatementKind::Assign(local, _) => cell_of
                    .get(&local.0)
                    .map(|&cell_idx| (cell_idx, *local, stmt.span)),
                _ => None,
            };

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
}

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
            if let StatementKind::Assign(local, _) = &s.kind
                && local.0 == x_local
            {
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
}
