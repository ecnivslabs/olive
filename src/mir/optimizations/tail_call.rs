use super::Transform;
use crate::mir::*;
use crate::span::Span;

pub struct TailCallOpt;

impl Transform for TailCallOpt {
    fn run(&self, func: &mut MirFunction) -> bool {
        let func_name = func.name.clone();
        let arg_count = func.arg_count;

        // Back-edge target can't be the entry: codegen seals it as predecessor-free,
        // so move the body into a fresh header and leave entry as a one-time jump.
        let header = match self.split_entry(func, &func_name, arg_count) {
            Some(h) => h,
            None => return false,
        };

        let mut changed = false;
        for bb_idx in 0..func.basic_blocks.len() {
            let Some(args) =
                Self::tail_call_args(&func.basic_blocks[bb_idx], &func_name, arg_count)
            else {
                continue;
            };

            let span = func.basic_blocks[bb_idx]
                .statements
                .last()
                .map(|s| s.span)
                .unwrap_or_default();

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

            let bb = &mut func.basic_blocks[bb_idx];
            bb.statements.pop();
            bb.statements.pop();

            // Stage args through temporaries so each sees pre-update param values.
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
                    target: BasicBlockId(header),
                },
                span,
            });

            changed = true;
        }

        changed
    }
}

impl TailCallOpt {
    /// Args of a self tail call ending `bb` (same-name call filling every param), else `None`.
    fn tail_call_args(bb: &BasicBlock, func_name: &str, arg_count: usize) -> Option<Vec<Operand>> {
        if !matches!(bb.terminator.as_ref()?.kind, TerminatorKind::Return) {
            return None;
        }
        let stmts = &bb.statements;
        if stmts.len() < 2 {
            return None;
        }
        let copy_src = match &stmts[stmts.len() - 1].kind {
            StatementKind::Assign(
                Local(0),
                Rvalue::Use(Operand::Copy(src) | Operand::Move(src)),
            ) => *src,
            _ => return None,
        };
        let args = match &stmts[stmts.len() - 2].kind {
            StatementKind::Assign(
                dest,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(name)),
                    args,
                },
            ) if *dest == copy_src && name == func_name => args.clone(),
            _ => return None,
        };
        if args.len() != arg_count {
            return None;
        }
        Some(args)
    }

    /// Moves entry contents into a new header block (entry jumps to it) and returns
    /// its index when a self tail call exists, else leaves the fn and returns `None`.
    fn split_entry(
        &self,
        func: &mut MirFunction,
        func_name: &str,
        arg_count: usize,
    ) -> Option<usize> {
        let has_tail = func
            .basic_blocks
            .iter()
            .any(|bb| Self::tail_call_args(bb, func_name, arg_count).is_some());
        if !has_tail {
            return None;
        }
        let header = func.basic_blocks.len();
        let span = func.basic_blocks[0]
            .terminator
            .as_ref()
            .map(|t| t.span)
            .unwrap_or_default();
        let entry = std::mem::replace(
            &mut func.basic_blocks[0],
            BasicBlock {
                statements: Vec::new(),
                terminator: Some(Terminator {
                    kind: TerminatorKind::Goto {
                        target: BasicBlockId(header),
                    },
                    span,
                }),
            },
        );
        func.basic_blocks.push(entry);
        Some(header)
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
