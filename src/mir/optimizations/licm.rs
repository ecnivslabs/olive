use crate::mir::loop_utils;
use crate::mir::optimizations::Transform;
use crate::mir::*;
use crate::span::Span;
use rustc_hash::FxHashSet as HashSet;

pub struct Licm;

impl Transform for Licm {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        let loops = loop_utils::find_loops(func);
        let mut processed_headers = HashSet::default();

        for lp in loops {
            if lp.header.0 == 0 || processed_headers.contains(&lp.header) {
                continue;
            }
            processed_headers.insert(lp.header);

            if self.optimize_loop(func, &lp) {
                changed = true;
                break;
            }
        }

        changed
    }
}

impl Licm {
    fn optimize_loop(&self, func: &mut MirFunction, lp: &loop_utils::Loop) -> bool {
        let mut assign_counts = rustc_hash::FxHashMap::default();
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(local, _) = &stmt.kind {
                    *assign_counts.entry(*local).or_insert(0) += 1;
                } else if let StatementKind::SetAttr(obj, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        *assign_counts.entry(*l).or_insert(0) += 1;
                    }
                } else if let StatementKind::SetIndex(obj, _, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        *assign_counts.entry(*l).or_insert(0) += 1;
                    }
                } else if let StatementKind::VectorStore(obj, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        *assign_counts.entry(*l).or_insert(0) += 1;
                    }
                } else if let StatementKind::PtrStore(ptr, _) = &stmt.kind
                    && let Operand::Copy(l) | Operand::Move(l) = ptr
                {
                    *assign_counts.entry(*l).or_insert(0) += 1;
                }
            }
        }

        let mut defined_in_loop = HashSet::default();
        for &bb_id in &lp.body {
            let bb = &func.basic_blocks[bb_id.0];
            for stmt in &bb.statements {
                if let StatementKind::Assign(local, _) = &stmt.kind {
                    defined_in_loop.insert(*local);
                } else if let StatementKind::SetAttr(obj, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        defined_in_loop.insert(*l);
                    }
                } else if let StatementKind::SetIndex(obj, _, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        defined_in_loop.insert(*l);
                    }
                } else if let StatementKind::VectorStore(obj, _, _) = &stmt.kind {
                    if let Operand::Copy(l) | Operand::Move(l) = obj {
                        defined_in_loop.insert(*l);
                    }
                } else if let StatementKind::PtrStore(ptr, _) = &stmt.kind
                    && let Operand::Copy(l) | Operand::Move(l) = ptr
                {
                    defined_in_loop.insert(*l);
                }
            }
        }

        let mut sorted_body: Vec<BasicBlockId> = lp.body.iter().copied().collect();
        sorted_body.sort_by_key(|id| id.0);

        let mut invariant_stmts = Vec::new();
        let mut invariant_locals = HashSet::default();
        let mut loop_changed = true;

        while loop_changed {
            loop_changed = false;
            for &bb_id in &sorted_body {
                let bb = &func.basic_blocks[bb_id.0];
                for (i, stmt) in bb.statements.iter().enumerate() {
                    if let StatementKind::Assign(local, rval) = &stmt.kind
                        && func.locals[local.0].name.is_none()
                        && assign_counts.get(local) == Some(&1)
                        && !invariant_locals.contains(local)
                        // Hoisting a move-type value past its Drop would free it early.
                        && !func.locals[local.0].ty.is_move_type()
                        && self.is_invariant(rval, &defined_in_loop, &invariant_locals)
                        && self.is_safe_to_hoist(rval)
                    {
                        invariant_locals.insert(*local);
                        invariant_stmts.push((bb_id, i));
                        loop_changed = true;
                    }
                }
            }
        }

        if !invariant_stmts.is_empty() {
            self.hoist_invariants(func, lp.header, &lp.body, invariant_stmts)
        } else {
            false
        }
    }

    fn is_invariant(
        &self,
        rval: &Rvalue,
        defined_in_loop: &HashSet<Local>,
        invariant_locals: &HashSet<Local>,
    ) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => {
                self.is_op_invariant(op, defined_in_loop, invariant_locals)
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r, _) => {
                self.is_op_invariant(l, defined_in_loop, invariant_locals)
                    && self.is_op_invariant(r, defined_in_loop, invariant_locals)
            }
            _ => false,
        }
    }

    fn is_op_invariant(
        &self,
        op: &Operand,
        defined_in_loop: &HashSet<Local>,
        invariant_locals: &HashSet<Local>,
    ) -> bool {
        match op {
            Operand::Constant(_) => true,
            Operand::Copy(l) | Operand::Move(l) => {
                !defined_in_loop.contains(l) || invariant_locals.contains(l)
            }
        }
    }

    fn is_safe_to_hoist(&self, rval: &Rvalue) -> bool {
        matches!(
            rval,
            Rvalue::Use(_)
                | Rvalue::UnaryOp(_, _)
                | Rvalue::BinaryOp(_, _, _)
                | Rvalue::GetIndex(_, _, _)
        )
    }

    fn hoist_invariants(
        &self,
        func: &mut MirFunction,
        header: BasicBlockId,
        body: &HashSet<BasicBlockId>,
        invariant_stmts: Vec<(BasicBlockId, usize)>,
    ) -> bool {
        let pre_header_id = BasicBlockId(func.basic_blocks.len());
        func.basic_blocks.push(BasicBlock {
            statements: Vec::new(),
            terminator: Some(Terminator {
                kind: TerminatorKind::Goto { target: header },
                span: Span::default(),
            }),
        });

        let mut changed = false;
        for i in 0..func.basic_blocks.len() - 1 {
            let bb_id = BasicBlockId(i);
            if body.contains(&bb_id) {
                continue;
            }

            let bb = &mut func.basic_blocks[i];
            if let Some(term) = &mut bb.terminator {
                match &mut term.kind {
                    TerminatorKind::Goto { target } if *target == header => {
                        *target = pre_header_id;
                        changed = true;
                    }
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        for (_, t) in targets {
                            if *t == header {
                                *t = pre_header_id;
                                changed = true;
                            }
                        }
                        if *otherwise == header {
                            *otherwise = pre_header_id;
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
        }

        if !changed {
            func.basic_blocks.pop();
            return false;
        }

        let mut stmts_to_move = Vec::new();
        for (bb_id, stmt_idx) in invariant_stmts {
            let stmt = func.basic_blocks[bb_id.0].statements[stmt_idx].clone();
            stmts_to_move.push((bb_id, stmt_idx, stmt));
        }

        let mut to_remove = stmts_to_move
            .iter()
            .map(|(bb, idx, _)| (*bb, *idx))
            .collect::<Vec<_>>();
        to_remove.sort_by(|a, b| {
            if a.0 != b.0 {
                a.0.0.cmp(&b.0.0)
            } else {
                b.1.cmp(&a.1)
            }
        });

        for (bb_id, idx) in to_remove {
            func.basic_blocks[bb_id.0].statements.remove(idx);
        }

        for (_, _, stmt) in stmts_to_move {
            func.basic_blocks[pre_header_id.0].statements.push(stmt);
        }

        true
    }
}

#[cfg(test)]
#[cfg_attr(test, allow(dead_code))]
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

    fn stmt(k: StatementKind) -> Statement {
        Statement {
            kind: k,
            span: sp(),
        }
    }

    fn func(blocks: Vec<BasicBlock>) -> MirFunction {
        MirFunction {
            name: "f".into(),
            locals: vec![],
            basic_blocks: blocks,
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    fn bb(stmts: Vec<Statement>, kind: TerminatorKind) -> BasicBlock {
        BasicBlock {
            statements: stmts,
            terminator: Some(Terminator { kind, span: sp() }),
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
    fn no_loops_no_change() {
        let f = func(vec![bb(vec![], TerminatorKind::Return)]);
        let mut f2 = f.clone();
        assert!(!Licm.run(&mut f2));
    }

    #[test]
    fn preheader_not_inserted_when_no_loop() {
        let mut f = func(vec![
            bb(
                vec![],
                TerminatorKind::Goto {
                    target: BasicBlockId(1),
                },
            ),
            bb(vec![], TerminatorKind::Return),
        ]);
        assert!(!Licm.run(&mut f));
    }
}
