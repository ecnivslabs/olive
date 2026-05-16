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
            MatchPattern::Identifier(name) => {
                let binding_local = self.declare_var(name.clone(), match_ty.clone(), true);
                self.push_statement(
                    StatementKind::Assign(binding_local, Rvalue::Use(Operand::Copy(discr))),
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
                                crate::mir::MirBuilder::enum_type_id(enum_name),
                                *tag as i64,
                            )
                        })
                    }
                    Type::Union(members) => members.iter().find_map(|ty| {
                        if let Type::Enum(en, _) = ty {
                            let mangled = format!("{}::{}", en, v_name);
                            self.enum_variants.get(&mangled).map(|(_, tag)| {
                                (
                                    en.clone(),
                                    crate::mir::MirBuilder::enum_type_id(en),
                                    *tag as i64,
                                )
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
                        let val_tmp = self.new_local(p_ty.clone() as Type, None, false);
                        self.push_statement(
                            StatementKind::Assign(
                                val_tmp,
                                Rvalue::GetIndex(
                                    Operand::Copy(discr),
                                    Operand::Constant(Constant::Int(i as i64)),
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
                let lit_op = self.lower_expr(lit_expr);
                let is_eq = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        is_eq,
                        Rvalue::BinaryOp(crate::parser::BinOp::Eq, Operand::Copy(discr), lit_op),
                    ),
                    expr_span,
                );
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
        }
    }

    pub(super) fn bind_for_target(&mut self, target: &ForTarget, val: Local, span: Span) {
        match target {
            ForTarget::Name(name, _) => {
                let local = self.declare_var(name.clone(), Type::Any, true);
                self.push_statement(
                    StatementKind::Assign(local, Rvalue::Use(Operand::Copy(val))),
                    span,
                );
            }
            ForTarget::Tuple(names) => {
                for (i, (name, _)) in names.iter().enumerate() {
                    let local = self.declare_var(name.clone(), Type::Any, true);
                    self.push_statement(
                        StatementKind::Assign(
                            local,
                            Rvalue::GetIndex(
                                Operand::Copy(val),
                                Operand::Constant(Constant::Int(i as i64)),
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

        Operand::Move(result_local)
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
                let k = self.lower_expr(k_expr);
                let v = self.lower_expr(v_expr);
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
        let iter_op = self.lower_expr(&clause.iter);
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
                    args: vec![iter_op],
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
        let next_val = self.new_local(Type::Any, None, true);
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

        self.bind_for_target(&clause.target, next_val, span);

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
