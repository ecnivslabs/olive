use super::LoopContext;
use super::MirBuilder;
use crate::mir::ir::*;
use crate::parser::{Expr, ForTarget, Stmt};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    pub(super) fn lower_if(
        &mut self,
        condition: &Expr,
        then_body: &[Stmt],
        elif_clauses: &[(Expr, Vec<Stmt>)],
        else_body: &Option<Vec<Stmt>>,
    ) {
        let cond_op = self.lower_expr(condition);
        let then_bb = self.new_block();
        let merge_bb = self.new_block();

        let next_bb = if !elif_clauses.is_empty() || else_body.is_some() {
            self.new_block()
        } else {
            merge_bb
        };

        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::SwitchInt {
                    discr: cond_op,
                    targets: vec![(1, then_bb)],
                    otherwise: next_bb,
                },
                condition.span,
            );
        }

        self.current_block = Some(then_bb);
        self.enter_scope();
        for s in then_body {
            self.lower_stmt(s);
        }
        self.leave_scope();
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto { target: merge_bb },
                Span::default(),
            );
        }

        let mut current_next = next_bb;
        for (elif_cond, elif_body) in elif_clauses {
            self.current_block = Some(current_next);
            let elif_op = self.lower_expr(elif_cond);
            let elif_then = self.new_block();
            let elif_next = self.new_block();

            self.terminate_block(
                current_next,
                TerminatorKind::SwitchInt {
                    discr: elif_op,
                    targets: vec![(1, elif_then)],
                    otherwise: elif_next,
                },
                elif_cond.span,
            );

            self.current_block = Some(elif_then);
            self.enter_scope();
            for s in elif_body {
                self.lower_stmt(s);
            }
            self.leave_scope();
            if let Some(bb) = self.current_block {
                self.terminate_block(
                    bb,
                    TerminatorKind::Goto { target: merge_bb },
                    Span::default(),
                );
            }
            current_next = elif_next;
        }

        if let Some(body) = else_body {
            self.current_block = Some(current_next);
            self.enter_scope();
            for s in body {
                self.lower_stmt(s);
            }
            self.leave_scope();
            if let Some(bb) = self.current_block {
                self.terminate_block(
                    bb,
                    TerminatorKind::Goto { target: merge_bb },
                    Span::default(),
                );
            }
        } else if current_next != merge_bb {
            self.terminate_block(
                current_next,
                TerminatorKind::Goto { target: merge_bb },
                Span::default(),
            );
        }

        self.current_block = Some(merge_bb);
    }

    pub(super) fn lower_while(
        &mut self,
        condition: &Expr,
        body: &[Stmt],
        else_body: &Option<Vec<Stmt>>,
    ) {
        let header_bb = self.new_block();
        let body_bb = self.new_block();
        let exit_bb = self.new_block();

        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto { target: header_bb },
                Span::default(),
            );
        }

        self.current_block = Some(header_bb);
        let cond_op = self.lower_expr(condition);

        let else_bb = if else_body.is_some() {
            self.new_block()
        } else {
            exit_bb
        };

        self.terminate_block(
            header_bb,
            TerminatorKind::SwitchInt {
                discr: cond_op,
                targets: vec![(1, body_bb)],
                otherwise: else_bb,
            },
            condition.span,
        );

        self.loop_stack.push(LoopContext {
            header: header_bb,
            exit: exit_bb,
        });
        self.current_block = Some(body_bb);
        self.enter_scope();
        for s in body {
            self.lower_stmt(s);
        }
        self.leave_scope();
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto { target: header_bb },
                Span::default(),
            );
        }
        self.loop_stack.pop();

        if let Some(eb) = else_body {
            self.current_block = Some(else_bb);
            self.enter_scope();
            for s in eb {
                self.lower_stmt(s);
            }
            self.leave_scope();
            if let Some(bb) = self.current_block {
                self.terminate_block(
                    bb,
                    TerminatorKind::Goto { target: exit_bb },
                    Span::default(),
                );
            }
        }

        self.current_block = Some(exit_bb);
    }

    pub(super) fn lower_for(
        &mut self,
        target: &ForTarget,
        iter: &Expr,
        body: &[Stmt],
        else_body: &Option<Vec<Stmt>>,
    ) {
        let iter_expr_op = self.lower_expr(iter);
        let iter_local = self.new_local(Type::Any, Some("_iter_obj".to_string()), true);

        self.push_statement(
            StatementKind::Assign(
                iter_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_iter".to_string())),
                    args: vec![iter_expr_op],
                },
            ),
            iter.span,
        );

        let header_bb = self.new_block();
        let body_bb = self.new_block();
        let exit_bb = self.new_block();

        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto { target: header_bb },
                Span::default(),
            );
        }

        self.current_block = Some(header_bb);
        let has_next = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(
                has_next,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_has_next".to_string())),
                    args: vec![Operand::Copy(iter_local)],
                },
            ),
            iter.span,
        );

        let else_bb = if else_body.is_some() {
            self.new_block()
        } else {
            exit_bb
        };
        self.terminate_block(
            header_bb,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(has_next),
                targets: vec![(1, body_bb)],
                otherwise: else_bb,
            },
            iter.span,
        );

        self.loop_stack.push(LoopContext {
            header: header_bb,
            exit: exit_bb,
        });
        self.current_block = Some(body_bb);
        self.enter_scope();

        let next_val = self.new_local(Type::Any, None, false);
        self.push_statement(
            StatementKind::Assign(
                next_val,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_next".to_string())),
                    args: vec![Operand::Copy(iter_local)],
                },
            ),
            iter.span,
        );

        match target {
            ForTarget::Name(name, _) => {
                let local = self.declare_var(name.clone(), Type::Any, true);
                self.push_statement(
                    StatementKind::Assign(local, Rvalue::Use(Operand::Copy(next_val))),
                    iter.span,
                );
            }
            ForTarget::Tuple(names) => {
                for (i, (name, _)) in names.iter().enumerate() {
                    let local = self.declare_var(name.clone(), Type::Any, true);
                    let idx_op = Operand::Constant(Constant::Int(i as i64));
                    let elem_tmp = self.new_local(Type::Any, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            elem_tmp,
                            Rvalue::GetIndex(Operand::Copy(next_val), idx_op),
                        ),
                        iter.span,
                    );
                    self.push_statement(
                        StatementKind::Assign(local, Rvalue::Use(Operand::Copy(elem_tmp))),
                        iter.span,
                    );
                }
            }
        }

        for s in body {
            self.lower_stmt(s);
        }
        self.leave_scope();
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto { target: header_bb },
                Span::default(),
            );
        }
        self.loop_stack.pop();

        if let Some(eb) = else_body {
            self.current_block = Some(else_bb);
            self.enter_scope();
            for s in eb {
                self.lower_stmt(s);
            }
            self.leave_scope();
            if let Some(bb) = self.current_block {
                self.terminate_block(
                    bb,
                    TerminatorKind::Goto { target: exit_bb },
                    Span::default(),
                );
            }
        }

        self.current_block = Some(exit_bb);
    }
}
