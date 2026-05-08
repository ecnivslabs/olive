use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

pub struct Inliner;

impl Inliner {
    pub fn new() -> Self {
        Self
    }

    pub fn run(&self, functions: &mut Vec<MirFunction>) {
        let fn_map: HashMap<String, MirFunction> = functions.iter()
            .map(|f| (f.name.clone(), f.clone()))
            .collect();

        for func in functions.iter_mut() {
            self.inline_function(func, &fn_map, 10);
            
            let mut changed = true;
            while changed {
                changed = false;
                changed |= self.constant_folding(func);
                changed |= self.peephole_optimize(func);
                changed |= self.common_subexpression_elimination(func);
                changed |= self.copy_propagation(func);
                changed |= self.dead_code_elimination(func);
            }
        }
    }

    fn constant_folding(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    if let Rvalue::BinaryOp(op, Operand::Constant(Constant::Int(a)), Operand::Constant(Constant::Int(b))) = rval {
                        use crate::parser::BinOp::*;
                        let res = match op {
                            Add => Some((*a).wrapping_add(*b)),
                            Sub => Some((*a).wrapping_sub(*b)),
                            Mul => Some((*a).wrapping_mul(*b)),
                            Div | FloorDiv => if *b != 0 { Some(*a / *b) } else { None },
                            Mod => if *b != 0 { Some(*a % *b) } else { None },
                            Eq => Some(if *a == *b { 1 } else { 0 }),
                            Lt => Some(if *a < *b { 1 } else { 0 }),
                            LtEq => Some(if *a <= *b { 1 } else { 0 }),
                            Gt => Some(if *a > *b { 1 } else { 0 }),
                            GtEq => Some(if *a >= *b { 1 } else { 0 }),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Int(val)));
                            changed = true;
                        }
                    }
                }
            }
        }
        changed
    }

    fn peephole_optimize(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    use crate::parser::BinOp::*;
                    match rval {
                        Rvalue::BinaryOp(Add, op, Operand::Constant(Constant::Int(0))) |
                        Rvalue::BinaryOp(Add, Operand::Constant(Constant::Int(0)), op) |
                        Rvalue::BinaryOp(Sub, op, Operand::Constant(Constant::Int(0))) |
                        Rvalue::BinaryOp(Mul, op, Operand::Constant(Constant::Int(1))) |
                        Rvalue::BinaryOp(Mul, Operand::Constant(Constant::Int(1)), op) |
                        Rvalue::BinaryOp(Div, op, Operand::Constant(Constant::Int(1))) |
                        Rvalue::BinaryOp(FloorDiv, op, Operand::Constant(Constant::Int(1))) => {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(Mul, _, op @ Operand::Constant(Constant::Int(0))) |
                        Rvalue::BinaryOp(Mul, op @ Operand::Constant(Constant::Int(0)), _) => {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        changed
    }

    fn common_subexpression_elimination(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            // Mapping from Rvalue to the local that holds its result.
            // We only do this per basic block for simplicity (Local CSE).
            let mut available_expressions: Vec<(Rvalue, Local)> = Vec::new();
            
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(dest, rval) = &mut stmt.kind {
                    // Only eliminate pure expressions (no calls or attributes which might change)
                    if matches!(rval, Rvalue::BinaryOp(..) | Rvalue::UnaryOp(..) | Rvalue::Use(..)) {
                        let mut found = None;
                        for (expr, local) in &available_expressions {
                            if expr == rval {
                                found = Some(*local);
                                break;
                            }
                        }
                        
                        if let Some(existing_local) = found {
                            if *dest != existing_local {
                                *rval = Rvalue::Use(Operand::Copy(existing_local));
                                changed = true;
                            }
                        } else {
                            available_expressions.push((rval.clone(), *dest));
                        }
                    }
                    
                    // If dest is overwritten, it might invalidate expressions that used it.
                    // But in SSA-like MIR, destinations are usually unique or at least well-behaved.
                    // However, we should be careful. We'll clear expressions that use 'dest'.
                    available_expressions.retain(|(expr, _)| {
                        !self.uses_local(expr, *dest)
                    });
                }
            }
        }
        changed
    }

    fn uses_local(&self, rval: &Rvalue, local: Local) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => self.is_local(op, local),
            Rvalue::BinaryOp(_, l, r) => self.is_local(l, local) || self.is_local(r, local),
            _ => false,
        }
    }

    fn is_local(&self, op: &Operand, local: Local) -> bool {
        if let Operand::Copy(l) | Operand::Move(l) = op {
            return *l == local;
        }
        false
    }

    fn dead_code_elimination(&self, func: &mut MirFunction) -> bool {
        let mut used = std::collections::HashSet::new();
        // Return value is always used.
        used.insert(Local(0));
        
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                match &stmt.kind {
                    StatementKind::Assign(_, rval) => self.record_rvalue_usage(rval, &mut used),
                    StatementKind::SetAttr(obj, _, val) => {
                        self.record_operand_usage(obj, &mut used);
                        self.record_operand_usage(val, &mut used);
                    }
                    StatementKind::SetIndex(obj, idx, val) => {
                        self.record_operand_usage(obj, &mut used);
                        self.record_operand_usage(idx, &mut used);
                        self.record_operand_usage(val, &mut used);
                    }
                    _ => {}
                }
            }
            if let Some(term) = &bb.terminator {
                match &term.kind {
                    TerminatorKind::SwitchInt { discr, .. } => self.record_operand_usage(discr, &mut used),
                    _ => {}
                }
            }
        }

        let mut changed = false;
        for bb in &mut func.basic_blocks {
            let old_len = bb.statements.len();
            bb.statements.retain(|stmt| {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    // Don't eliminate calls as they might have side effects.
                    if matches!(rval, Rvalue::Call { .. }) { return true; }
                    used.contains(dest)
                } else {
                    true
                }
            });
            if bb.statements.len() != old_len {
                changed = true;
            }
        }
        changed
    }

    fn record_rvalue_usage(&self, rval: &Rvalue, used: &mut std::collections::HashSet<Local>) {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => self.record_operand_usage(op, used),
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
                self.record_operand_usage(l, used);
                self.record_operand_usage(r, used);
            }
            Rvalue::Call { func, args } => {
                self.record_operand_usage(func, used);
                for arg in args { self.record_operand_usage(arg, used); }
            }
            Rvalue::Aggregate(_, ops) => {
                for op in ops { self.record_operand_usage(op, used); }
            }
            Rvalue::Ref(l) | Rvalue::MutRef(l) => { used.insert(*l); }
        }
    }

    fn record_operand_usage(&self, op: &Operand, used: &mut std::collections::HashSet<Local>) {
        if let Operand::Copy(l) | Operand::Move(l) = op {
            used.insert(*l);
        }
    }

    fn copy_propagation(&self, func: &mut MirFunction) -> bool {
        let mut copies: HashMap<Local, Local> = HashMap::default();
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, Rvalue::Use(Operand::Copy(src) | Operand::Move(src))) = &stmt.kind {
                    if dest != src {
                        copies.insert(*dest, *src);
                    }
                }
            }
        }

        if copies.is_empty() { return false; }

        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                changed |= self.remap_statement_locals(stmt, &copies);
            }
            if let Some(term) = &mut bb.terminator {
                changed |= self.remap_terminator_locals(term, &copies);
            }
        }
        changed
    }

    fn remap_statement_locals(&self, stmt: &mut Statement, map: &HashMap<Local, Local>) -> bool {
        match &mut stmt.kind {
            StatementKind::Assign(_l, rval) => {
                // Don't remap the destination of the copy itself yet, 
                // but remap the rvalue.
                self.remap_rvalue_locals(rval, map)
            }
            StatementKind::SetAttr(obj, _, val) => {
                let mut changed = self.remap_operand_locals(obj, map);
                changed |= self.remap_operand_locals(val, map);
                changed
            }
            StatementKind::SetIndex(obj, idx, val) => {
                let mut changed = self.remap_operand_locals(obj, map);
                changed |= self.remap_operand_locals(idx, map);
                changed |= self.remap_operand_locals(val, map);
                changed
            }
            _ => false,
        }
    }

    fn remap_terminator_locals(&self, term: &mut Terminator, map: &HashMap<Local, Local>) -> bool {
        match &mut term.kind {
            TerminatorKind::SwitchInt { discr, .. } => {
                self.remap_operand_locals(discr, map)
            }
            _ => false,
        }
    }

    fn remap_rvalue_locals(&self, rval: &mut Rvalue, map: &HashMap<Local, Local>) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => {
                self.remap_operand_locals(op, map)
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
                let mut changed = self.remap_operand_locals(l, map);
                changed |= self.remap_operand_locals(r, map);
                changed
            }
            Rvalue::Call { func, args } => {
                let mut changed = self.remap_operand_locals(func, map);
                for arg in args {
                    changed |= self.remap_operand_locals(arg, map);
                }
                changed
            }
            Rvalue::Aggregate(_, ops) => {
                let mut changed = false;
                for op in ops {
                    changed |= self.remap_operand_locals(op, map);
                }
                changed
            }
            _ => false,
        }
    }

    fn remap_operand_locals(&self, op: &mut Operand, map: &HashMap<Local, Local>) -> bool {
        match op {
            Operand::Copy(l) | Operand::Move(l) => {
                if let Some(new_l) = map.get(l) {
                    *l = *new_l;
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn inline_function(&self, func: &mut MirFunction, fn_map: &HashMap<String, MirFunction>, max_depth: usize) {
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
                        if let StatementKind::Assign(_, Rvalue::Call { func: Operand::Constant(Constant::Function(name)), args }) = &stmt.kind {
                            if let Some(target_fn) = fn_map.get(name) {
                                // Inline if small or if it's the specific target we want to optimize.
                                // Limit recursion for the same function.
                                if target_fn.basic_blocks.len() < 100 {
                                    call_found = Some((stmt_idx, name.clone(), args.clone()));
                                    break;
                                }
                            }
                        }
                    }
                }

                if let Some((stmt_idx, target_name, args)) = call_found {
                    self.perform_inline(func, i, stmt_idx, fn_map.get(&target_name).unwrap(), &args);
                    changed = true;
                    current_depth += 1;
                    break;
                }
                i += 1;
            }
        }
    }

    fn perform_inline(&self, caller: &mut MirFunction, bb_idx: usize, stmt_idx: usize, callee: &MirFunction, args: &[Operand]) {
        let local_offset = caller.locals.len();
        
        // 1. Copy callee locals to caller.
        for decl in &callee.locals {
            caller.locals.push(decl.clone());
        }

        // 2. Split the current block at the call site.
        let mut tail_statements = caller.basic_blocks[bb_idx].statements.split_off(stmt_idx);
        let call_stmt = tail_statements.remove(0);
        let ret_local = if let StatementKind::Assign(l, _) = call_stmt.kind { Some(l) } else { None };
        
        let tail_bb_id = BasicBlockId(caller.basic_blocks.len());
        let tail_bb = BasicBlock {
            statements: tail_statements,
            terminator: caller.basic_blocks[bb_idx].terminator.take(),
        };
        
        // 3. Map callee blocks to new IDs in caller.
        let block_offset = caller.basic_blocks.len() + 1; // +1 because we'll add the tail later
        let mut callee_bb_map = HashMap::default();
        for (i, _) in callee.basic_blocks.iter().enumerate() {
            callee_bb_map.insert(BasicBlockId(i), BasicBlockId(block_offset + i));
        }

        // 4. Connect caller's first half to callee's entry block.
        caller.basic_blocks[bb_idx].terminator = Some(Terminator {
            kind: TerminatorKind::Goto { target: BasicBlockId(block_offset) },
            span: call_stmt.span,
        });

        // 5. Initialize callee parameters with arguments.
        // Callee locals 1..=arg_count are parameters.
        let mut init_stmts = Vec::new();
        for (j, arg) in args.iter().enumerate() {
            let param_local = Local(local_offset + j + 1);
            // Mark storage as live for the parameter.
            init_stmts.push(Statement {
                kind: StatementKind::StorageLive(param_local),
                span: call_stmt.span,
            });
            init_stmts.push(Statement {
                kind: StatementKind::Assign(param_local, Rvalue::Use(arg.clone())),
                span: call_stmt.span,
            });
        }
        
        // Also mark all other callee locals as StorageLive.
        for j in (callee.arg_count + 1)..callee.locals.len() {
            init_stmts.push(Statement {
                kind: StatementKind::StorageLive(Local(local_offset + j)),
                span: call_stmt.span,
            });
        }
        // Local 0 is the return value.
        init_stmts.push(Statement {
            kind: StatementKind::StorageLive(Local(local_offset)),
            span: call_stmt.span,
        });

        // 6. Translate callee blocks and add them to caller.
        let mut translated_blocks = Vec::new();
        for (i, bb) in callee.basic_blocks.iter().enumerate() {
            let mut new_bb = bb.clone();
            
            // Remap locals in statements.
            for stmt in &mut new_bb.statements {
                self.remap_statement(stmt, local_offset);
            }
            
            // If it's the entry block, prepend parameter initialization.
            if i == 0 {
                let mut combined = init_stmts.clone();
                combined.extend(new_bb.statements);
                new_bb.statements = combined;
            }

            // Remap terminator.
            if let Some(term) = &mut new_bb.terminator {
                match &mut term.kind {
                    TerminatorKind::Goto { target } => {
                        *target = *callee_bb_map.get(target).unwrap();
                    }
                    TerminatorKind::SwitchInt { discr, targets, otherwise } => {
                        self.remap_operand(discr, local_offset);
                        for (_, target) in targets {
                            *target = *callee_bb_map.get(target).unwrap();
                        }
                        *otherwise = *callee_bb_map.get(otherwise).unwrap();
                    }
                    TerminatorKind::Return => {
                        // Replace return with goto tail.
                        if let Some(dest) = ret_local {
                            // Assign callee's _0 (return value) to caller's destination.
                            new_bb.statements.push(Statement {
                                kind: StatementKind::Assign(dest, Rvalue::Use(Operand::Copy(Local(local_offset)))),
                                span: term.span,
                            });
                        }
                        term.kind = TerminatorKind::Goto { target: tail_bb_id };
                    }
                    _ => {}
                }
            } else {
                // If no terminator, it's an implicit return.
                new_bb.terminator = Some(Terminator {
                    kind: TerminatorKind::Goto { target: tail_bb_id },
                    span: call_stmt.span,
                });
            }
            translated_blocks.push(new_bb);
        }

        // Add the blocks.
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
            StatementKind::StorageLive(l) | StatementKind::StorageDead(l) | StatementKind::Drop(l) => {
                l.0 += offset;
            }
        }
    }

    fn remap_rvalue(&self, rval: &mut Rvalue, offset: usize) {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => {
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
