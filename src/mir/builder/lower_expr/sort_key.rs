//! `sort(key=f)` / `sorted(xs, key=f)` lowering (E5.5). Shared by the
//! `sorted` builtin (`call.rs`) and the `.sort()` list method
//! (`call_method.rs`): both end up here once the checker has confirmed `f`
//! is `fn(elem) -> K` with `K` orderable.
//!
//! Two runtime strategies were built and benchmarked: decorate-sort-
//! undecorate (evaluate `f` once per element into a parallel key list, then
//! sort by that -- `O(n)` calls to `f`) and comparator-call (re-evaluate
//! `f` on every comparison, `O(n log n)` calls, no extra allocation).
//! Measured with `hyperfine` on an AOT release build sorting 1e6 strings by
//! length (`key=lambda w: len(w))`, a cheap key so the indirect-call
//! overhead itself dominates rather than the key computation: decorate
//! 2.336s ± 0.030s mean, comparator 2.593s ± 0.031s mean, decorate ~1.11x
//! faster. Decorate won, so it's the only variant left in
//! `std_lib/src/list.rs` (`olive_list_sort_by_keys`) -- the comparator
//! implementation was deleted along with its codegen registration, not
//! kept as a documented alternative, per rule 5.

use super::super::MirBuilder;
use crate::mir::ir::*;
use crate::semantic::types::Type;
use crate::span::Span;

const KEY_INT: i64 = 0;
const KEY_FLOAT: i64 = 1;
const KEY_STR: i64 = 2;

fn key_kind_of(ty: &Type) -> i64 {
    match ty {
        Type::Float | Type::F32 | Type::FloatLiteral(_) => KEY_FLOAT,
        Type::Str => KEY_STR,
        _ => KEY_INT,
    }
}

impl<'a> MirBuilder<'a> {
    /// Sorts `list_op` (already the list to mutate in place -- a copy for
    /// `sorted`, the receiver itself for `.sort()`) by `key_op(elem)` for
    /// each element. Returns the same operand, sorted.
    pub(super) fn lower_sort_by_key(
        &mut self,
        list_op: Operand,
        elem_ty: &Type,
        key_op: Operand,
        key_ret_ty: &Type,
        span: Span,
    ) -> Operand {
        let list_local = match list_op {
            Operand::Copy(l) | Operand::Move(l) => l,
            Operand::Constant(_) => unreachable!("a list value is never a bare constant"),
        };

        let len_local = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                len_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_len".to_string())),
                    args: vec![Operand::Copy(list_local)],
                },
            ),
            span,
        );

        let keys_local = self.new_unscoped_local(Type::List(Box::new(key_ret_ty.clone())));
        self.push_statement(
            StatementKind::Assign(
                keys_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_new".to_string())),
                    // `__olive_list_new(n)` pre-fills `n` zeroed elements
                    // (a fixed-size buffer), not an empty list reserving
                    // capacity `n` -- `0` here plus `append` below is the
                    // same "start empty, grow" shape `data.rs`'s splat-list
                    // lowering already uses.
                    args: vec![Operand::Constant(Constant::Int(0))],
                },
            ),
            span,
        );

        self.enter_scope();
        let i_local = self.new_local(Type::Int, None, true);
        self.push_statement(
            StatementKind::Assign(i_local, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            span,
        );

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let latch_bb = self.new_block();
        let exit_bb = self.new_block();

        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: cond_bb }, span);
        }

        self.current_block = Some(cond_bb);
        let cond = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(
                cond,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Lt,
                    Operand::Copy(i_local),
                    Operand::Copy(len_local),
                ),
            ),
            span,
        );
        self.terminate_block(
            cond_bb,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(cond),
                targets: vec![(1, body_bb)],
                otherwise: exit_bb,
            },
            span,
        );

        self.current_block = Some(body_bb);
        let elem_local = self.new_local_with_owning(elem_ty.clone(), None, true, false);
        self.push_statement(
            StatementKind::Assign(
                elem_local,
                Rvalue::GetIndex(Operand::Copy(list_local), Operand::Copy(i_local), true),
            ),
            span,
        );
        let key_result = self.new_local(key_ret_ty.clone(), None, false);
        self.push_statement(
            StatementKind::Assign(
                key_result,
                Rvalue::Call {
                    func: key_op.clone(),
                    args: vec![Operand::Copy(elem_local)],
                },
            ),
            span,
        );
        let append_result = self.new_local(Type::Null, None, false);
        self.push_statement(
            StatementKind::Assign(
                append_result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_append".to_string())),
                    args: vec![Operand::Copy(keys_local), Operand::Copy(key_result)],
                },
            ),
            span,
        );
        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: latch_bb }, span);
        }

        self.current_block = Some(latch_bb);
        let next_i = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                next_i,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(i_local),
                    Operand::Constant(Constant::Int(1)),
                ),
            ),
            span,
        );
        self.push_statement(
            StatementKind::Assign(i_local, Rvalue::Use(Operand::Copy(next_i))),
            span,
        );
        self.terminate_block(latch_bb, TerminatorKind::Goto { target: cond_bb }, span);

        self.current_block = Some(exit_bb);
        self.leave_scope();

        let key_kind = Operand::Constant(Constant::Int(key_kind_of(key_ret_ty)));
        let sort_result = self.new_local(Type::Null, None, false);
        self.push_statement(
            StatementKind::Assign(
                sort_result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(
                        "__olive_list_sort_by_keys".to_string(),
                    )),
                    args: vec![
                        Operand::Copy(list_local),
                        Operand::Copy(keys_local),
                        key_kind,
                    ],
                },
            ),
            span,
        );
        self.push_statement(StatementKind::Drop(keys_local), span);
        let _ = sort_result;
        Operand::Copy(list_local)
    }

    /// Sorts `list_op` (a struct element list) by its own `__lt__` (E6.3):
    /// no scalar key exists to decorate with, so this computes an index
    /// permutation via a stable insertion sort -- one `__lt__` call per
    /// comparison, always through a borrowed `&Self` read (`Rvalue::Ref`),
    /// never touching the list's own ownership of its elements -- then
    /// permutes the list in one pass (`__olive_list_apply_order`, pure
    /// pointer-word reordering, no user code involved).
    pub(super) fn lower_sort_by_lt(
        &mut self,
        list_op: Operand,
        elem_ty: &Type,
        span: Span,
    ) -> Operand {
        let list_local = match list_op {
            Operand::Copy(l) | Operand::Move(l) => l,
            Operand::Constant(_) => unreachable!("a list value is never a bare constant"),
        };
        let (struct_name, type_args) =
            Self::deref_struct_ty(elem_ty.clone()).expect("caller checked elem_ty is a struct");
        let lt_base = format!("{struct_name}::__lt__");
        let lt_name = if type_args.is_empty() {
            lt_base
        } else {
            self.monomorphize(&lt_base, &type_args)
        };

        let len_local = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                len_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_len".to_string())),
                    args: vec![Operand::Copy(list_local)],
                },
            ),
            span,
        );

        // order = [0, 1, ..., n-1]
        let order_local = self.new_unscoped_local(Type::List(Box::new(Type::Int)));
        self.push_statement(
            StatementKind::Assign(
                order_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_new".to_string())),
                    args: vec![Operand::Constant(Constant::Int(0))],
                },
            ),
            span,
        );

        self.enter_scope();
        let fill_i = self.new_local(Type::Int, None, true);
        self.push_statement(
            StatementKind::Assign(fill_i, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            span,
        );
        let fill_cond_bb = self.new_block();
        let fill_body_bb = self.new_block();
        let fill_latch_bb = self.new_block();
        let fill_exit_bb = self.new_block();
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto {
                    target: fill_cond_bb,
                },
                span,
            );
        }
        self.current_block = Some(fill_cond_bb);
        let fill_cond = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(
                fill_cond,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Lt,
                    Operand::Copy(fill_i),
                    Operand::Copy(len_local),
                ),
            ),
            span,
        );
        self.terminate_block(
            fill_cond_bb,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(fill_cond),
                targets: vec![(1, fill_body_bb)],
                otherwise: fill_exit_bb,
            },
            span,
        );
        self.current_block = Some(fill_body_bb);
        let fill_append = self.new_local(Type::Null, None, false);
        self.push_statement(
            StatementKind::Assign(
                fill_append,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_append".to_string())),
                    args: vec![Operand::Copy(order_local), Operand::Copy(fill_i)],
                },
            ),
            span,
        );
        self.terminate_block(
            fill_body_bb,
            TerminatorKind::Goto {
                target: fill_latch_bb,
            },
            span,
        );
        self.current_block = Some(fill_latch_bb);
        let fill_next = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                fill_next,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(fill_i),
                    Operand::Constant(Constant::Int(1)),
                ),
            ),
            span,
        );
        self.push_statement(
            StatementKind::Assign(fill_i, Rvalue::Use(Operand::Copy(fill_next))),
            span,
        );
        self.terminate_block(
            fill_latch_bb,
            TerminatorKind::Goto {
                target: fill_cond_bb,
            },
            span,
        );
        self.current_block = Some(fill_exit_bb);
        self.leave_scope();

        // Stable insertion sort over `order`, comparing the struct elements
        // it indexes: for i in 1..n, insert order[i] into the already-sorted
        // order[0..i) by shifting while `__lt__` says the new element
        // belongs earlier. Strict `<` never moves an equal element past
        // another, so ties keep their original order.
        self.enter_scope();
        let i_local = self.new_local(Type::Int, None, true);
        self.push_statement(
            StatementKind::Assign(i_local, Rvalue::Use(Operand::Constant(Constant::Int(1)))),
            span,
        );
        let outer_cond_bb = self.new_block();
        let outer_body_bb = self.new_block();
        let outer_latch_bb = self.new_block();
        let outer_exit_bb = self.new_block();
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto {
                    target: outer_cond_bb,
                },
                span,
            );
        }
        self.current_block = Some(outer_cond_bb);
        let outer_cond = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(
                outer_cond,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Lt,
                    Operand::Copy(i_local),
                    Operand::Copy(len_local),
                ),
            ),
            span,
        );
        self.terminate_block(
            outer_cond_bb,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(outer_cond),
                targets: vec![(1, outer_body_bb)],
                otherwise: outer_exit_bb,
            },
            span,
        );

        self.current_block = Some(outer_body_bb);
        let key_idx = self.new_local(Type::Int, None, true);
        self.push_statement(
            StatementKind::Assign(
                key_idx,
                Rvalue::GetIndex(Operand::Copy(order_local), Operand::Copy(i_local), true),
            ),
            span,
        );
        let j_local = self.new_local(Type::Int, None, true);
        self.push_statement(
            StatementKind::Assign(
                j_local,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Sub,
                    Operand::Copy(i_local),
                    Operand::Constant(Constant::Int(1)),
                ),
            ),
            span,
        );
        let inner_cond_bb = self.new_block();
        let inner_test_lt_bb = self.new_block();
        let inner_shift_bb = self.new_block();
        let inner_done_bb = self.new_block();
        self.terminate_block(
            outer_body_bb,
            TerminatorKind::Goto {
                target: inner_cond_bb,
            },
            span,
        );

        self.current_block = Some(inner_cond_bb);
        let j_ge_zero = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(
                j_ge_zero,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::GtEq,
                    Operand::Copy(j_local),
                    Operand::Constant(Constant::Int(0)),
                ),
            ),
            span,
        );
        self.terminate_block(
            inner_cond_bb,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(j_ge_zero),
                targets: vec![(1, inner_test_lt_bb)],
                otherwise: inner_done_bb,
            },
            span,
        );

        self.current_block = Some(inner_test_lt_bb);
        let order_j = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                order_j,
                Rvalue::GetIndex(Operand::Copy(order_local), Operand::Copy(j_local), true),
            ),
            span,
        );
        let elem_key = self.new_local_with_owning(elem_ty.clone(), None, false, false);
        self.push_statement(
            StatementKind::Assign(
                elem_key,
                Rvalue::GetIndex(Operand::Copy(list_local), Operand::Copy(key_idx), true),
            ),
            span,
        );
        let elem_j = self.new_local_with_owning(elem_ty.clone(), None, false, false);
        self.push_statement(
            StatementKind::Assign(
                elem_j,
                Rvalue::GetIndex(Operand::Copy(list_local), Operand::Copy(order_j), true),
            ),
            span,
        );
        let ref_ty = Type::Ref(Box::new(elem_ty.clone()));
        let ref_key = self.new_local_with_owning(ref_ty.clone(), None, false, false);
        self.push_statement(StatementKind::Assign(ref_key, Rvalue::Ref(elem_key)), span);
        let ref_j = self.new_local_with_owning(ref_ty, None, false, false);
        self.push_statement(StatementKind::Assign(ref_j, Rvalue::Ref(elem_j)), span);
        let lt_result = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(
                lt_result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(lt_name.clone())),
                    args: vec![Operand::Copy(ref_key), Operand::Copy(ref_j)],
                },
            ),
            span,
        );
        self.terminate_block(
            inner_test_lt_bb,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(lt_result),
                targets: vec![(1, inner_shift_bb)],
                otherwise: inner_done_bb,
            },
            span,
        );

        self.current_block = Some(inner_shift_bb);
        let j_plus_1 = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                j_plus_1,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(j_local),
                    Operand::Constant(Constant::Int(1)),
                ),
            ),
            span,
        );
        self.push_statement(
            StatementKind::SetIndex(
                Operand::Copy(order_local),
                Operand::Copy(j_plus_1),
                Operand::Copy(order_j),
                true,
            ),
            span,
        );
        let j_minus_1 = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                j_minus_1,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Sub,
                    Operand::Copy(j_local),
                    Operand::Constant(Constant::Int(1)),
                ),
            ),
            span,
        );
        self.push_statement(
            StatementKind::Assign(j_local, Rvalue::Use(Operand::Copy(j_minus_1))),
            span,
        );
        self.terminate_block(
            inner_shift_bb,
            TerminatorKind::Goto {
                target: inner_cond_bb,
            },
            span,
        );

        self.current_block = Some(inner_done_bb);
        let final_slot = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                final_slot,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(j_local),
                    Operand::Constant(Constant::Int(1)),
                ),
            ),
            span,
        );
        self.push_statement(
            StatementKind::SetIndex(
                Operand::Copy(order_local),
                Operand::Copy(final_slot),
                Operand::Copy(key_idx),
                true,
            ),
            span,
        );
        self.terminate_block(
            inner_done_bb,
            TerminatorKind::Goto {
                target: outer_latch_bb,
            },
            span,
        );

        self.current_block = Some(outer_latch_bb);
        let i_plus_1 = self.new_local(Type::Int, None, false);
        self.push_statement(
            StatementKind::Assign(
                i_plus_1,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(i_local),
                    Operand::Constant(Constant::Int(1)),
                ),
            ),
            span,
        );
        self.push_statement(
            StatementKind::Assign(i_local, Rvalue::Use(Operand::Copy(i_plus_1))),
            span,
        );
        self.terminate_block(
            outer_latch_bb,
            TerminatorKind::Goto {
                target: outer_cond_bb,
            },
            span,
        );

        self.current_block = Some(outer_exit_bb);
        self.leave_scope();

        let apply_result = self.new_local(Type::Null, None, false);
        self.push_statement(
            StatementKind::Assign(
                apply_result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(
                        "__olive_list_apply_order".to_string(),
                    )),
                    args: vec![Operand::Copy(list_local), Operand::Copy(order_local)],
                },
            ),
            span,
        );
        self.push_statement(StatementKind::Drop(order_local), span);
        let _ = apply_result;
        Operand::Copy(list_local)
    }
}
