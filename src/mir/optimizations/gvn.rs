use super::Transform;
use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

pub struct GlobalValueNumbering;

impl Transform for GlobalValueNumbering {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;

        let mut value_map: HashMap<Rvalue, (Local, usize)> = HashMap::default();
        let mut assign_counts: HashMap<Local, usize> = HashMap::default();

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
                if let StatementKind::Assign(dest, _) = &stmt.kind {
                    *assign_counts.entry(*dest).or_insert(0) += if in_loop { 2 } else { 1 };
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

        for bb_idx in 0..func.basic_blocks.len() {
            value_map.clear();
            let mut i = 0;
            while i < func.basic_blocks[bb_idx].statements.len() {
                let stmt = &func.basic_blocks[bb_idx].statements[i];
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    let dest = *dest;
                    if matches!(rval, Rvalue::BinaryOp(..) | Rvalue::UnaryOp(..))
                        && !Self::rvalue_has_move(rval)
                        && self.operands_stable(rval, &assign_counts, func.arg_count)
                    {
                        if let Some(&(existing, _)) = value_map.get(rval) {
                            if existing != dest {
                                let new_rval = Rvalue::Use(Operand::Copy(existing));
                                if func.basic_blocks[bb_idx].statements[i].kind
                                    != StatementKind::Assign(dest, new_rval.clone())
                                {
                                    func.basic_blocks[bb_idx].statements[i].kind =
                                        StatementKind::Assign(dest, new_rval);
                                    changed = true;
                                }
                            }
                        } else if assign_counts.get(&dest) == Some(&1) {
                            value_map.insert(rval.clone(), (dest, bb_idx));
                        }
                    }

                    value_map.retain(|expr, _| !self.uses_local(expr, dest));
                }

                if matches!(
                    func.basic_blocks[bb_idx].statements[i].kind,
                    StatementKind::SetIndex(..)
                        | StatementKind::SetAttr(..)
                        | StatementKind::VectorStore(..)
                ) {
                    value_map.retain(|expr, _| {
                        !matches!(expr, Rvalue::GetIndex(..) | Rvalue::GetAttr(..))
                    });
                }

                if matches!(
                    &func.basic_blocks[bb_idx].statements[i].kind,
                    StatementKind::Assign(_, Rvalue::Call { .. })
                ) {
                    value_map.clear();
                }

                // A moved-from local's storage may be consumed or mutated in
                // place at runtime (e.g. string concat growing its left
                // operand): any cached value numbered under that local is no
                // longer safe to hand out as a substitute, or the runtime
                // gets two "sole owners" of the same freed/mutated buffer.
                let mut moved = Vec::new();
                Self::moved_locals(&func.basic_blocks[bb_idx].statements[i].kind, &mut moved);
                if !moved.is_empty() {
                    value_map.retain(|_, (existing, _)| !moved.contains(existing));
                }

                i += 1;
            }
        }
        changed
    }
}

impl GlobalValueNumbering {
    fn operands_stable(
        &self,
        rval: &Rvalue,
        counts: &HashMap<Local, usize>,
        arg_count: usize,
    ) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => self.op_stable(op, counts, arg_count),
            Rvalue::BinaryOp(_, l, r) => {
                self.op_stable(l, counts, arg_count) && self.op_stable(r, counts, arg_count)
            }
            _ => false,
        }
    }

    fn op_stable(&self, op: &Operand, counts: &HashMap<Local, usize>, arg_count: usize) -> bool {
        match op {
            Operand::Constant(_) => true,
            Operand::Copy(l) | Operand::Move(l) => l.0 <= arg_count || counts.get(l) == Some(&1),
        }
    }

    fn uses_local(&self, rval: &Rvalue, local: Local) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => self.is_local(op, local),
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r, _) => {
                self.is_local(l, local) || self.is_local(r, local)
            }
            _ => false,
        }
    }

    fn is_local(&self, op: &Operand, local: Local) -> bool {
        matches!(op, Operand::Copy(l) | Operand::Move(l) if *l == local)
    }

    /// Whether any operand of `rval` moves its source: value-numbering such
    /// an rvalue would try to hand its single-use result to a second
    /// destination, which the runtime consuming the moved operand makes unsound.
    fn rvalue_has_move(rval: &Rvalue) -> bool {
        let mut has = false;
        Self::for_each_operand(rval, &mut |op| {
            if matches!(op, Operand::Move(_)) {
                has = true;
            }
        });
        has
    }

    fn for_each_operand(rval: &Rvalue, f: &mut impl FnMut(&Operand)) {
        match rval {
            Rvalue::Use(op)
            | Rvalue::UnaryOp(_, op)
            | Rvalue::Cast(op, _)
            | Rvalue::GetAttr(op, _)
            | Rvalue::GetTag(op)
            | Rvalue::GetTypeId(op)
            | Rvalue::VectorSplat(op, _)
            | Rvalue::VectorReduce(_, op, _)
            | Rvalue::PtrLoad(op)
            | Rvalue::GenOf(op)
            | Rvalue::FatPtrData(op)
            | Rvalue::VTableLoad { vtable: op, .. } => f(op),
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r, _) | Rvalue::VectorLoad(l, r, _) => {
                f(l);
                f(r);
            }
            Rvalue::VectorFMA(a, b, c) => {
                f(a);
                f(b);
                f(c);
            }
            Rvalue::Call { func, args } => {
                f(func);
                for arg in args {
                    f(arg);
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for op in ops {
                    f(op);
                }
            }
            Rvalue::Ref(_) | Rvalue::MutRef(_) => {}
        }
    }

    /// Locals a statement consumes via `Operand::Move`. Any value cached
    /// under one of these in `value_map` must be dropped: its storage may
    /// now be freed or mutated, so it can no longer stand in for another use.
    fn moved_locals(kind: &StatementKind, out: &mut Vec<Local>) {
        let mut scan = |op: &Operand| {
            if let Operand::Move(l) = op {
                out.push(*l);
            }
        };
        match kind {
            StatementKind::Assign(_, rval) => Self::for_each_operand(rval, &mut scan),
            StatementKind::SetAttr(obj, _, val) => {
                scan(obj);
                scan(val);
            }
            StatementKind::SetIndex(obj, idx, val, _) => {
                scan(obj);
                scan(idx);
                scan(val);
            }
            StatementKind::VectorStore(obj, idx, val) => {
                scan(obj);
                scan(idx);
                scan(val);
            }
            StatementKind::PtrStore(ptr, val) => {
                scan(ptr);
                scan(val);
            }
            _ => {}
        }
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
    fn replaces_duplicate_binop() {
        let mut f = func(
            "f",
            vec![local_decl(), local_decl(), local_decl()],
            vec![
                assign(
                    2,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Add,
                        Operand::Copy(Local(0)),
                        Operand::Copy(Local(1)),
                    ),
                ),
                assign(
                    3,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Add,
                        Operand::Copy(Local(0)),
                        Operand::Copy(Local(1)),
                    ),
                ),
            ],
            2,
        );
        assert!(GlobalValueNumbering.run(&mut f));
        match &f.basic_blocks[0].statements[1].kind {
            StatementKind::Assign(_, Rvalue::Use(Operand::Copy(Local(2)))) => {}
            _ => panic!("expected Use(Copy(Local(2)))"),
        }
    }

    #[test]
    fn no_change_unique_binops() {
        let mut f = func(
            "f",
            vec![local_decl(), local_decl(), local_decl()],
            vec![
                assign(
                    2,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Add,
                        Operand::Copy(Local(0)),
                        Operand::Copy(Local(1)),
                    ),
                ),
                assign(
                    3,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Sub,
                        Operand::Copy(Local(0)),
                        Operand::Copy(Local(1)),
                    ),
                ),
            ],
            2,
        );
        assert!(!GlobalValueNumbering.run(&mut f));
    }

    #[test]
    fn no_change_call_clears_map() {
        let mut f = func(
            "f",
            vec![local_decl(), local_decl(), local_decl()],
            vec![
                assign(
                    2,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Add,
                        Operand::Copy(Local(0)),
                        Operand::Copy(Local(1)),
                    ),
                ),
                assign(
                    3,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("g".into())),
                        args: vec![],
                    },
                ),
                assign(
                    4,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Add,
                        Operand::Copy(Local(0)),
                        Operand::Copy(Local(1)),
                    ),
                ),
            ],
            2,
        );
        assert!(!GlobalValueNumbering.run(&mut f));
    }
}
