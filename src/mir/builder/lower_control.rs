use super::LoopContext;
use super::MirBuilder;
use crate::mir::ir::*;
use crate::parser::{BinOp, CallArg, Expr, ExprKind, ForTarget, Stmt};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    /// Lowers a branch/loop condition to a 0/1 discriminant.
    fn lower_condition(&mut self, condition: &Expr) -> Operand {
        let op = self.lower_expr(condition);
        let ty = self.get_type(condition.id);
        self.truthify(op, &ty, condition.span)
    }

    /// Reduces an operand to a 0/1 discriminant. An `Any` goes through
    /// `__olive_any_truthy`; a bool passes through.
    pub(crate) fn truthify(&mut self, op: Operand, ty: &Type, span: Span) -> Operand {
        if *ty != Type::Any {
            return op;
        }
        let tmp = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(
                tmp,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_any_truthy".to_string())),
                    args: vec![op],
                },
            ),
            span,
        );
        self.operand_for_local(tmp)
    }

    pub(super) fn lower_if(
        &mut self,
        condition: &Expr,
        then_body: &[Stmt],
        elif_clauses: &[(Expr, Vec<Stmt>)],
        else_body: &Option<Vec<Stmt>>,
        is_tail: bool,
    ) {
        let cond_op = self.lower_condition(condition);
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
        for (i, s) in then_body.iter().enumerate() {
            self.lower_stmt_with_tail(s, is_tail && i == then_body.len() - 1);
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
            let elif_op = self.lower_condition(elif_cond);
            let elif_then = self.new_block();
            let elif_next = self.new_block();

            if let Some(bb) = self.current_block {
                self.terminate_block(
                    bb,
                    TerminatorKind::SwitchInt {
                        discr: elif_op,
                        targets: vec![(1, elif_then)],
                        otherwise: elif_next,
                    },
                    elif_cond.span,
                );
            }

            self.current_block = Some(elif_then);
            self.enter_scope();
            for (i, s) in elif_body.iter().enumerate() {
                self.lower_stmt_with_tail(s, is_tail && i == elif_body.len() - 1);
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
            for (i, s) in body.iter().enumerate() {
                self.lower_stmt_with_tail(s, is_tail && i == body.len() - 1);
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
        let cond_op = self.lower_condition(condition);

        let else_bb = if else_body.is_some() {
            self.new_block()
        } else {
            exit_bb
        };

        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::SwitchInt {
                    discr: cond_op,
                    targets: vec![(1, body_bb)],
                    otherwise: else_bb,
                },
                condition.span,
            );
        }

        self.loop_stack.push(LoopContext {
            header: header_bb,
            exit: exit_bb,
            scope_depth: self.scope_locals.len(),
            cleanup: None,
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
        if let ExprKind::Range {
            start,
            end,
            inclusive,
        } = &iter.kind
            && let ForTarget::Name(name, _) = target
        {
            self.lower_for_range(name, start, end, *inclusive, body, else_body);
            return;
        }

        // Desugar enumerate(iterable) in for-loop position:
        // for (i, x) in enumerate(xs): body
        // becomes:
        // let _counter = 0
        // let _iter = iter(xs)
        // while has_next(_iter):
        //   let x = next(_iter)
        //   let i = _counter
        //   _counter = _counter + 1
        //   body
        if let ExprKind::Call { callee, args } = &iter.kind
            && let ExprKind::Identifier(name) = &callee.kind
            && name == "enumerate"
            && !args.is_empty()
        {
            let inner_expr = match &args[0] {
                CallArg::Positional(e)
                | CallArg::Keyword(_, e)
                | CallArg::Splat(e)
                | CallArg::KwSplat(e) => e,
            };
            let inner_op = self.lower_expr(inner_expr);
            let inner_ty = self.get_type(inner_expr.id);
            let inner_copy = self.new_local(inner_ty, None, true);
            self.push_statement(
                StatementKind::Assign(inner_copy, Rvalue::Use(inner_op)),
                iter.span,
            );
            let iter_local = self.new_local(Type::Any, Some("_iter".to_string()), true);
            self.push_statement(
                StatementKind::Assign(
                    iter_local,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("__olive_iter".to_string())),
                        args: vec![Operand::Copy(inner_copy)],
                    },
                ),
                iter.span,
            );
            let counter = self.new_local(Type::Int, Some("_count".to_string()), true);
            self.push_statement(
                StatementKind::Assign(counter, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
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
                scope_depth: self.scope_locals.len(),
                cleanup: Some(iter_local),
            });
            self.current_block = Some(body_bb);
            self.enter_scope();

            let mut inner_iter_ty = self.get_type(inner_expr.id);
            while let Type::Ref(inner) | Type::MutRef(inner) = inner_iter_ty {
                inner_iter_ty = *inner;
            }
            let next_is_owning = matches!(inner_iter_ty, Type::Str);
            let elem_ty = match inner_iter_ty {
                Type::Str => Type::Str,
                Type::List(t) | Type::Set(t) => *t,
                Type::Dict(k, _) => *k,
                _ => Type::Any,
            };
            let next_val = self.new_local_with_owning(elem_ty, None, false, next_is_owning);
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
                ForTarget::Tuple(names) => {
                    if let Some((i_name, _)) = names.first() {
                        let i_local = self.declare_var(i_name.clone(), Type::Int, true);
                        self.push_statement(
                            StatementKind::Assign(i_local, Rvalue::Use(Operand::Copy(counter))),
                            iter.span,
                        );
                    }
                    if let Some((x_name, _)) = names.get(1) {
                        let view_local = self.declare_var_view(x_name.clone(), Type::Any, true);
                        let elem_copy = self.new_local_with_owning(Type::Any, None, false, false);
                        self.push_statement(
                            StatementKind::Assign(elem_copy, Rvalue::Use(Operand::Copy(next_val))),
                            iter.span,
                        );
                        self.push_statement(
                            StatementKind::Assign(
                                view_local,
                                Rvalue::Use(Operand::Copy(elem_copy)),
                            ),
                            iter.span,
                        );
                    }
                }
                ForTarget::Name(name, _) => {
                    let local = self.declare_var(
                        name.clone(),
                        Type::Tuple(vec![Type::Int, Type::Any]),
                        true,
                    );
                    let tuple_local =
                        self.new_local(Type::Tuple(vec![Type::Int, Type::Any]), None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            tuple_local,
                            Rvalue::Aggregate(
                                AggregateKind::Tuple,
                                vec![Operand::Copy(counter), Operand::Copy(next_val)],
                            ),
                        ),
                        iter.span,
                    );
                    self.push_statement(
                        StatementKind::Assign(local, Rvalue::Use(Operand::Copy(tuple_local))),
                        iter.span,
                    );
                }
            }

            let one = Operand::Constant(Constant::Int(1));
            self.push_statement(
                StatementKind::Assign(
                    counter,
                    Rvalue::BinaryOp(BinOp::Add, Operand::Copy(counter), one),
                ),
                iter.span,
            );

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
            return;
        }

        // Desugar zip(a, b) in for-loop position:
        // for (a, b) in zip(xs, ys): body
        // becomes:
        // let _copy_xs = copy(xs)
        // let _copy_ys = copy(ys)
        // let _len_xs = len(_copy_xs)
        // let _len_ys = len(_copy_ys)
        // let _min_len = if _len_xs < _len_ys { _len_xs } else { _len_ys }
        // let _i = 0
        // while _i < _min_len:
        //   let a = @_copy_xs[_i]
        //   let b = @_copy_ys[_i]
        //   _i = _i + 1
        //   body
        if let ExprKind::Call { callee, args } = &iter.kind
            && let ExprKind::Identifier(name) = &callee.kind
            && name == "zip"
            && args.len() >= 2
        {
            let a_expr = match &args[0] {
                CallArg::Positional(e)
                | CallArg::Keyword(_, e)
                | CallArg::Splat(e)
                | CallArg::KwSplat(e) => e,
            };
            let b_expr = match &args[1] {
                CallArg::Positional(e)
                | CallArg::Keyword(_, e)
                | CallArg::Splat(e)
                | CallArg::KwSplat(e) => e,
            };

            let a_op = self.lower_expr(a_expr);
            let a_ty = self.get_type(a_expr.id);
            let b_op = self.lower_expr(b_expr);
            let b_ty = self.get_type(b_expr.id);

            let copy_a = self.new_local(a_ty.clone(), None, true);
            self.push_statement(StatementKind::Assign(copy_a, Rvalue::Use(a_op)), iter.span);
            let copy_b = self.new_local(b_ty.clone(), None, true);
            self.push_statement(StatementKind::Assign(copy_b, Rvalue::Use(b_op)), iter.span);

            let len_a_fn = MirBuilder::<'a>::len_runtime_fn(&a_ty);
            let len_b_fn = MirBuilder::<'a>::len_runtime_fn(&b_ty);
            let len_a = self.new_local(Type::Int, None, false);
            self.push_statement(
                StatementKind::Assign(
                    len_a,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(len_a_fn.to_string())),
                        args: vec![Operand::Copy(copy_a)],
                    },
                ),
                iter.span,
            );
            let len_b = self.new_local(Type::Int, None, false);
            self.push_statement(
                StatementKind::Assign(
                    len_b,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(len_b_fn.to_string())),
                        args: vec![Operand::Copy(copy_b)],
                    },
                ),
                iter.span,
            );

            let cmp = self.new_local(Type::Bool, None, false);
            self.push_statement(
                StatementKind::Assign(
                    cmp,
                    Rvalue::BinaryOp(BinOp::Lt, Operand::Copy(len_a), Operand::Copy(len_b)),
                ),
                iter.span,
            );

            let min_bb = self.new_block();
            let max_bb = self.new_block();
            let header_bb = self.new_block();
            let body_bb = self.new_block();
            let exit_bb = self.new_block();

            if let Some(bb) = self.current_block {
                self.terminate_block(
                    bb,
                    TerminatorKind::SwitchInt {
                        discr: Operand::Copy(cmp),
                        targets: vec![(1, min_bb)],
                        otherwise: max_bb,
                    },
                    iter.span,
                );
            }

            let min_len = self.new_local(Type::Int, Some("_min_len".to_string()), true);
            self.current_block = Some(min_bb);
            self.push_statement(
                StatementKind::Assign(min_len, Rvalue::Use(Operand::Copy(len_a))),
                iter.span,
            );
            self.terminate_block(
                min_bb,
                TerminatorKind::Goto { target: header_bb },
                Span::default(),
            );

            self.current_block = Some(max_bb);
            self.push_statement(
                StatementKind::Assign(min_len, Rvalue::Use(Operand::Copy(len_b))),
                iter.span,
            );
            self.terminate_block(
                max_bb,
                TerminatorKind::Goto { target: header_bb },
                Span::default(),
            );

            self.current_block = Some(header_bb);
            let index = self.new_local(Type::Int, Some("_i".to_string()), true);
            self.push_statement(
                StatementKind::Assign(index, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
                iter.span,
            );
            let cond = self.new_local(Type::Bool, None, false);
            self.push_statement(
                StatementKind::Assign(
                    cond,
                    Rvalue::BinaryOp(BinOp::Lt, Operand::Copy(index), Operand::Copy(min_len)),
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
                    discr: Operand::Copy(cond),
                    targets: vec![(1, body_bb)],
                    otherwise: else_bb,
                },
                iter.span,
            );

            self.loop_stack.push(LoopContext {
                header: header_bb,
                exit: exit_bb,
                scope_depth: self.scope_locals.len(),
                cleanup: None,
            });
            self.current_block = Some(body_bb);
            self.enter_scope();

            let a_elem_ty = MirBuilder::<'a>::elem_type_for(&a_ty);
            let b_elem_ty = MirBuilder::<'a>::elem_type_for(&b_ty);

            let a_val = self.new_local_with_owning(a_elem_ty.clone(), None, false, false);
            self.push_statement(
                StatementKind::Assign(
                    a_val,
                    Rvalue::GetIndex(Operand::Copy(copy_a), Operand::Copy(index), false),
                ),
                iter.span,
            );
            let b_val = self.new_local_with_owning(b_elem_ty.clone(), None, false, false);
            self.push_statement(
                StatementKind::Assign(
                    b_val,
                    Rvalue::GetIndex(Operand::Copy(copy_b), Operand::Copy(index), false),
                ),
                iter.span,
            );

            match target {
                ForTarget::Tuple(names) => {
                    if let Some((an, _)) = names.first() {
                        let a_local = self.declare_var(an.clone(), a_elem_ty, true);
                        self.push_statement(
                            StatementKind::Assign(a_local, Rvalue::Use(Operand::Copy(a_val))),
                            iter.span,
                        );
                    }
                    if let Some((bn, _)) = names.get(1) {
                        let b_local = self.declare_var(bn.clone(), b_elem_ty, true);
                        self.push_statement(
                            StatementKind::Assign(b_local, Rvalue::Use(Operand::Copy(b_val))),
                            iter.span,
                        );
                    }
                }
                ForTarget::Name(name, _) => {
                    let local = self.declare_var(
                        name.clone(),
                        Type::Tuple(vec![a_elem_ty, b_elem_ty]),
                        true,
                    );
                    let tuple_local =
                        self.new_local(Type::Tuple(vec![Type::Any, Type::Any]), None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            tuple_local,
                            Rvalue::Aggregate(
                                AggregateKind::Tuple,
                                vec![Operand::Copy(a_val), Operand::Copy(b_val)],
                            ),
                        ),
                        iter.span,
                    );
                    self.push_statement(
                        StatementKind::Assign(local, Rvalue::Use(Operand::Copy(tuple_local))),
                        iter.span,
                    );
                }
            }

            let one = Operand::Constant(Constant::Int(1));
            self.push_statement(
                StatementKind::Assign(
                    index,
                    Rvalue::BinaryOp(BinOp::Add, Operand::Copy(index), one),
                ),
                iter.span,
            );

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
            return;
        }

        let iter_expr_op = self.lower_expr(iter);
        let iter_ty = self.get_type(iter.id);
        let iter_copy = self.new_local(iter_ty, None, true);
        self.push_statement(
            StatementKind::Assign(iter_copy, Rvalue::Use(iter_expr_op)),
            iter.span,
        );
        let iter_local = self.new_local(Type::Any, Some("_iter_obj".to_string()), true);

        self.push_statement(
            StatementKind::Assign(
                iter_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_iter".to_string())),
                    args: vec![Operand::Copy(iter_copy)],
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
            scope_depth: self.scope_locals.len(),
            cleanup: Some(iter_local),
        });
        self.current_block = Some(body_bb);
        self.enter_scope();

        // Type by element so typed values dispatch correctly, not via `Any`.
        let mut iter_ty = self.get_type(iter.id);
        while let Type::Ref(inner) | Type::MutRef(inner) = iter_ty {
            iter_ty = *inner;
        }
        // `__olive_next` never copies; only str iteration yields a fresh value.
        let next_is_owning = matches!(iter_ty, Type::Str);
        let elem_ty = match iter_ty {
            Type::Str => Type::Str,
            Type::List(t) | Type::Set(t) => *t,
            Type::Dict(k, _) => *k,
            _ => Type::Any,
        };

        let next_val = self.new_local_with_owning(elem_ty.clone(), None, false, next_is_owning);
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
                let local = self.declare_var(name.clone(), elem_ty, true);
                self.push_statement(
                    StatementKind::Assign(local, Rvalue::Use(Operand::Copy(next_val))),
                    iter.span,
                );
            }
            ForTarget::Tuple(names) => {
                let comp_tys: Vec<Type> = match &elem_ty {
                    Type::Tuple(comps) => comps.clone(),
                    _ => Vec::new(),
                };
                for (i, (name, _)) in names.iter().enumerate() {
                    // Type each binding by its tuple component so operators and
                    // methods dispatch correctly rather than via the `Any` path.
                    let comp_ty = comp_tys.get(i).cloned().unwrap_or(Type::Any);
                    // Each binding is a view into the element, which the iterable
                    // owns, so it must not be freed here (avoids a double free
                    // when both bindings are consumed in one expression).
                    let local = self.declare_var_view(name.clone(), comp_ty.clone(), true);
                    let idx_op = Operand::Constant(Constant::Int(i as i64));
                    let elem_tmp = self.new_local_with_owning(comp_ty, None, false, false);
                    self.push_statement(
                        StatementKind::Assign(
                            elem_tmp,
                            Rvalue::GetIndex(Operand::Copy(next_val), idx_op, false),
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
        // Every exit path (loop done, break, else-body) converges here; free
        // the iterator once. Freeing again on a rare path is a no-op.
        self.emit_iter_free(iter_local);
    }

    /// Lowers `for name in start..end` to a counted loop, avoiding any iterator
    /// allocation. `continue` lands on the latch so the counter still advances.
    fn lower_for_range(
        &mut self,
        name: &str,
        start: &Expr,
        end: &Expr,
        inclusive: bool,
        body: &[Stmt],
        else_body: &Option<Vec<Stmt>>,
    ) {
        let start_op = self.lower_expr(start);
        let end_op = self.lower_expr(end);
        let end_local = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(end_local, Rvalue::Use(end_op)),
            end.span,
        );

        self.enter_scope();
        let i_local = self.declare_var(name.to_string(), Type::Int, true);
        self.push_statement(
            StatementKind::Assign(i_local, Rvalue::Use(start_op)),
            start.span,
        );

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let latch_bb = self.new_block();
        let exit_bb = self.new_block();

        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: cond_bb }, start.span);
        }

        self.current_block = Some(cond_bb);
        let cond = self.new_local(Type::Bool, None, false);
        let cmp = if inclusive {
            crate::parser::BinOp::LtEq
        } else {
            crate::parser::BinOp::Lt
        };
        self.push_statement(
            StatementKind::Assign(
                cond,
                Rvalue::BinaryOp(cmp, Operand::Copy(i_local), Operand::Copy(end_local)),
            ),
            start.span,
        );
        let after_bb = if else_body.is_some() {
            self.new_block()
        } else {
            exit_bb
        };
        self.terminate_block(
            cond_bb,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(cond),
                targets: vec![(1, body_bb)],
                otherwise: after_bb,
            },
            start.span,
        );

        self.loop_stack.push(LoopContext {
            header: latch_bb,
            exit: exit_bb,
            // The loop's own scope (`i_local` + body) was already entered above,
            // unlike the other loop forms, so it sits one frame lower.
            scope_depth: self.scope_locals.len().saturating_sub(1),
            cleanup: None,
        });
        self.current_block = Some(body_bb);
        for s in body {
            self.lower_stmt(s);
        }
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto { target: latch_bb },
                Span::default(),
            );
        }

        self.current_block = Some(latch_bb);
        self.push_statement(
            StatementKind::Assign(
                i_local,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(i_local),
                    Operand::Constant(Constant::Int(1)),
                ),
            ),
            Span::default(),
        );
        self.terminate_block(
            latch_bb,
            TerminatorKind::Goto { target: cond_bb },
            Span::default(),
        );
        self.loop_stack.pop();

        if let Some(eb) = else_body {
            self.current_block = Some(after_bb);
            for s in eb {
                self.lower_stmt(s);
            }
            if let Some(bb) = self.current_block {
                self.terminate_block(
                    bb,
                    TerminatorKind::Goto { target: exit_bb },
                    Span::default(),
                );
            }
        }

        self.leave_scope();
        self.current_block = Some(exit_bb);
    }

    fn len_runtime_fn(ty: &Type) -> &'static str {
        match ty {
            Type::Str => "__olive_str_len",
            Type::Dict(_, _) | Type::Any | Type::Struct(_, _, _) => "__olive_obj_len",
            Type::Bytes => "__olive_buf_len",
            _ => "__olive_list_len",
        }
    }

    fn elem_type_for(ty: &Type) -> Type {
        let mut t = ty;
        while let Type::Ref(inner) | Type::MutRef(inner) = t {
            t = inner;
        }
        match t {
            Type::Str => Type::Str,
            Type::List(inner) | Type::Set(inner) => *inner.clone(),
            Type::Dict(k, _) => *k.clone(),
            _ => Type::Any,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::MirBuilder;
    use crate::lexer::Lexer;
    use crate::mir::ir::{StatementKind, TerminatorKind};
    use crate::parser::Parser;
    use crate::semantic::{Resolver, TypeChecker};
    use rustc_hash::FxHashSet;

    fn build(src: &str) -> Vec<super::super::super::ir::MirFunction> {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        let mut tc = TypeChecker::new();
        tc.check_program(&prog);
        let mut builder = MirBuilder::new(
            &tc.expr_types,
            &tc.expr_kwarg_maps,
            &tc.type_env[0],
            tc.struct_fields.clone(),
            &tc.traits,
            FxHashSet::default(),
        );
        builder.build_program(&prog);
        builder.functions
    }

    #[test]
    fn if_statement_creates_switch() {
        let fns = build("fn f(x: i64) -> i64:\n    if x > 0:\n        return 1\n    return 0\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_switch = f.basic_blocks.iter().any(|bb| {
            bb.terminator
                .as_ref()
                .is_some_and(|t| matches!(t.kind, TerminatorKind::SwitchInt { .. }))
        });
        assert!(has_switch);
    }

    #[test]
    fn if_else_creates_multiple_blocks() {
        let fns = build(
            "fn f(x: i64) -> i64:\n    if x > 0:\n        return 1\n    else:\n        return -1\n",
        );
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            f.basic_blocks.len() >= 3,
            "expected at least 3 blocks for if-else"
        );
    }

    #[test]
    fn while_loop_creates_backedge() {
        let fns = build(
            "fn f(n: i64) -> i64:\n    let i = 0\n    while i < n:\n        i = i + 1\n    return i\n",
        );
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_goto = f.basic_blocks.iter().any(|bb| {
            bb.terminator
                .as_ref()
                .is_some_and(|t| matches!(t.kind, TerminatorKind::Goto { .. }))
        });
        assert!(has_goto);
    }

    #[test]
    fn for_loop_emits_iter_call() {
        let fns = build(
            "fn f(xs: [i64]) -> i64:\n    let s = 0\n    for x in xs:\n        s = s + x\n    return s\n",
        );
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_call = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    s.kind,
                    StatementKind::Assign(_, crate::mir::ir::Rvalue::Call { .. })
                )
            })
        });
        assert!(has_call);
    }

    #[test]
    fn nested_if_elif_else_works() {
        let fns = build(
            "fn sign(x: i64) -> i64:\n    if x > 0:\n        return 1\n    elif x < 0:\n        return -1\n    else:\n        return 0\n",
        );
        let f = fns.iter().find(|f| f.name == "sign").unwrap();
        assert!(
            f.basic_blocks.len() >= 4,
            "nested if-elif-else should produce multiple blocks"
        );
    }
}
