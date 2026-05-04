use super::Transform;
use crate::mir::*;
use crate::span::Span;

pub struct TailCallOpt;

impl Transform for TailCallOpt {
    fn run(&self, func: &mut MirFunction) -> bool {
        let func_name = func.name.clone();
        let arg_count = func.arg_count;
        let mut changed = false;

        for bb_idx in 0..func.basic_blocks.len() {
            if let Some(term) = &func.basic_blocks[bb_idx].terminator {
                if !matches!(term.kind, TerminatorKind::Return) {
                    continue;
                }
            } else {
                continue;
            }

            let stmts = &func.basic_blocks[bb_idx].statements;
            if stmts.len() < 2 {
                continue;
            }

            let last = &stmts[stmts.len() - 1];
            let second_last = &stmts[stmts.len() - 2];

            let copy_src = match &last.kind {
                StatementKind::Assign(Local(0), Rvalue::Use(Operand::Copy(src))) => *src,
                StatementKind::Assign(Local(0), Rvalue::Use(Operand::Move(src))) => *src,
                _ => continue,
            };

            let args = match &second_last.kind {
                StatementKind::Assign(
                    dest,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(name)),
                        args,
                    },
                ) if *dest == copy_src && *name == func_name => args.clone(),
                _ => continue,
            };

            if args.len() != arg_count {
                continue;
            }

            let span = stmts[stmts.len() - 1].span;
            let bb = &mut func.basic_blocks[bb_idx];

            bb.statements.pop();
            bb.statements.pop();

            let base_tmp = func.locals.len();
            for _ in 0..arg_count {
                func.locals.push(LocalDecl {
                    ty: crate::semantic::types::Type::Any,
                    name: None,
                    span: Span::default(),
                    is_mut: true,
                    is_owning: true,
                });
            }

            for (j, arg) in args.iter().enumerate() {
                bb.statements.push(Statement {
                    kind: StatementKind::Assign(Local(base_tmp + j), Rvalue::Use(arg.clone())),
                    span,
                });
            }

            for j in 0..arg_count {
                bb.statements.push(Statement {
                    kind: StatementKind::Assign(
                        Local(j + 1),
                        Rvalue::Use(Operand::Copy(Local(base_tmp + j))),
                    ),
                    span,
                });
            }

            bb.terminator = Some(Terminator {
                kind: TerminatorKind::Goto {
                    target: BasicBlockId(0),
                },
                span,
            });

            changed = true;
        }

        changed
    }
}

#[cfg(test)]
#[cfg_attr(test, allow(dead_code))]
mod tests {
    use super::*;

    fn sp() -> crate::span::Span {
        crate::span::Span {
            file_id: 0,
            line: 0,
            col: 0,
            start: 0,
            end: 0,
        }
    }

    fn assign(l: usize, rv: Rvalue) -> Statement {
        Statement {
            kind: StatementKind::Assign(Local(l), rv),
            span: sp(),
        }
    }

    fn stmt(k: StatementKind) -> Statement {
        Statement {
            kind: k,
            span: sp(),
        }
    }

    fn func(name: &str, stmts: Vec<Statement>, args: usize) -> MirFunction {
        MirFunction {
            name: name.into(),
            locals: vec![],
            basic_blocks: vec![BasicBlock {
                statements: stmts,
                terminator: Some(Terminator {
                    kind: TerminatorKind::Return,
                    span: sp(),
                }),
            }],
            arg_count: args,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    fn bb(stmts: Vec<Statement>, kind: TerminatorKind) -> BasicBlock {
        BasicBlock {
            statements: stmts,
            terminator: Some(Terminator { kind, span: sp() }),
        }
    }

    #[test]
    fn no_tail_call_no_change() {
        let mut f = func("f", vec![], 0);
        assert!(!TailCallOpt.run(&mut f));
    }

    #[test]
    fn tail_call_self_optimized() {
        let mut f = MirFunction {
            name: "f".into(),
            locals: vec![],
            basic_blocks: vec![bb(
                vec![
                    assign(
                        1,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function("f".into())),
                            args: vec![],
                        },
                    ),
                    assign(0, Rvalue::Use(Operand::Copy(Local(1)))),
                ],
                TerminatorKind::Return,
            )],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        };
        assert!(TailCallOpt.run(&mut f));
    }

    #[test]
    fn non_tail_call_not_optimized() {
        let mut f = MirFunction {
            name: "f".into(),
            locals: vec![],
            basic_blocks: vec![bb(
                vec![
                    assign(
                        1,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function("g".into())),
                            args: vec![],
                        },
                    ),
                    assign(0, Rvalue::Use(Operand::Copy(Local(1)))),
                ],
                TerminatorKind::Return,
            )],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        };
        assert!(!TailCallOpt.run(&mut f));
    }

    #[test]
    fn insufficient_statements_no_change() {
        let mut f = func("f", vec![], 0);
        assert!(!TailCallOpt.run(&mut f));
    }
}
