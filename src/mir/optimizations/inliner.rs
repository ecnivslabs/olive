use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

pub struct Inliner;

impl Default for Inliner {
    fn default() -> Self {
        Self::new()
    }
}

impl Inliner {
    pub fn new() -> Self {
        Self
    }

    pub fn inline_function(
        &self,
        func: &mut MirFunction,
        fn_map: &HashMap<String, MirFunction>,
        max_depth: usize,
    ) {
        let mut changed = true;
        let mut current_depth = 0;

        while changed && current_depth < max_depth {
            changed = false;
            let mut i = 0;
            while i < func.basic_blocks.len() {
                let mut call_found = None;
                {
                    let bb = &func.basic_blocks[i];
                    for (stmt_idx, stmt) in bb.statements.iter().enumerate() {
                        if let StatementKind::Assign(
                            _,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(name)),
                                args,
                            },
                        ) = &stmt.kind
                        {
                            if name == &func.name {
                                continue;
                            }
                            if let Some(target_fn) = fn_map.get(name) {
                                if target_fn.is_async {
                                    continue;
                                }
                                // Don't inline a callee on a recursion cycle
                                // (direct or mutual); it re-grows every pass to
                                // the depth limit and leaves a broken body.
                                if Self::reaches(name, name, fn_map) {
                                    continue;
                                }
                                if target_fn.basic_blocks.len() < 100 {
                                    call_found = Some((stmt_idx, name.clone(), args.clone()));
                                    break;
                                }
                            }
                        }
                    }
                }

                if let Some((stmt_idx, target_name, args)) = call_found {
                    self.perform_inline(
                        func,
                        i,
                        stmt_idx,
                        fn_map.get(&target_name).unwrap(),
                        &args,
                    );
                    changed = true;
                    current_depth += 1;
                    break;
                }
                i += 1;
            }
        }
    }

    /// Whether `from` can transitively call `target`. Called with `from ==
    /// target` to detect a recursion cycle.
    fn reaches(from: &str, target: &str, fn_map: &HashMap<String, MirFunction>) -> bool {
        let mut stack = vec![from.to_string()];
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut first = true;
        while let Some(name) = stack.pop() {
            if !first && name == target {
                return true;
            }
            first = false;
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(f) = fn_map.get(&name) else { continue };
            for bb in &f.basic_blocks {
                for s in &bb.statements {
                    if let StatementKind::Assign(
                        _,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(n)),
                            ..
                        },
                    ) = &s.kind
                    {
                        if n == target {
                            return true;
                        }
                        stack.push(n.clone());
                    }
                }
            }
        }
        false
    }

    fn perform_inline(
        &self,
        caller: &mut MirFunction,
        bb_idx: usize,
        stmt_idx: usize,
        callee: &MirFunction,
        args: &[Operand],
    ) {
        let local_offset = caller.locals.len();

        for decl in &callee.locals {
            caller.locals.push(decl.clone());
        }

        let mut tail_statements = caller.basic_blocks[bb_idx].statements.split_off(stmt_idx);
        let call_stmt = tail_statements.remove(0);
        let ret_local = if let StatementKind::Assign(l, _) = call_stmt.kind {
            Some(l)
        } else {
            None
        };

        let tail_bb_id = BasicBlockId(caller.basic_blocks.len());
        let tail_bb = BasicBlock {
            statements: tail_statements,
            terminator: caller.basic_blocks[bb_idx].terminator.take(),
        };

        let block_offset = caller.basic_blocks.len() + 1;
        let mut callee_bb_map = HashMap::default();
        for (i, _) in callee.basic_blocks.iter().enumerate() {
            callee_bb_map.insert(BasicBlockId(i), BasicBlockId(block_offset + i));
        }

        caller.basic_blocks[bb_idx].terminator = Some(Terminator {
            kind: TerminatorKind::Goto {
                target: BasicBlockId(block_offset),
            },
            span: call_stmt.span,
        });

        let mut init_stmts = Vec::new();
        for (j, arg) in args.iter().enumerate() {
            let param_local = Local(local_offset + j + 1);
            init_stmts.push(Statement {
                kind: StatementKind::StorageLive(param_local),
                span: call_stmt.span,
            });
            init_stmts.push(Statement {
                kind: StatementKind::Assign(param_local, Rvalue::Use(arg.clone())),
                span: call_stmt.span,
            });
        }

        for j in (callee.arg_count + 1)..callee.locals.len() {
            init_stmts.push(Statement {
                kind: StatementKind::StorageLive(Local(local_offset + j)),
                span: call_stmt.span,
            });
        }
        init_stmts.push(Statement {
            kind: StatementKind::StorageLive(Local(local_offset)),
            span: call_stmt.span,
        });

        let mut translated_blocks = Vec::new();
        for (i, bb) in callee.basic_blocks.iter().enumerate() {
            let mut new_bb = bb.clone();

            for stmt in &mut new_bb.statements {
                self.remap_statement(stmt, local_offset);
            }

            if i == 0 {
                let mut combined = init_stmts.clone();
                combined.extend(new_bb.statements);
                new_bb.statements = combined;
            }

            if let Some(term) = &mut new_bb.terminator {
                match &mut term.kind {
                    TerminatorKind::Goto { target } => {
                        *target = *callee_bb_map.get(target).unwrap();
                    }
                    TerminatorKind::SwitchInt {
                        discr,
                        targets,
                        otherwise,
                    } => {
                        self.remap_operand(discr, local_offset);
                        for (_, target) in targets {
                            *target = *callee_bb_map.get(target).unwrap();
                        }
                        *otherwise = *callee_bb_map.get(otherwise).unwrap();
                    }
                    TerminatorKind::Return => {
                        if let Some(dest) = ret_local {
                            new_bb.statements.retain(|s| {
                                !matches!(&s.kind, StatementKind::StorageDead(l) if l.0 == local_offset)
                            });
                            new_bb.statements.push(Statement {
                                kind: StatementKind::Assign(
                                    dest,
                                    Rvalue::Use(Operand::Copy(Local(local_offset))),
                                ),
                                span: term.span,
                            });
                        }
                        term.kind = TerminatorKind::Goto { target: tail_bb_id };
                    }
                    _ => {}
                }
            } else {
                new_bb.terminator = Some(Terminator {
                    kind: TerminatorKind::Goto { target: tail_bb_id },
                    span: call_stmt.span,
                });
            }
            translated_blocks.push(new_bb);
        }

        caller.basic_blocks.push(tail_bb);
        caller.basic_blocks.extend(translated_blocks);
    }

    fn remap_statement(&self, stmt: &mut Statement, offset: usize) {
        match &mut stmt.kind {
            StatementKind::Assign(l, rval) => {
                l.0 += offset;
                self.remap_rvalue(rval, offset);
            }
            StatementKind::SetAttr(obj, _, val) => {
                self.remap_operand(obj, offset);
                self.remap_operand(val, offset);
            }
            StatementKind::SetIndex(obj, idx, val) => {
                self.remap_operand(obj, offset);
                self.remap_operand(idx, offset);
                self.remap_operand(val, offset);
            }
            StatementKind::StorageLive(l)
            | StatementKind::StorageDead(l)
            | StatementKind::Drop(l) => {
                l.0 += offset;
            }
            StatementKind::VectorStore(obj, idx, val) => {
                self.remap_operand(obj, offset);
                self.remap_operand(idx, offset);
                self.remap_operand(val, offset);
            }
            StatementKind::PtrStore(ptr, val) => {
                self.remap_operand(ptr, offset);
                self.remap_operand(val, offset);
            }
        }
    }

    fn remap_rvalue(&self, rval: &mut Rvalue, offset: usize) {
        match rval {
            Rvalue::Use(op)
            | Rvalue::UnaryOp(_, op)
            | Rvalue::GetAttr(op, _)
            | Rvalue::GetTag(op)
            | Rvalue::GetTypeId(op)
            | Rvalue::FatPtrData(op)
            | Rvalue::Cast(op, _) => {
                self.remap_operand(op, offset);
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
                self.remap_operand(l, offset);
                self.remap_operand(r, offset);
            }
            Rvalue::Call { func, args } => {
                self.remap_operand(func, offset);
                for arg in args {
                    self.remap_operand(arg, offset);
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for op in ops {
                    self.remap_operand(op, offset);
                }
            }
            Rvalue::Ref(l) | Rvalue::MutRef(l) => {
                l.0 += offset;
            }
            Rvalue::PtrLoad(op) => self.remap_operand(op, offset),
            Rvalue::VTableLoad {
                vtable,
                method_idx: _,
            } => {
                self.remap_operand(vtable, offset);
            }
            Rvalue::VectorSplat(op, _) => self.remap_operand(op, offset),
            Rvalue::VectorLoad(obj, idx, _) => {
                self.remap_operand(obj, offset);
                self.remap_operand(idx, offset);
            }
            Rvalue::VectorFMA(a, b, c) => {
                self.remap_operand(a, offset);
                self.remap_operand(b, offset);
                self.remap_operand(c, offset);
            }
        }
    }

    fn remap_operand(&self, op: &mut Operand, offset: usize) {
        match op {
            Operand::Copy(l) | Operand::Move(l) => {
                l.0 += offset;
            }
            _ => {}
        }
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

    fn callee_fn(name: &str, body: Vec<Statement>) -> MirFunction {
        MirFunction {
            name: name.into(),
            locals: vec![local_decl()],
            basic_blocks: vec![BasicBlock {
                statements: body,
                terminator: Some(Terminator {
                    kind: TerminatorKind::Return,
                    span: sp(),
                }),
            }],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    #[test]
    fn simple_inline_replaces_call() {
        let mut caller = func(
            "main",
            vec![local_decl()],
            vec![assign(
                0,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("callee".into())),
                    args: vec![],
                },
            )],
            0,
        );
        let callee = callee_fn(
            "callee",
            vec![assign(0, Rvalue::Use(Operand::Constant(Constant::Int(42))))],
        );
        let mut fn_map: HashMap<String, MirFunction> = HashMap::default();
        fn_map.insert("callee".into(), callee);
        let inliner = Inliner::new();
        inliner.inline_function(&mut caller, &fn_map, 3);
        assert!(caller.basic_blocks.len() > 1, "expected inlined blocks");
    }

    #[test]
    fn no_inline_unknown_function() {
        let mut caller = func(
            "main",
            vec![local_decl()],
            vec![assign(
                0,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("unknown".into())),
                    args: vec![],
                },
            )],
            0,
        );
        let fn_map: HashMap<String, MirFunction> = HashMap::default();
        let inliner = Inliner::new();
        inliner.inline_function(&mut caller, &fn_map, 3);
        assert_eq!(caller.basic_blocks.len(), 1);
    }

    #[test]
    fn no_inline_mutually_recursive_callee() {
        // `a` calls `b`, `b` calls `a`: inlining either must be refused.
        let a = callee_fn(
            "a",
            vec![assign(
                0,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("b".into())),
                    args: vec![],
                },
            )],
        );
        let b = callee_fn(
            "b",
            vec![assign(
                0,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("a".into())),
                    args: vec![],
                },
            )],
        );
        let mut fn_map: HashMap<String, MirFunction> = HashMap::default();
        fn_map.insert("a".into(), a);
        fn_map.insert("b".into(), b);
        let mut caller = func(
            "main",
            vec![local_decl()],
            vec![assign(
                0,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("a".into())),
                    args: vec![],
                },
            )],
            0,
        );
        let inliner = Inliner::new();
        inliner.inline_function(&mut caller, &fn_map, 5);
        assert_eq!(
            caller.basic_blocks.len(),
            1,
            "must not inline a callee in a recursion cycle"
        );
    }

    #[test]
    fn no_inline_if_not_in_map() {
        let mut caller = func(
            "f",
            vec![local_decl()],
            vec![assign(
                0,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("other".into())),
                    args: vec![],
                },
            )],
            0,
        );
        // empty fn_map, no inlining happens
        let fn_map: HashMap<String, MirFunction> = HashMap::default();
        let inliner = Inliner::new();
        inliner.inline_function(&mut caller, &fn_map, 3);
        assert_eq!(caller.basic_blocks.len(), 1);
    }
}
