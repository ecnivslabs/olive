//! JIT-only instrumentation so a runtime fault mid-call-chain prints every
//! frame between it and `main`, innermost first, not just the caret at the
//! fault site. Runs once, after optimization, only on the MIR handed to the
//! debug (`pit run`) JIT pipeline -- AOT release never calls this, so it
//! stays permanently caret-only at zero cost; see `std_lib::shadow_stack`
//! for the runtime side.
//!
//! Every statically-known call to another function in this same program
//! (`Constant::Function(name)` where `name` is itself one of `functions`) is
//! wrapped with a push right before it and a pop right after: `assign; push;
//! call; pop`. A call through a function-typed value (a closure, a trait
//! object) has no static name to show and is left alone, same limitation the
//! ownership escape analysis already lives with (see
//! `mir::optimizations::ownership::summaries`).

use super::ir::*;
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub fn instrument(functions: &mut [MirFunction], file_names: &HashMap<usize, String>) {
    let user_fns: HashSet<String> = functions.iter().map(|f| f.name.clone()).collect();
    for func in functions.iter_mut() {
        instrument_function(func, &user_fns, file_names);
    }
}

fn instrument_function(
    func: &mut MirFunction,
    user_fns: &HashSet<String>,
    file_names: &HashMap<usize, String>,
) {
    for bb_idx in 0..func.basic_blocks.len() {
        let original = std::mem::take(&mut func.basic_blocks[bb_idx].statements);
        let mut rebuilt = Vec::with_capacity(original.len());
        for stmt in original {
            let callee = match &stmt.kind {
                StatementKind::Assign(
                    _,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(name)),
                        ..
                    },
                ) if user_fns.contains(name) => Some(name.clone()),
                _ => None,
            };
            // A compiler-synthesized call (the top-level `__main__` wrapper
            // invoking the user's `main`) carries no real source location;
            // showing `0:0` would be noise, not information, so it's left
            // uninstrumented like an indirect call.
            let Some(callee) = callee.filter(|_| stmt.span != Span::default()) else {
                rebuilt.push(stmt);
                continue;
            };
            let span = stmt.span;
            let loc = match file_names.get(&span.file_id) {
                Some(file) => format!("{file}:{}:{}", span.line, span.col),
                None => format!("{}:{}", span.line, span.col),
            };
            let name_local = str_const_local(func, callee, span, &mut rebuilt);
            let loc_local = str_const_local(func, loc, span, &mut rebuilt);
            let push_sink = alloc_local(func, Type::Null);
            rebuilt.push(Statement {
                kind: StatementKind::Assign(
                    push_sink,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_shadow_push".to_string(),
                        )),
                        args: vec![Operand::Copy(name_local), Operand::Copy(loc_local)],
                    },
                ),
                span,
            });
            rebuilt.push(stmt);
            let pop_sink = alloc_local(func, Type::Null);
            rebuilt.push(Statement {
                kind: StatementKind::Assign(
                    pop_sink,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_shadow_pop".to_string(),
                        )),
                        args: vec![],
                    },
                ),
                span,
            });
        }
        func.basic_blocks[bb_idx].statements = rebuilt;
    }
}

fn alloc_local(func: &mut MirFunction, ty: Type) -> Local {
    let id = func.locals.len();
    func.locals.push(LocalDecl {
        ty,
        name: None,
        span: Span::default(),
        is_mut: false,
        is_owning: true,
    });
    Local(id)
}

fn str_const_local(
    func: &mut MirFunction,
    s: String,
    span: Span,
    out: &mut Vec<Statement>,
) -> Local {
    let local = alloc_local(func, Type::Str);
    out.push(Statement {
        kind: StatementKind::Assign(local, Rvalue::Use(Operand::Constant(Constant::Str(s)))),
        span,
    });
    local
}
