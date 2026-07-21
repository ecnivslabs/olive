use super::MirBuilder;
use crate::mir::AggregateKind;
use crate::mir::ir::*;
use crate::parser::{CompClause, ForTarget, MatchPattern};
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    pub(super) fn lower_pattern(
        &mut self,
        pattern: &MatchPattern,
        discr: Local,
        match_ty: &Type,
        success_bb: BasicBlockId,
        failure_bb: BasicBlockId,
        expr_span: Span,
    ) {
        match pattern {
            MatchPattern::Wildcard => {
                self.terminate_block(
                    self.current_block.unwrap(),
                    TerminatorKind::Goto { target: success_bb },
                    expr_span,
                );
            }
            MatchPattern::Identifier(name, _) => {
                // A catch-all/type-narrowing binding aliases the scrutinee (or an
                // already-non-owning payload extracted from it) rather than owning
                // a separate value; the scrutinee's own drop releases it.
                let discr_ty = self.current_locals[discr.0].ty.clone();
                let binding_local = self.declare_var_view(name.clone(), match_ty.clone(), true);
                // A binding narrowed out of a tag-encoded union decodes the
                // payload; the raw word is the encoding, not the value.
                let value =
                    if discr_ty.is_tag_encoded_union() && !matches!(match_ty, Type::Union(_)) {
                        self.unbox_from_any(Operand::Copy(discr), match_ty, expr_span)
                            .unwrap_or(Operand::Copy(discr))
                    } else {
                        Operand::Copy(discr)
                    };
                self.push_statement(
                    StatementKind::Assign(binding_local, Rvalue::Use(value)),
                    expr_span,
                );
                self.terminate_block(
                    self.current_block.unwrap(),
                    TerminatorKind::Goto { target: success_bb },
                    expr_span,
                );
            }
            MatchPattern::Variant(v_name, inner_patterns) => {
                let resolved = match match_ty {
                    Type::Enum(enum_name, _) => {
                        let mangled = format!("{}::{}", enum_name, v_name);
                        self.enum_variants.get(&mangled).map(|(_, tag)| {
                            (
                                enum_name.clone(),
                                crate::mir::enum_type_id(enum_name),
                                *tag as i64,
                            )
                        })
                    }
                    Type::Union(members) => members.iter().find_map(|ty| {
                        if let Type::Enum(en, _) = ty {
                            let mangled = format!("{}::{}", en, v_name);
                            self.enum_variants.get(&mangled).map(|(_, tag)| {
                                (en.clone(), crate::mir::enum_type_id(en), *tag as i64)
                            })
                        } else {
                            None
                        }
                    }),
                    _ => None,
                };

                let (enum_name, type_id, tag_id) =
                    resolved.unwrap_or_else(|| (String::new(), 0, 0));

                let tag_check_start_bb = if matches!(match_ty, Type::Union(_)) {
                    let type_id_tmp = self.new_local(Type::Int, None, false);
                    self.push_statement(
                        StatementKind::Assign(type_id_tmp, Rvalue::GetTypeId(Operand::Copy(discr))),
                        expr_span,
                    );
                    let type_match_bb = self.new_block();
                    self.terminate_block(
                        self.current_block.unwrap(),
                        TerminatorKind::SwitchInt {
                            discr: Operand::Copy(type_id_tmp),
                            targets: vec![(type_id, type_match_bb)],
                            otherwise: failure_bb,
                        },
                        expr_span,
                    );
                    self.current_block = Some(type_match_bb);
                    type_match_bb
                } else {
                    self.current_block.unwrap()
                };

                let tag_tmp = self.new_local(Type::Int, None, false);
                self.push_statement(
                    StatementKind::Assign(tag_tmp, Rvalue::GetTag(Operand::Copy(discr))),
                    expr_span,
                );

                let variant_match_bb = self.new_block();
                self.terminate_block(
                    self.current_block.unwrap_or(tag_check_start_bb),
                    TerminatorKind::SwitchInt {
                        discr: Operand::Copy(tag_tmp),
                        targets: vec![(tag_id, variant_match_bb)],
                        otherwise: failure_bb,
                    },
                    expr_span,
                );

                self.current_block = Some(variant_match_bb);

                if inner_patterns.is_empty() {
                    self.terminate_block(
                        variant_match_bb,
                        TerminatorKind::Goto { target: success_bb },
                        expr_span,
                    );
                } else {
                    let mangled = format!("{}::{}", enum_name, v_name);
                    let param_types = self
                        .global_types
                        .get(&mangled)
                        .and_then(|ty| {
                            if let Type::Fn(pts, _, _) = ty {
                                Some(pts.clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| vec![Type::Any; inner_patterns.len()]);

                    let mut current_bb = variant_match_bb;
                    for (i, (p, p_ty)) in inner_patterns.iter().zip(param_types.iter()).enumerate()
                    {
                        self.current_block = Some(current_bb);
                        let val_tmp =
                            self.new_local_with_owning(p_ty.clone() as Type, None, false, false);
                        self.push_statement(
                            StatementKind::Assign(
                                val_tmp,
                                Rvalue::GetIndex(
                                    Operand::Copy(discr),
                                    Operand::Constant(Constant::Int(i as i64)),
                                    false,
                                ),
                            ),
                            expr_span,
                        );

                        let next_bb = if i == inner_patterns.len() - 1 {
                            success_bb
                        } else {
                            self.new_block()
                        };

                        self.lower_pattern(p, val_tmp, p_ty, next_bb, failure_bb, expr_span);
                        current_bb = next_bb;
                    }
                }
            }
            MatchPattern::Literal(lit_expr) => {
                let is_eq = self.new_local(Type::Bool, None, false);
                if matches!(lit_expr.kind, crate::parser::ExprKind::Null) {
                    // A `None` arm matches a boxed null in an `Any` via the
                    // runtime check; a statically-null scrutinee always matches.
                    let rvalue = match match_ty {
                        Type::Any => Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_any_is_null".to_string(),
                            )),
                            args: vec![Operand::Copy(discr)],
                        },
                        Type::Null => Rvalue::Use(Operand::Constant(Constant::Bool(true))),
                        // Boxed encodings (Any member or tagged scalar union)
                        // hold null as a tag word; pointer unions use bare 0.
                        Type::Union(members)
                            if members.contains(&Type::Any) || match_ty.is_tag_encoded_union() =>
                        {
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_any_is_null".to_string(),
                                )),
                                args: vec![Operand::Copy(discr)],
                            }
                        }
                        Type::Union(_) => Rvalue::BinaryOp(
                            crate::parser::BinOp::Eq,
                            Operand::Copy(discr),
                            Operand::Constant(Constant::Int(0)),
                        ),
                        _ => Rvalue::Use(Operand::Constant(Constant::Bool(false))),
                    };
                    self.push_statement(StatementKind::Assign(is_eq, rvalue), expr_span);
                } else if match_ty.is_tag_encoded_union() {
                    // Kind-respecting compare: an int word must never raw-match
                    // a str literal, and null must not unbox to a matching 0.
                    let lit_ty = self.get_type(lit_expr.id);
                    let lit_op = self.lower_expr(lit_expr);
                    let lit_box = self.box_into_any(lit_op, &lit_ty, expr_span);
                    self.push_statement(
                        StatementKind::Assign(
                            is_eq,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_any_eq_strict".to_string(),
                                )),
                                args: vec![Operand::Copy(discr), lit_box],
                            },
                        ),
                        expr_span,
                    );
                } else {
                    let lit_op = self.lower_expr(lit_expr);
                    self.push_statement(
                        StatementKind::Assign(
                            is_eq,
                            Rvalue::BinaryOp(
                                crate::parser::BinOp::Eq,
                                Operand::Copy(discr),
                                lit_op,
                            ),
                        ),
                        expr_span,
                    );
                }
                self.terminate_block(
                    self.current_block.unwrap(),
                    TerminatorKind::SwitchInt {
                        discr: Operand::Copy(is_eq),
                        targets: vec![(1, success_bb)],
                        otherwise: failure_bb,
                    },
                    expr_span,
                );
            }
            MatchPattern::Tuple(items) => {
                let field_tys = match match_ty {
                    Type::Tuple(tys) if tys.len() == items.len() => tys.clone(),
                    _ => vec![Type::Any; items.len()],
                };
                self.lower_positional_fields(
                    items, &field_tys, discr, success_bb, failure_bb, expr_span,
                );
            }
            MatchPattern::StructFields(struct_name, fields, _) => {
                let mut current_bb = self.current_block.unwrap();
                if fields.is_empty() {
                    self.terminate_block(
                        current_bb,
                        TerminatorKind::Goto { target: success_bb },
                        expr_span,
                    );
                }
                for (i, (fname, fpat)) in fields.iter().enumerate() {
                    self.current_block = Some(current_bb);
                    let field_ty = self
                        .struct_field_types
                        .get(&(struct_name.clone(), fname.clone()))
                        .cloned()
                        .unwrap_or(Type::Any);
                    let val_tmp = self.new_local_with_owning(field_ty.clone(), None, false, false);
                    self.push_statement(
                        StatementKind::Assign(
                            val_tmp,
                            Rvalue::GetAttr(Operand::Copy(discr), fname.clone()),
                        ),
                        expr_span,
                    );
                    let next_bb = if i == fields.len() - 1 {
                        success_bb
                    } else {
                        self.new_block()
                    };
                    self.lower_pattern(fpat, val_tmp, &field_ty, next_bb, failure_bb, expr_span);
                    current_bb = next_bb;
                }
            }
            MatchPattern::List {
                before,
                rest,
                after,
            } => {
                let elem_ty = match match_ty {
                    Type::List(t) => (**t).clone(),
                    _ => Type::Any,
                };
                let needed = (before.len() + after.len()) as i64;

                let len_tmp = self.new_local(Type::Int, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        len_tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_list_len".to_string(),
                            )),
                            args: vec![Operand::Copy(discr)],
                        },
                    ),
                    expr_span,
                );
                let len_ok = self.new_local(Type::Bool, None, false);
                let cmp_op = if rest.is_some() {
                    crate::parser::BinOp::GtEq
                } else {
                    crate::parser::BinOp::Eq
                };
                self.push_statement(
                    StatementKind::Assign(
                        len_ok,
                        Rvalue::BinaryOp(
                            cmp_op,
                            Operand::Copy(len_tmp),
                            Operand::Constant(Constant::Int(needed)),
                        ),
                    ),
                    expr_span,
                );
                let len_ok_bb = self.new_block();
                self.terminate_block(
                    self.current_block.unwrap(),
                    TerminatorKind::SwitchInt {
                        discr: Operand::Copy(len_ok),
                        targets: vec![(1, len_ok_bb)],
                        otherwise: failure_bb,
                    },
                    expr_span,
                );
                self.current_block = Some(len_ok_bb);

                // `before` reads fixed positions from the front; `after` reads
                // fixed positions counted back from the (now proven-long-enough)
                // runtime length; `rest`, if present, is the one real
                // allocation here -- an independent deep-copied slice, same
                // semantics as E4.4's starred destructuring.
                let after_start_bb = self.new_block();
                self.lower_positional_fields(
                    before,
                    &vec![elem_ty.clone(); before.len()],
                    discr,
                    after_start_bb,
                    failure_bb,
                    expr_span,
                );

                self.current_block = Some(after_start_bb);
                let mut current_bb = after_start_bb;
                let rest_bb = self.new_block();
                if after.is_empty() {
                    self.terminate_block(
                        current_bb,
                        TerminatorKind::Goto { target: rest_bb },
                        expr_span,
                    );
                }
                for (j, apat) in after.iter().enumerate() {
                    self.current_block = Some(current_bb);
                    let back_offset = (after.len() - j) as i64;
                    let idx_tmp = self.new_local(Type::Int, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            idx_tmp,
                            Rvalue::BinaryOp(
                                crate::parser::BinOp::Sub,
                                Operand::Copy(len_tmp),
                                Operand::Constant(Constant::Int(back_offset)),
                            ),
                        ),
                        expr_span,
                    );
                    let val_tmp = self.new_local_with_owning(elem_ty.clone(), None, false, false);
                    self.push_statement(
                        StatementKind::Assign(
                            val_tmp,
                            Rvalue::GetIndex(Operand::Copy(discr), Operand::Copy(idx_tmp), false),
                        ),
                        expr_span,
                    );
                    let next_bb = if j == after.len() - 1 {
                        rest_bb
                    } else {
                        self.new_block()
                    };
                    self.lower_pattern(apat, val_tmp, &elem_ty, next_bb, failure_bb, expr_span);
                    current_bb = next_bb;
                }

                self.current_block = Some(rest_bb);
                if let Some((name, _)) = rest {
                    let stop_tmp = self.new_local(Type::Int, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            stop_tmp,
                            Rvalue::BinaryOp(
                                crate::parser::BinOp::Sub,
                                Operand::Copy(len_tmp),
                                Operand::Constant(Constant::Int(after.len() as i64)),
                            ),
                        ),
                        expr_span,
                    );
                    let func_name = if Self::list_elem_needs_copy(&elem_ty) {
                        "__olive_list_getslice_typed"
                    } else {
                        "__olive_list_getslice"
                    };
                    const SLICE_HAS_START: i64 = 1;
                    const SLICE_HAS_STOP: i64 = 2;
                    let rest_local =
                        self.declare_var(name.clone(), Type::List(Box::new(elem_ty)), false);
                    self.push_statement(
                        StatementKind::Assign(
                            rest_local,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(func_name.to_string())),
                                args: vec![
                                    Operand::Copy(discr),
                                    Operand::Constant(Constant::Int(before.len() as i64)),
                                    Operand::Copy(stop_tmp),
                                    Operand::Constant(Constant::Int(0)),
                                    Operand::Constant(Constant::Int(
                                        SLICE_HAS_START | SLICE_HAS_STOP,
                                    )),
                                ],
                            },
                        ),
                        expr_span,
                    );
                }
                self.terminate_block(
                    self.current_block.unwrap(),
                    TerminatorKind::Goto { target: success_bb },
                    expr_span,
                );
            }
            MatchPattern::Range(start, end, inclusive) => {
                let start_op = self.lower_expr(start);
                let end_op = self.lower_expr(end);
                let ge_lo = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        ge_lo,
                        Rvalue::BinaryOp(
                            crate::parser::BinOp::GtEq,
                            Operand::Copy(discr),
                            start_op,
                        ),
                    ),
                    expr_span,
                );
                let hi_check_bb = self.new_block();
                self.terminate_block(
                    self.current_block.unwrap(),
                    TerminatorKind::SwitchInt {
                        discr: Operand::Copy(ge_lo),
                        targets: vec![(1, hi_check_bb)],
                        otherwise: failure_bb,
                    },
                    expr_span,
                );
                self.current_block = Some(hi_check_bb);
                let cmp_op = if *inclusive {
                    crate::parser::BinOp::LtEq
                } else {
                    crate::parser::BinOp::Lt
                };
                let le_hi = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        le_hi,
                        Rvalue::BinaryOp(cmp_op, Operand::Copy(discr), end_op),
                    ),
                    expr_span,
                );
                self.terminate_block(
                    hi_check_bb,
                    TerminatorKind::SwitchInt {
                        discr: Operand::Copy(le_hi),
                        targets: vec![(1, success_bb)],
                        otherwise: failure_bb,
                    },
                    expr_span,
                );
            }
            MatchPattern::Or(alts) => {
                // Every alternative tests the same `discr`/`match_ty`; a
                // failed one falls through to try the next, and only the
                // last alternative's failure reaches the real
                // `failure_bb`. The checker already proved every
                // alternative binds the same names at the same types
                // (E0436), but each alternative's own recursive lowering
                // still declares its *own* fresh local per binding -- left
                // alone, the arm body would resolve a name to whichever
                // alternative's local was declared last, uninitialized on
                // every other path. One shared local per name, written by
                // every alternative on its own success path before all of
                // them converge on `success_bb`, is what makes "the arm
                // body is never duplicated" actually sound.
                let mut names = Vec::new();
                collect_binding_names(&alts[0], &mut names);
                let shared_locals: Vec<(String, Local)> = names
                    .into_iter()
                    .filter_map(|name| {
                        let ty = self.pattern_binding_type(&alts[0], match_ty, &name)?;
                        let local = self.new_local_with_owning(ty, Some(name.clone()), true, false);
                        Some((name, local))
                    })
                    .collect();

                let mut current_bb = self.current_block.unwrap();
                for (i, alt) in alts.iter().enumerate() {
                    self.current_block = Some(current_bb);
                    let alt_matched_bb = self.new_block();
                    let next_try_bb = if i == alts.len() - 1 {
                        failure_bb
                    } else {
                        self.new_block()
                    };
                    self.lower_pattern(
                        alt,
                        discr,
                        match_ty,
                        alt_matched_bb,
                        next_try_bb,
                        expr_span,
                    );

                    self.current_block = Some(alt_matched_bb);
                    for (name, shared_local) in &shared_locals {
                        if let Some(alt_local) = self.lookup_var(name) {
                            self.push_statement(
                                StatementKind::Assign(
                                    *shared_local,
                                    Rvalue::Use(Operand::Copy(alt_local)),
                                ),
                                expr_span,
                            );
                        }
                    }
                    self.terminate_block(
                        self.current_block.unwrap(),
                        TerminatorKind::Goto { target: success_bb },
                        expr_span,
                    );
                    current_bb = next_try_bb;
                }
                // Every alternative's copy targets the same shared local,
                // so from here on `name` must resolve to it regardless of
                // which alternative actually matched at runtime.
                for (name, local) in shared_locals {
                    self.var_map.last_mut().unwrap().insert(name, local);
                }
            }
        }
    }

    /// Shared chain for "read fixed position `i` via `GetIndex`, recursively
    /// match it, move on to `i+1`" -- tuple elements and a list pattern's
    /// fixed `before` slots are this same shape (a variant's own positional
    /// payload loop stays separate above: it also needs the enum tag switch
    /// interleaved, not just the index reads).
    fn lower_positional_fields(
        &mut self,
        items: &[MatchPattern],
        item_tys: &[Type],
        discr: Local,
        success_bb: BasicBlockId,
        failure_bb: BasicBlockId,
        expr_span: Span,
    ) {
        let mut current_bb = self.current_block.unwrap();
        if items.is_empty() {
            self.terminate_block(
                current_bb,
                TerminatorKind::Goto { target: success_bb },
                expr_span,
            );
            return;
        }
        for (i, (item, item_ty)) in items.iter().zip(item_tys).enumerate() {
            self.current_block = Some(current_bb);
            let val_tmp = self.new_local_with_owning(item_ty.clone(), None, false, false);
            self.push_statement(
                StatementKind::Assign(
                    val_tmp,
                    Rvalue::GetIndex(
                        Operand::Copy(discr),
                        Operand::Constant(Constant::Int(i as i64)),
                        false,
                    ),
                ),
                expr_span,
            );
            let next_bb = if i == items.len() - 1 {
                success_bb
            } else {
                self.new_block()
            };
            self.lower_pattern(item, val_tmp, item_ty, next_bb, failure_bb, expr_span);
            current_bb = next_bb;
        }
    }

    /// The type a name would bind to inside `pattern`, given `pattern`
    /// itself is matched against `ty` -- a read-only walk mirroring
    /// `lower_pattern`'s own structural recursion (enum payload types via
    /// `global_types`, tuple fields positionally, struct fields by name,
    /// list elements), used only to pre-size an or-pattern's shared
    /// binding locals before any alternative actually runs.
    fn pattern_binding_type(&self, pattern: &MatchPattern, ty: &Type, name: &str) -> Option<Type> {
        match pattern {
            MatchPattern::Identifier(n, _) if n == name => Some(ty.clone()),
            MatchPattern::Variant(v_name, inner) => {
                let enum_name = match ty {
                    Type::Enum(en, _) => en.clone(),
                    Type::Union(members) => members.iter().find_map(|m| match m {
                        Type::Enum(en, _) => Some(en.clone()),
                        _ => None,
                    })?,
                    _ => return None,
                };
                let mangled = format!("{enum_name}::{v_name}");
                let param_types = match self.global_types.get(&mangled) {
                    Some(Type::Fn(pts, _, _)) => pts.clone(),
                    _ => return None,
                };
                inner
                    .iter()
                    .zip(param_types.iter())
                    .find_map(|(p, p_ty)| self.pattern_binding_type(p, p_ty, name))
            }
            MatchPattern::Tuple(items) => {
                let Type::Tuple(field_tys) = ty else {
                    return None;
                };
                items
                    .iter()
                    .zip(field_tys.iter())
                    .find_map(|(item, item_ty)| self.pattern_binding_type(item, item_ty, name))
            }
            MatchPattern::StructFields(struct_name, fields, _) => {
                fields.iter().find_map(|(fname, fpat)| {
                    let fty = self
                        .struct_field_types
                        .get(&(struct_name.clone(), fname.clone()))?;
                    self.pattern_binding_type(fpat, fty, name)
                })
            }
            MatchPattern::List {
                before,
                rest,
                after,
            } => {
                let Type::List(elem_ty) = ty else {
                    return None;
                };
                if let Some(t) = before
                    .iter()
                    .chain(after)
                    .find_map(|p| self.pattern_binding_type(p, elem_ty, name))
                {
                    return Some(t);
                }
                match rest {
                    Some((n, _)) if n == name => Some(Type::List(elem_ty.clone())),
                    _ => None,
                }
            }
            MatchPattern::Or(alts) => alts
                .iter()
                .find_map(|alt| self.pattern_binding_type(alt, ty, name)),
            _ => None,
        }
    }

    pub(super) fn bind_for_target(
        &mut self,
        target: &ForTarget,
        val: Local,
        elem_ty: &Type,
        span: Span,
    ) {
        match target {
            ForTarget::Name(name, _) => {
                // Binds an alias of `val`, which the comprehension's own
                // iteration machinery owns and drops; not a separate value.
                let local = self.declare_var_view(name.clone(), elem_ty.clone(), true);
                self.push_statement(
                    StatementKind::Assign(local, Rvalue::Use(Operand::Copy(val))),
                    span,
                );
            }
            ForTarget::Tuple(names) => {
                let comp_tys: Vec<Type> = match elem_ty {
                    Type::Tuple(comps) => comps.clone(),
                    _ => Vec::new(),
                };
                for (i, (name, _)) in names.iter().enumerate() {
                    let bind_ty = comp_tys.get(i).cloned().unwrap_or(Type::Any);
                    let local = self.declare_var_view(name.clone(), bind_ty, true);
                    self.push_statement(
                        StatementKind::Assign(
                            local,
                            Rvalue::GetIndex(
                                Operand::Copy(val),
                                Operand::Constant(Constant::Int(i as i64)),
                                false,
                            ),
                        ),
                        span,
                    );
                }
            }
        }
    }

    pub(super) fn lower_comprehension(
        &mut self,
        elt: Option<(&crate::parser::Expr, &crate::parser::Expr)>,
        single_elt: Option<&crate::parser::Expr>,
        clauses: &[CompClause],
        aggregate_kind: AggregateKind,
        span: Span,
        result_ty: Type,
    ) -> Operand {
        let result_local = self.new_local(result_ty, None, true);
        self.push_statement(StatementKind::StorageLive(result_local), span);
        self.push_statement(
            StatementKind::Assign(
                result_local,
                Rvalue::Aggregate(aggregate_kind.clone(), vec![]),
            ),
            span,
        );

        self.lower_comp_clause(
            elt,
            single_elt,
            clauses,
            0,
            result_local,
            aggregate_kind,
            span,
        );

        Operand::Copy(result_local)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_comp_clause(
        &mut self,
        elt: Option<(&crate::parser::Expr, &crate::parser::Expr)>,
        single_elt: Option<&crate::parser::Expr>,
        clauses: &[CompClause],
        clause_idx: usize,
        result_local: Local,
        aggregate_kind: AggregateKind,
        span: Span,
    ) {
        if clause_idx == clauses.len() {
            if let Some((k_expr, v_expr)) = elt {
                let (key_box_ty, val_box_ty) = match &self.current_locals[result_local.0].ty {
                    Type::Dict(k, v) => (*k.clone(), *v.clone()),
                    _ => (Type::Any, Type::Any),
                };
                let k_op = self.lower_expr(k_expr);
                let k = self.coerce_to_hashable(k_op, k_expr, &key_box_ty);
                let v_op = self.lower_expr(v_expr);
                let v = self.coerce_to_elem(v_op, v_expr, &val_box_ty);
                let set_id = Operand::Constant(Constant::Function("__olive_obj_set".to_string()));
                let tmp = self.new_local(Type::Any, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: set_id,
                            args: vec![Operand::Copy(result_local), k, v],
                        },
                    ),
                    span,
                );
            } else if let Some(e_expr) = single_elt {
                let val = self.lower_expr(e_expr);
                let func_name = match aggregate_kind {
                    AggregateKind::Set => "__olive_set_add",
                    _ => "__olive_list_append",
                };
                let tmp = self.new_local(Type::Any, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(func_name.to_string())),
                            args: vec![Operand::Copy(result_local), val],
                        },
                    ),
                    span,
                );
            }
            return;
        }

        let clause = &clauses[clause_idx];
        // Type the yielded value by the iterable's element type so concrete
        // values are read raw rather than as boxed `Any`, matching `for` loops.
        let mut iter_ty = self.get_type(clause.iter.id);
        while let Type::Ref(inner) | Type::MutRef(inner) = iter_ty {
            iter_ty = *inner;
        }
        let elem_ty = match iter_ty {
            Type::Str => Type::Str,
            Type::List(t) | Type::Set(t) => *t,
            Type::Dict(k, _) => *k,
            _ => Type::Any,
        };
        let (iter_copy, _) = self.borrow_iterable(&clause.iter);
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let next_clause_bb = self.new_block();
        let exit_bb = self.new_block();

        let iter_local = self.new_local(Type::Any, None, true);
        self.push_statement(
            StatementKind::Assign(
                iter_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("iter".to_string())),
                    args: vec![Operand::Copy(iter_copy)],
                },
            ),
            span,
        );

        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: cond_bb },
            span,
        );

        self.current_block = Some(cond_bb);
        let has_next = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(
                has_next,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("has_next".to_string())),
                    args: vec![Operand::Copy(iter_local)],
                },
            ),
            span,
        );
        self.terminate_block(
            cond_bb,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(has_next),
                targets: vec![(1, body_bb)],
                otherwise: exit_bb,
            },
            span,
        );

        self.current_block = Some(body_bb);
        let next_val = self.new_local(elem_ty.clone(), None, true);
        self.push_statement(
            StatementKind::Assign(
                next_val,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("next".to_string())),
                    args: vec![Operand::Copy(iter_local)],
                },
            ),
            span,
        );

        self.bind_for_target(&clause.target, next_val, &elem_ty, span);

        if let Some(cond_expr) = &clause.condition {
            let cond_val = self.lower_expr(cond_expr);
            self.terminate_block(
                self.current_block.unwrap(),
                TerminatorKind::SwitchInt {
                    discr: cond_val,
                    targets: vec![(1, next_clause_bb)],
                    otherwise: cond_bb,
                },
                span,
            );
        } else {
            self.terminate_block(
                self.current_block.unwrap(),
                TerminatorKind::Goto {
                    target: next_clause_bb,
                },
                span,
            );
        }

        self.current_block = Some(next_clause_bb);
        self.lower_comp_clause(
            elt,
            single_elt,
            clauses,
            clause_idx + 1,
            result_local,
            aggregate_kind,
            span,
        );
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: cond_bb },
            span,
        );

        self.current_block = Some(exit_bb);
    }
}

/// Every name a pattern binds -- the MIR-side counterpart of the type
/// checker's own `pattern_binding_names` (kept separate since neither side
/// depends on the other's internals).
fn collect_binding_names(pattern: &MatchPattern, out: &mut Vec<String>) {
    match pattern {
        MatchPattern::Identifier(name, _) => out.push(name.clone()),
        MatchPattern::Variant(_, inner) => {
            for p in inner {
                collect_binding_names(p, out);
            }
        }
        MatchPattern::Tuple(items) | MatchPattern::Or(items) => {
            for p in items {
                collect_binding_names(p, out);
            }
        }
        MatchPattern::StructFields(_, fields, _) => {
            for (_, p) in fields {
                collect_binding_names(p, out);
            }
        }
        MatchPattern::List {
            before,
            rest,
            after,
        } => {
            for p in before.iter().chain(after) {
                collect_binding_names(p, out);
            }
            if let Some((name, _)) = rest {
                out.push(name.clone());
            }
        }
        MatchPattern::Wildcard | MatchPattern::Literal(_) | MatchPattern::Range(..) => {}
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
            tc.enum_defs.clone(),
        );
        builder.build_program(&prog);
        builder.functions
    }

    #[test]
    fn match_wildcard_produces_goto() {
        let fns =
            build("fn f(x: i64) -> i64:\n    match x:\n        case _:\n            return 1\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_goto = f.basic_blocks.iter().any(|bb| {
            bb.terminator
                .as_ref()
                .is_some_and(|t| matches!(t.kind, TerminatorKind::Goto { .. }))
        });
        assert!(has_goto);
    }

    #[test]
    fn match_identifier_binds_variable() {
        let fns =
            build("fn f(x: i64) -> i64:\n    match x:\n        case y:\n            return y\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_assign = f.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(s.kind, StatementKind::Assign(_, _)))
        });
        assert!(has_assign);
    }

    #[test]
    fn match_literal_uses_eq() {
        let fns = build(
            "fn f(x: i64) -> i64:\n    match x:\n        case 42:\n            return 1\n        case _:\n            return 0\n",
        );
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_switch = f.basic_blocks.iter().any(|bb| {
            bb.terminator
                .as_ref()
                .is_some_and(|t| matches!(t.kind, TerminatorKind::SwitchInt { .. }))
        });
        assert!(has_switch);
    }

    #[test]
    fn enum_match_produces_switch() {
        let fns = build(
            "enum Color:\n    Red\n    Green\n    Blue\n\nfn f(c: Color) -> i64:\n    match c:\n        case Red:\n            return 0\n        case _:\n            return 1\n",
        );
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_switch = f.basic_blocks.iter().any(|bb| {
            bb.terminator
                .as_ref()
                .is_some_and(|t| matches!(t.kind, TerminatorKind::SwitchInt { .. }))
        });
        assert!(has_switch);
    }

    #[test]
    fn list_comprehension_produces_loop_structure() {
        let fns = build("fn f() -> [i64]:\n    return [x for x in [1, 2, 3]]\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert!(
            f.basic_blocks.len() >= 2,
            "comprehension should create multiple blocks"
        );
    }

    /// E12.3 acceptance: a match on int ranges lowers to a compare chain
    /// (`SwitchInt` terminators from the `>=`/`<`-family `BinaryOp`s), no
    /// allocation, no runtime helper call.
    #[test]
    fn range_match_is_a_compare_chain() {
        let fns = build(
            "fn f(n: i64) -> i64:\n    match n:\n        case 0..10:\n            return 1\n        case 10..=20:\n            return 2\n        case _:\n            return 0\n",
        );
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let switch_count = f
            .basic_blocks
            .iter()
            .filter(|bb| {
                bb.terminator
                    .as_ref()
                    .is_some_and(|t| matches!(t.kind, TerminatorKind::SwitchInt { .. }))
            })
            .count();
        // Two ranges, two comparisons apiece (>= lo, </<= hi): at least 4
        // SwitchInt terminators, none of them a call.
        assert!(
            switch_count >= 4,
            "expected a compare chain, got {switch_count} switches"
        );
        let has_call = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, crate::mir::Rvalue::Call { .. })
                )
            })
        });
        assert!(
            !has_call,
            "a range pattern should never need a runtime call"
        );
    }

    #[test]
    fn tuple_pattern_reads_by_index() {
        let fns = build(
            "fn f(p: (i64, i64)) -> i64:\n    match p:\n        case (0, 0):\n            return 1\n        case (a, b):\n            return a\n",
        );
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_get_index = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, crate::mir::Rvalue::GetIndex(..))
                )
            })
        });
        assert!(has_get_index);
    }

    #[test]
    fn struct_pattern_reads_by_attr() {
        let fns = build(
            "struct P:\n    x: i64\n    y: i64\n\nfn f(p: P) -> i64:\n    match p:\n        case P(x=0, y=n):\n            return n\n        case P(x=n, y=m):\n            return n\n",
        );
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_get_attr = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, crate::mir::Rvalue::GetAttr(..))
                )
            })
        });
        assert!(has_get_attr);
    }

    #[test]
    fn list_pattern_checks_length_first() {
        let fns = build(
            "fn f(xs: [i64]) -> i64:\n    match xs:\n        case []:\n            return 0\n        case [first, *rest]:\n            return first\n",
        );
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_len_call = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(
                        _,
                        crate::mir::Rvalue::Call {
                            func: crate::mir::Operand::Constant(crate::mir::Constant::Function(name)),
                            ..
                        }
                    ) if name == "__olive_list_len"
                )
            })
        });
        assert!(has_len_call);
    }

    #[test]
    fn or_pattern_both_alternatives_reach_success() {
        let fns = build(
            "enum R:\n    A(i64)\n    B(i64)\n\nfn f(r: R) -> i64:\n    match r:\n        case A(v) | B(v):\n            return v\n",
        );
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        // Two `GetTag` reads (one per alternative's own enum-tag test),
        // proving both A and B are actually tried, not just one.
        let tag_reads = f
            .basic_blocks
            .iter()
            .flat_map(|bb| &bb.statements)
            .filter(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(_, crate::mir::Rvalue::GetTag(_))
                )
            })
            .count();
        assert_eq!(tag_reads, 2);
    }

    #[test]
    fn guarded_arm_does_not_bypass_later_arms() {
        // A guard can fail even when the pattern matches, so a guarded
        // literal arm must not short-circuit the exhaustiveness-relevant
        // structure: this just proves the guard's own SwitchInt exists
        // alongside the pattern's.
        let fns = build(
            "fn f(n: i64) -> i64:\n    match n:\n        case x if x > 0:\n            return 1\n        case _:\n            return 0\n",
        );
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let switch_count = f
            .basic_blocks
            .iter()
            .filter(|bb| {
                bb.terminator
                    .as_ref()
                    .is_some_and(|t| matches!(t.kind, TerminatorKind::SwitchInt { .. }))
            })
            .count();
        assert!(switch_count >= 1);
    }
}
