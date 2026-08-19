use super::super::MirBuilder;
use crate::mir::ir::*;
use crate::parser::BinOp;
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    /// Whether a value of this type can sit raw in an `Any` slot. Only
    /// slab-kind-tagged words are self-describing there; a raw struct
    /// pointer misreads by kind (its header word is a field count, not a
    /// kind tag), and so does anything transitively holding one. Enums
    /// carry a real kind header plus their own descriptor, so they stay.
    pub(crate) fn any_needs_erase(ty: &Type) -> bool {
        match ty {
            Type::Struct(..) => true,
            Type::List(e) | Type::Set(e) => Self::any_needs_erase(e),
            Type::Dict(k, v) => Self::any_needs_erase(k) || Self::any_needs_erase(v),
            Type::Tuple(members) => members.iter().any(Self::any_needs_erase),
            Type::Union(members) => {
                let non_null: Vec<&Type> = members
                    .iter()
                    .filter(|m| !matches!(m, Type::Null))
                    .collect();
                match non_null.as_slice() {
                    [single] => Self::any_needs_erase(single),
                    // Multi-member unions erase by runtime kind, which no
                    // static descriptor can name: unchanged behavior.
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Erases a tuple's elements into `Any`, rebuilding the same shape
    /// with each position boxed through `box_into_any`. A tuple holding
    /// a raw struct (directly or nested) misreads by kind on the untyped
    /// free path, exactly like the list and set shapes above.
    pub(super) fn erase_tuple_elements(
        &mut self,
        source: Operand,
        members: &[Type],
        span: Span,
    ) -> Operand {
        let mut boxed: Vec<Operand> = Vec::with_capacity(members.len());
        for (i, member) in members.iter().enumerate() {
            let item = self.new_local_with_owning(member.clone(), None, false, false);
            self.push_statement(
                StatementKind::Assign(
                    item,
                    Rvalue::GetIndex(
                        source.clone(),
                        Operand::Constant(Constant::Int(i as i64)),
                        false,
                    ),
                ),
                span,
            );
            boxed.push(self.box_into_any(Operand::Copy(item), member, span));
        }
        let rebuilt = self.new_local(
            Type::Tuple(members.iter().map(|_| Type::Any).collect()),
            None,
            false,
        );
        self.push_statement(
            StatementKind::Assign(rebuilt, Rvalue::Aggregate(AggregateKind::Tuple, boxed)),
            span,
        );
        Operand::Copy(rebuilt)
    }

    /// Erases a set's elements into `Any` the way `erase_list_elements`
    /// does for lists, so a set crossing into an `Any` slot carries
    /// self-describing words: raw struct payloads would otherwise misread
    /// by kind on the untyped free path (a 1-field header is `KIND_LIST`).
    /// Iterates a typed snapshot (struct elements copy exactly), boxes
    /// each element through `box_into_any`, and collects into a fresh
    /// `Set(Any)`. Ownership and drops flow through the same escape-copy
    /// machinery as the list version: the source keeps its ownership, the
    /// new set owns independent words, and the snapshot frees typed at
    /// scope end.
    pub(super) fn erase_set_elements(
        &mut self,
        source: Operand,
        element: &Type,
        span: Span,
    ) -> Operand {
        let result = self.new_local(Type::Set(Box::new(Type::Any)), None, false);
        self.push_statement(
            StatementKind::Assign(
                result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_set_new".into())),
                    args: vec![Operand::Constant(Constant::Int(0))],
                },
            ),
            span,
        );
        self.enter_scope();
        let iter = self.new_local(Type::Any, Some("_iter_obj".to_string()), true);
        self.push_statement(
            StatementKind::Assign(
                iter,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_iter_typed".into())),
                    args: vec![source],
                },
            ),
            span,
        );
        let header = self.new_block();
        let body = self.new_block();
        let done = self.new_block();
        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: header }, span);
        }
        self.current_block = Some(header);
        let more = self.new_unscoped_local(Type::Bool);
        self.push_statement(
            StatementKind::Assign(
                more,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_has_next".into())),
                    args: vec![Operand::Copy(iter)],
                },
            ),
            span,
        );
        self.terminate_block(
            header,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(more),
                targets: vec![(1, body)],
                otherwise: done,
            },
            span,
        );
        self.current_block = Some(body);
        let item = self.new_local_with_owning(element.clone(), None, false, false);
        self.push_statement(
            StatementKind::Assign(
                item,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_next".into())),
                    args: vec![Operand::Copy(iter)],
                },
            ),
            span,
        );
        let erased = self.box_into_any(Operand::Copy(item), element, span);
        let void_sink = self.new_local(Type::Null, None, false);
        self.push_statement(
            StatementKind::Assign(
                void_sink,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_set_add".into())),
                    args: vec![Operand::Copy(result), erased],
                },
            ),
            span,
        );
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: header },
            span,
        );
        self.current_block = Some(done);
        self.leave_scope();
        Operand::Copy(result)
    }

    /// Erases a dict's values into `Any` for an `Any`-bound dict: struct
    /// payloads misread by kind on the untyped free path, so each value
    /// boxes through `box_into_any` while keys keep their static type
    /// (structural key hashing is preserved). Produces `Dict(K, Any)`,
    /// exactly the shape a hand-built `Any`-valued dict has: values stored
    /// boxed, keys by the static key type. Values snapshot typed through
    /// `__olive_obj_items_typed`, so struct keys and values both copy
    /// exactly; stores go through `SetIndex`, whose codegen and
    /// escape-copy handling match user `d[k] = v` stores.
    pub(super) fn erase_dict_values(
        &mut self,
        source: Operand,
        key: &Type,
        value: &Type,
        span: Span,
    ) -> Operand {
        let result = self.new_local(
            Type::Dict(Box::new(key.clone()), Box::new(Type::Any)),
            None,
            false,
        );
        self.push_statement(
            StatementKind::Assign(
                result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_obj_new".into())),
                    args: vec![],
                },
            ),
            span,
        );
        let pairs = self.new_local(
            Type::List(Box::new(Type::Tuple(vec![key.clone(), value.clone()]))),
            None,
            false,
        );
        self.push_statement(
            StatementKind::Assign(
                pairs,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_obj_items_typed".into())),
                    args: vec![source],
                },
            ),
            span,
        );
        let length = self.new_unscoped_local(Type::Int);
        self.push_statement(
            StatementKind::Assign(
                length,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_len".into())),
                    args: vec![Operand::Copy(pairs)],
                },
            ),
            span,
        );
        let index = self.new_unscoped_local(Type::Int);
        self.push_statement(
            StatementKind::Assign(index, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            span,
        );
        let condition = self.new_block();
        let body = self.new_block();
        let done = self.new_block();
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: condition },
            span,
        );
        self.current_block = Some(condition);
        let more = self.new_unscoped_local(Type::Bool);
        self.push_statement(
            StatementKind::Assign(
                more,
                Rvalue::BinaryOp(BinOp::Lt, Operand::Copy(index), Operand::Copy(length)),
            ),
            span,
        );
        self.terminate_block(
            condition,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(more),
                targets: vec![(1, body)],
                otherwise: done,
            },
            span,
        );
        self.current_block = Some(body);
        let pair = self.new_local_with_owning(
            Type::Tuple(vec![key.clone(), value.clone()]),
            None,
            false,
            false,
        );
        self.push_statement(
            StatementKind::Assign(
                pair,
                Rvalue::GetIndex(Operand::Copy(pairs), Operand::Copy(index), false),
            ),
            span,
        );
        let key_tmp = self.new_local_with_owning(key.clone(), None, false, false);
        self.push_statement(
            StatementKind::Assign(
                key_tmp,
                Rvalue::GetIndex(
                    Operand::Copy(pair),
                    Operand::Constant(Constant::Int(0)),
                    false,
                ),
            ),
            span,
        );
        // Keys are stored raw (only `str` gets a private copy inside the
        // store), and no escape rule covers the key position, so a view
        // into the snapshot would dangle once it frees: keys that can
        // hold heap data box (structs and collections, like values), and
        // every other heap-owning key takes an independent copy up front.
        // Immediates own nothing and `str` is already covered, so both
        // stay as-is.
        let stored_key = if Self::any_needs_erase(key) {
            self.box_into_any(Operand::Copy(key_tmp), key, span)
        } else if *key != Type::Str && Self::list_elem_needs_copy(key) {
            let dup = self.new_local(key.clone(), None, false);
            self.push_statement(
                StatementKind::Assign(
                    dup,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(
                            "__olive_copy_typed".to_string(),
                        )),
                        args: vec![Operand::Copy(key_tmp)],
                    },
                ),
                span,
            );
            Operand::Copy(dup)
        } else {
            Operand::Copy(key_tmp)
        };
        let val_tmp = self.new_local_with_owning(value.clone(), None, false, false);
        self.push_statement(
            StatementKind::Assign(
                val_tmp,
                Rvalue::GetIndex(
                    Operand::Copy(pair),
                    Operand::Constant(Constant::Int(1)),
                    false,
                ),
            ),
            span,
        );
        let erased = self.box_into_any(Operand::Copy(val_tmp), value, span);
        self.push_statement(
            StatementKind::SetIndex(Operand::Copy(result), stored_key, erased, false),
            span,
        );
        self.push_statement(
            StatementKind::Assign(
                index,
                Rvalue::BinaryOp(
                    BinOp::Add,
                    Operand::Copy(index),
                    Operand::Constant(Constant::Int(1)),
                ),
            ),
            span,
        );
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: condition },
            span,
        );
        self.current_block = Some(done);
        Operand::Copy(result)
    }

    /// Erases a `T | None` union into `Any`: `None` is the zero sentinel
    /// and passes through untouched, while a live `T` boxes exactly as a
    /// plain `T` (recursing into the collection arms above when nested).
    /// The discriminant reads the word itself, so no tag decoding can
    /// misfire on either arm.
    pub(super) fn erase_nullable(&mut self, source: Operand, inner: &Type, span: Span) -> Operand {
        let out = self.new_local(Type::Any, None, false);
        let box_bb = self.new_block();
        let none_bb = self.new_block();
        let done_bb = self.new_block();
        let discr = match &source {
            Operand::Copy(l) | Operand::Move(l) => Operand::Copy(*l),
            other => other.clone(),
        };
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::SwitchInt {
                    discr,
                    targets: vec![(0, none_bb)],
                    otherwise: box_bb,
                },
                span,
            );
        }
        self.current_block = Some(box_bb);
        let boxed = self.box_into_any(source, inner, span);
        self.push_statement(StatementKind::Assign(out, Rvalue::Use(boxed)), span);
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: done_bb },
            span,
        );
        // The zero sentinel keeps its meaning: `None` in, `None` out.
        self.current_block = Some(none_bb);
        self.push_statement(
            StatementKind::Assign(out, Rvalue::Use(Operand::Constant(Constant::None))),
            span,
        );
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: done_bb },
            span,
        );
        self.current_block = Some(done_bb);
        Operand::Copy(out)
    }

    pub(super) fn erase_list_elements(
        &mut self,
        source: Operand,
        element: &Type,
        span: Span,
    ) -> Operand {
        let length = self.new_unscoped_local(Type::Int);
        self.push_statement(
            StatementKind::Assign(
                length,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_len".into())),
                    args: vec![source.clone()],
                },
            ),
            span,
        );
        let result = self.new_local(Type::List(Box::new(Type::Any)), None, false);
        self.push_statement(
            StatementKind::Assign(
                result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_new".into())),
                    args: vec![Operand::Copy(length)],
                },
            ),
            span,
        );
        let index = self.new_unscoped_local(Type::Int);
        self.push_statement(
            StatementKind::Assign(index, Rvalue::Use(Operand::Constant(Constant::Int(0)))),
            span,
        );
        let condition = self.new_block();
        let body = self.new_block();
        let done = self.new_block();
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: condition },
            span,
        );
        self.current_block = Some(condition);
        let more = self.new_unscoped_local(Type::Bool);
        self.push_statement(
            StatementKind::Assign(
                more,
                Rvalue::BinaryOp(BinOp::Lt, Operand::Copy(index), Operand::Copy(length)),
            ),
            span,
        );
        self.terminate_block(
            condition,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(more),
                targets: vec![(1, body)],
                otherwise: done,
            },
            span,
        );
        self.current_block = Some(body);
        let item = self.new_unscoped_local_with_owning(element.clone(), false);
        self.push_statement(
            StatementKind::Assign(item, Rvalue::GetIndex(source, Operand::Copy(index), true)),
            span,
        );
        let erased = self.box_into_any(Operand::Copy(item), element, span);
        self.push_statement(
            StatementKind::SetIndex(Operand::Copy(result), Operand::Copy(index), erased, true),
            span,
        );
        self.push_statement(
            StatementKind::Assign(
                index,
                Rvalue::BinaryOp(
                    BinOp::Add,
                    Operand::Copy(index),
                    Operand::Constant(Constant::Int(1)),
                ),
            ),
            span,
        );
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: condition },
            span,
        );
        self.current_block = Some(done);
        // The loop above stores Any-boxed elements into a raw `KIND_LIST`
        // container; kind-dispatched consumers would misread the boxed
        // words as raw values, so mark the result before returning it.
        // The mark borrows (a non-owning temp): `result` keeps sole
        // ownership and moves out to the caller.
        let marked = self.new_unscoped_local_with_owning(Type::List(Box::new(Type::Any)), false);
        self.push_statement(
            StatementKind::Assign(
                marked,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_list_mark_any".into())),
                    args: vec![Operand::Copy(result)],
                },
            ),
            span,
        );
        let _ = marked;
        Operand::Copy(result)
    }
}
