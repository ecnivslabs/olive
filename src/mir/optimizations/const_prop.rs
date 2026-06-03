use super::Transform;
use crate::mir::*;
use crate::semantic::types::Type;
use rustc_hash::FxHashMap as HashMap;

pub struct ConstantPropagation;

impl Transform for ConstantPropagation {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut assign_counts: HashMap<Local, usize> = HashMap::default();
        let mut constant_assignments: HashMap<Local, Constant> = HashMap::default();

        let loops = crate::mir::loop_utils::find_loops(func);
        let mut loop_blocks = rustc_hash::FxHashSet::default();
        for lp in &loops {
            loop_blocks.insert(lp.header);
            for &bb in &lp.body {
                loop_blocks.insert(bb);
            }
        }

        for (bb_idx, bb) in func.basic_blocks.iter().enumerate() {
            let in_loop = loop_blocks.contains(&BasicBlockId(bb_idx));
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    *assign_counts.entry(*dest).or_insert(0) += if in_loop { 2 } else { 1 };
                    // A `T | None` union has no boxed form -- its declared type and its
                    // narrowed payload's type diverge (e.g. `float | None` vs `float`),
                    // so a constant of the payload's type isn't interchangeable with the
                    // union's own raw-word representation (the `== None` sentinel check
                    // needs the latter). Propagating one for the other miscompiles.
                    if let Rvalue::Use(Operand::Constant(c)) = rval
                        && !matches!(func.locals[dest.0].ty, Type::Union(_))
                    {
                        constant_assignments.insert(*dest, c.clone());
                    }
                } else if let StatementKind::SetAttr(obj, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        *assign_counts.entry(*l).or_insert(0) += if in_loop { 2 } else { 1 };
                    }
                } else if let StatementKind::SetIndex(obj, _, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        *assign_counts.entry(*l).or_insert(0) += if in_loop { 2 } else { 1 };
                    }
                } else if let StatementKind::VectorStore(obj, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        *assign_counts.entry(*l).or_insert(0) += if in_loop { 2 } else { 1 };
                    }
                } else if let StatementKind::PtrStore(ptr, _) = &stmt.kind
                    && let Operand::Copy(l) | Operand::Move(l) = ptr
                {
                    *assign_counts.entry(*l).or_insert(0) += if in_loop { 2 } else { 1 };
                }
            }
        }

        let mut safe_constants: HashMap<Local, Constant> = HashMap::default();
        for (local, count) in &assign_counts {
            if *count == 1
                && let Some(c) = constant_assignments.get(local)
            {
                safe_constants.insert(*local, c.clone());
            }
        }

        let mut changed = false;

        if !safe_constants.is_empty() {
            for bb in &mut func.basic_blocks {
                for stmt in &mut bb.statements {
                    if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                        changed |= self.propagate_constants_in_rvalue(rval, &safe_constants);
                    } else if let StatementKind::SetIndex(obj, idx, val, _) = &mut stmt.kind {
                        changed |= self.propagate_constants_in_operand(obj, &safe_constants);
                        changed |= self.propagate_constants_in_operand(idx, &safe_constants);
                        changed |= self.propagate_constants_in_operand(val, &safe_constants);
                    } else if let StatementKind::SetAttr(obj, _, val) = &mut stmt.kind {
                        changed |= self.propagate_constants_in_operand(obj, &safe_constants);
                        changed |= self.propagate_constants_in_operand(val, &safe_constants);
                    } else if let StatementKind::VectorStore(obj, idx, val) = &mut stmt.kind {
                        changed |= self.propagate_constants_in_operand(obj, &safe_constants);
                        changed |= self.propagate_constants_in_operand(idx, &safe_constants);
                        changed |= self.propagate_constants_in_operand(val, &safe_constants);
                    }
                }
                if let Some(term) = &mut bb.terminator
                    && let TerminatorKind::SwitchInt { discr, .. } = &mut term.kind
                {
                    changed |= self.propagate_constants_in_operand(discr, &safe_constants);
                }
            }
        }

        for bb in &mut func.basic_blocks {
            let mut local_consts: HashMap<Local, Constant> = HashMap::default();
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(dest, rval) = &mut stmt.kind {
                    changed |= self.propagate_constants_in_rvalue(rval, &local_consts);

                    if let Rvalue::Use(Operand::Constant(c)) = rval
                        && !matches!(func.locals[dest.0].ty, Type::Union(_))
                    {
                        local_consts.insert(*dest, c.clone());
                    } else {
                        local_consts.remove(dest);
                    }
                }
            }
        }

        changed
    }
}

impl ConstantPropagation {
    fn propagate_constants_in_rvalue(
        &self,
        rval: &mut Rvalue,
        map: &HashMap<Local, Constant>,
    ) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => {
                self.propagate_constants_in_operand(op, map)
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r, _) => {
                let mut changed = self.propagate_constants_in_operand(l, map);
                changed |= self.propagate_constants_in_operand(r, map);
                changed
            }
            Rvalue::Call { func, args } => {
                let mut changed = self.propagate_constants_in_operand(func, map);
                for arg in args {
                    changed |= self.propagate_constants_in_operand(arg, map);
                }
                changed
            }
            Rvalue::Aggregate(_, ops) => {
                let mut changed = false;
                for op in ops {
                    changed |= self.propagate_constants_in_operand(op, map);
                }
                changed
            }
            Rvalue::PtrLoad(op) => self.propagate_constants_in_operand(op, map),
            Rvalue::VectorSplat(op, _) => self.propagate_constants_in_operand(op, map),
            Rvalue::VectorLoad(obj, idx, _) => {
                let mut changed = self.propagate_constants_in_operand(obj, map);
                changed |= self.propagate_constants_in_operand(idx, map);
                changed
            }
            Rvalue::VectorFMA(a, b, c) => {
                let mut changed = self.propagate_constants_in_operand(a, map);
                changed |= self.propagate_constants_in_operand(b, map);
                changed |= self.propagate_constants_in_operand(c, map);
                changed
            }
            _ => false,
        }
    }

    fn propagate_constants_in_operand(
        &self,
        op: &mut Operand,
        map: &HashMap<Local, Constant>,
    ) -> bool {
        if let Operand::Copy(l) | Operand::Move(l) = op
            && let Some(c) = map.get(l)
        {
            *op = Operand::Constant(c.clone());
            return true;
        }
        false
    }
}

#[cfg(test)]
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

    fn func(name: &str, locals: Vec<LocalDecl>, stmts: Vec<Statement>, args: usize) -> MirFunction {
        MirFunction {
            name: name.into(),
            locals,
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

    fn local_decl() -> LocalDecl {
        LocalDecl {
            ty: crate::semantic::types::Type::Int,
            name: None,
            span: sp(),
            is_mut: false,
            is_owning: false,
        }
    }

    #[test]
    fn propagate_single_use_copy() {
        let mut f = func(
            "f",
            vec![local_decl(), local_decl()],
            vec![
                assign(1, Rvalue::Use(Operand::Constant(Constant::Int(42)))),
                assign(
                    0,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Add,
                        Operand::Copy(Local(1)),
                        Operand::Constant(Constant::Int(1)),
                    ),
                ),
            ],
            0,
        );
        assert!(ConstantPropagation.run(&mut f));
        match &f.basic_blocks[0].statements[1].kind {
            StatementKind::Assign(
                _,
                Rvalue::BinaryOp(_, Operand::Constant(Constant::Int(42)), _),
            ) => {}
            _ => panic!("local(1) should be replaced by constant 42"),
        }
    }

    #[test]
    fn no_prop_when_unknown_source() {
        let mut f = func(
            "f",
            vec![local_decl(), local_decl()],
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(1)),
                ),
            )],
            1,
        );
        assert!(!ConstantPropagation.run(&mut f));
    }

    #[test]
    fn propagate_into_switch_discr() {
        let mut f = MirFunction {
            name: "f".into(),
            locals: vec![local_decl(), local_decl()],
            basic_blocks: vec![
                BasicBlock {
                    statements: vec![assign(
                        1,
                        Rvalue::Use(Operand::Constant(Constant::Bool(true))),
                    )],
                    terminator: Some(Terminator {
                        kind: TerminatorKind::SwitchInt {
                            discr: Operand::Copy(Local(1)),
                            targets: vec![(1, BasicBlockId(1))],
                            otherwise: BasicBlockId(2),
                        },
                        span: sp(),
                    }),
                },
                BasicBlock {
                    statements: vec![],
                    terminator: Some(Terminator {
                        kind: TerminatorKind::Return,
                        span: sp(),
                    }),
                },
            ],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        };
        assert!(ConstantPropagation.run(&mut f));
        match &f.basic_blocks[0].terminator.as_ref().unwrap().kind {
            TerminatorKind::SwitchInt {
                discr: Operand::Constant(Constant::Bool(true)),
                ..
            } => {}
            _ => panic!("discr should be constant true"),
        }
    }

    #[test]
    fn propagate_nothing_no_constants() {
        let mut f = func(
            "f",
            vec![local_decl(), local_decl()],
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(1)),
                ),
            )],
            1,
        );
        assert!(!ConstantPropagation.run(&mut f));
    }
}
