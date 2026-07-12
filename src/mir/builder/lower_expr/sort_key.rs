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
}
