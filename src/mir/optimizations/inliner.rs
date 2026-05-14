use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

pub struct Inliner;

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
                                // do not inline async fns — their return must be wrapped by codegen
                                if target_fn.is_async {
                                    continue;
                                }
                                // do not inline self-recursive functions (avoids exponential blowup)
                                let is_recursive = target_fn.basic_blocks.iter().any(|bb| {
                                    bb.statements.iter().any(|s| {
                                        matches!(&s.kind, StatementKind::Assign(_, Rvalue::Call {
                                            func: Operand::Constant(Constant::Function(n)), ..
                                        }) if n == &target_fn.name)
                                    })
                                });
                                if is_recursive {
                                    continue;
                                }
                                // inline small functions
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

    fn perform_inline(
        &self,
        caller: &mut MirFunction,
        bb_idx: usize,
        stmt_idx: usize,
        callee: &MirFunction,
        args: &[Operand],
    ) {
        let local_offset = caller.locals.len();

        // copy locals
        for decl in &callee.locals {
            caller.locals.push(decl.clone());
        }

        // split block
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

        // map blocks
        let block_offset = caller.basic_blocks.len() + 1; // +1 because we'll add the tail later
        let mut callee_bb_map = HashMap::default();
        for (i, _) in callee.basic_blocks.iter().enumerate() {
            callee_bb_map.insert(BasicBlockId(i), BasicBlockId(block_offset + i));
        }

        // connect entry
        caller.basic_blocks[bb_idx].terminator = Some(Terminator {
            kind: TerminatorKind::Goto {
                target: BasicBlockId(block_offset),
            },
            span: call_stmt.span,
        });

        // connect params
        let mut init_stmts = Vec::new();
        for (j, arg) in args.iter().enumerate() {
            let param_local = Local(local_offset + j + 1);
            // mark storage live
            init_stmts.push(Statement {
                kind: StatementKind::StorageLive(param_local),
                span: call_stmt.span,
            });
            init_stmts.push(Statement {
                kind: StatementKind::Assign(param_local, Rvalue::Use(arg.clone())),
                span: call_stmt.span,
            });
        }

        // mark locals live
        for j in (callee.arg_count + 1)..callee.locals.len() {
            init_stmts.push(Statement {
                kind: StatementKind::StorageLive(Local(local_offset + j)),
                span: call_stmt.span,
            });
        }
        // local 0 is return
        init_stmts.push(Statement {
            kind: StatementKind::StorageLive(Local(local_offset)),
            span: call_stmt.span,
        });

        // translate blocks
        let mut translated_blocks = Vec::new();
        for (i, bb) in callee.basic_blocks.iter().enumerate() {
            let mut new_bb = bb.clone();

            // remap locals
            for stmt in &mut new_bb.statements {
                self.remap_statement(stmt, local_offset);
            }

            // prepend param init
            if i == 0 {
                let mut combined = init_stmts.clone();
                combined.extend(new_bb.statements);
                new_bb.statements = combined;
            }

            // remap terminator
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
                        // replace return with goto
                        if let Some(dest) = ret_local {
                            // assign return value
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
                // implicit return
                new_bb.terminator = Some(Terminator {
                    kind: TerminatorKind::Goto { target: tail_bb_id },
                    span: call_stmt.span,
                });
            }
            translated_blocks.push(new_bb);
        }

        // add blocks
        caller.basic_blocks.push(tail_bb); // This will have ID tail_bb_id
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
        }
    }

    fn remap_rvalue(&self, rval: &mut Rvalue, offset: usize) {
        match rval {
            Rvalue::Use(op)
            | Rvalue::UnaryOp(_, op)
            | Rvalue::GetAttr(op, _)
            | Rvalue::GetTag(op)
            | Rvalue::GetTypeId(op) => {
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
