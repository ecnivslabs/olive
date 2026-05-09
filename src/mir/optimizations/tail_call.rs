use super::Transform;
use crate::mir::*;
use crate::span::Span;

pub struct TailCallOpt;

impl Transform for TailCallOpt {
    fn name(&self) -> &'static str {
        "tail_call_opt"
    }

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
