use crate::mir::*;
use rustc_hash::FxHashMap as HashMap;

pub struct Inliner;

impl Inliner {
    pub fn new() -> Self {
        Self
    }

    pub fn run(&self, functions: &mut Vec<MirFunction>) {
        let fn_map: HashMap<String, MirFunction> = functions
            .iter()
            .map(|f| (f.name.clone(), f.clone()))
            .collect();

        for func in functions.iter_mut() {
            self.inline_function(func, &fn_map, 10);

            let mut changed = true;
            let mut iterations = 0;
            while changed {
                iterations += 1;
                if iterations > 100 {
                    break;
                }
                changed = false;
                changed |= self.copy_propagation(func);
                changed |= self.global_constant_propagation(func);
                changed |= self.constant_folding(func);
                changed |= self.strength_reduction(func);
                changed |= self.peephole_optimize(func);
                changed |= self.common_subexpression_elimination(func);
                changed |= self.branch_simplification(func);
                changed |= self.dead_code_elimination(func);
                changed |= self.unreachable_block_elimination(func);
            }
        }
    }

    fn global_constant_propagation(&self, func: &mut MirFunction) -> bool {
        let mut assign_counts: HashMap<Local, usize> = HashMap::default();
        let mut constant_assignments: HashMap<Local, Constant> = HashMap::default();

        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    *assign_counts.entry(*dest).or_insert(0) += 1;
                    if let Rvalue::Use(Operand::Constant(c)) = rval {
                        constant_assignments.insert(*dest, c.clone());
                    }
                }
            }
        }

        let mut safe_constants: HashMap<Local, Constant> = HashMap::default();
        for (local, count) in assign_counts {
            if count == 1 {
                if let Some(c) = constant_assignments.get(&local) {
                    safe_constants.insert(local, c.clone());
                }
            }
        }

        if safe_constants.is_empty() {
            return false;
        }

        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    changed |= self.propagate_constants_in_rvalue(rval, &safe_constants);
                } else if let StatementKind::SetIndex(obj, idx, val) = &mut stmt.kind {
                    changed |= self.propagate_constants_in_operand(obj, &safe_constants);
                    changed |= self.propagate_constants_in_operand(idx, &safe_constants);
                    changed |= self.propagate_constants_in_operand(val, &safe_constants);
                } else if let StatementKind::SetAttr(obj, _, val) = &mut stmt.kind {
                    changed |= self.propagate_constants_in_operand(obj, &safe_constants);
                    changed |= self.propagate_constants_in_operand(val, &safe_constants);
                }
            }
            if let Some(term) = &mut bb.terminator {
                if let TerminatorKind::SwitchInt { discr, .. } = &mut term.kind {
                    changed |= self.propagate_constants_in_operand(discr, &safe_constants);
                }
            }
        }
        changed
    }

    fn propagate_constants_in_rvalue(
        &self,
        rval: &mut Rvalue,
        map: &HashMap<Local, Constant>,
    ) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => {
                self.propagate_constants_in_operand(op, map)
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
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
            _ => false,
        }
    }

    fn propagate_constants_in_operand(
        &self,
        op: &mut Operand,
        map: &HashMap<Local, Constant>,
    ) -> bool {
        if let Operand::Copy(l) | Operand::Move(l) = op {
            if let Some(c) = map.get(l) {
                *op = Operand::Constant(c.clone());
                return true;
            }
        }
        false
    }

    fn constant_folding(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    if let Rvalue::BinaryOp(
                        op,
                        Operand::Constant(Constant::Int(a)),
                        Operand::Constant(Constant::Int(b)),
                    ) = rval
                    {
                        use crate::parser::BinOp::*;
                        let res = match op {
                            Add => Some(Constant::Int((*a).wrapping_add(*b))),
                            Sub => Some(Constant::Int((*a).wrapping_sub(*b))),
                            Mul => Some(Constant::Int((*a).wrapping_mul(*b))),
                            Div | FloorDiv => {
                                if *b != 0 {
                                    Some(Constant::Int(*a / *b))
                                } else {
                                    None
                                }
                            }
                            Mod => {
                                if *b != 0 {
                                    Some(Constant::Int(*a % *b))
                                } else {
                                    None
                                }
                            }
                            Eq => Some(Constant::Bool(*a == *b)),
                            NotEq => Some(Constant::Bool(*a != *b)),
                            Lt => Some(Constant::Bool(*a < *b)),
                            LtEq => Some(Constant::Bool(*a <= *b)),
                            Gt => Some(Constant::Bool(*a > *b)),
                            GtEq => Some(Constant::Bool(*a >= *b)),
                            Shl => Some(Constant::Int((*a).wrapping_shl(*b as u32))),
                            Shr => Some(Constant::Int((*a).wrapping_shr(*b as u32))),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(val));
                            changed = true;
                        }
                    } else if let Rvalue::BinaryOp(
                        op,
                        Operand::Constant(Constant::Float(a_bits)),
                        Operand::Constant(Constant::Float(b_bits)),
                    ) = rval
                    {
                        let a = f64::from_bits(*a_bits);
                        let b = f64::from_bits(*b_bits);
                        use crate::parser::BinOp::*;
                        let res = match op {
                            Add => Some(Constant::Float((a + b).to_bits())),
                            Sub => Some(Constant::Float((a - b).to_bits())),
                            Mul => Some(Constant::Float((a * b).to_bits())),
                            Div => Some(Constant::Float((a / b).to_bits())),
                            Eq => Some(Constant::Bool(a == b)),
                            NotEq => Some(Constant::Bool(a != b)),
                            Lt => Some(Constant::Bool(a < b)),
                            LtEq => Some(Constant::Bool(a <= b)),
                            Gt => Some(Constant::Bool(a > b)),
                            GtEq => Some(Constant::Bool(a >= b)),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(val));
                            changed = true;
                        }
                    } else if let Rvalue::BinaryOp(
                        op,
                        Operand::Constant(Constant::Bool(a)),
                        Operand::Constant(Constant::Bool(b)),
                    ) = rval
                    {
                        use crate::parser::BinOp::*;
                        let res = match op {
                            Eq => Some(Constant::Bool(*a == *b)),
                            NotEq => Some(Constant::Bool(*a != *b)),
                            And => Some(Constant::Bool(*a && *b)),
                            Or => Some(Constant::Bool(*a || *b)),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(val));
                            changed = true;
                        }
                    } else if let Rvalue::UnaryOp(op, Operand::Constant(c)) = rval {
                        use crate::parser::UnaryOp::*;
                        let res = match (op, c) {
                            (Neg, Constant::Int(a)) => Some(Constant::Int(-*a)),
                            (Neg, Constant::Float(a)) => {
                                Some(Constant::Float((-f64::from_bits(*a)).to_bits()))
                            }
                            (Not, Constant::Bool(a)) => Some(Constant::Bool(!*a)),
                            (Not, Constant::Int(a)) => Some(Constant::Bool(*a == 0)),
                            (Pos, Constant::Int(a)) => Some(Constant::Int(*a)),
                            (Pos, Constant::Float(a)) => Some(Constant::Float(*a)),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(val));
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
                        Rvalue::BinaryOp(Add, op, Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(Add, Operand::Constant(Constant::Int(0)), op)
                        | Rvalue::BinaryOp(Sub, op, Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(Mul, op, Operand::Constant(Constant::Int(1)))
                        | Rvalue::BinaryOp(Mul, Operand::Constant(Constant::Int(1)), op)
                        | Rvalue::BinaryOp(Div, op, Operand::Constant(Constant::Int(1)))
                        | Rvalue::BinaryOp(FloorDiv, op, Operand::Constant(Constant::Int(1))) => {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(Mul, _, op @ Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(Mul, op @ Operand::Constant(Constant::Int(0)), _) => {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(Div, l, r) | Rvalue::BinaryOp(FloorDiv, l, r)
                            if l == r =>
                        {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Int(1)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Mul, op, Operand::Constant(Constant::Int(2)))
                        | Rvalue::BinaryOp(Mul, Operand::Constant(Constant::Int(2)), op) => {
                            *rval = Rvalue::BinaryOp(Add, op.clone(), op.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(Eq, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(true)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(NotEq, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(false)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Lt, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(false)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Gt, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(false)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(LtEq, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(true)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(GtEq, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(true)));
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
            let mut available_expressions: Vec<(Rvalue, Local)> = Vec::new();
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(dest, rval) = &mut stmt.kind {
                    if matches!(
                        rval,
                        Rvalue::BinaryOp(..) | Rvalue::UnaryOp(..) | Rvalue::Use(..)
                    ) {
                        let mut found = None;
                        for (expr, local) in &available_expressions {
                            if expr == rval {
                                found = Some(*local);
                                break;
                            }
                        }

                        if let Some(existing_local) = found {
                            let new_rval = Rvalue::Use(Operand::Copy(existing_local));
                            if *rval != new_rval {
                                *rval = new_rval;
                                changed = true;
                            }
                        } else {
                            available_expressions.push((rval.clone(), *dest));
                        }
                    }

                    available_expressions.retain(|(expr, _)| !self.uses_local(expr, *dest));

                    // Any assignment might invalidate heap-based expressions if we don't track aliasing.
                    // For now, let's be conservative: if we assign a local that is used in a GetIndex,
                    // we already handle it via uses_local.
                } else if matches!(
                    stmt.kind,
                    StatementKind::SetIndex(..) | StatementKind::SetAttr(..)
                ) {
                    // Invalidate all heap-based expressions.
                    available_expressions.retain(|(expr, _)| {
                        !matches!(expr, Rvalue::GetIndex(..) | Rvalue::GetAttr(..))
                    });
                } else if let StatementKind::Assign(_, Rvalue::Call { .. }) = &stmt.kind {
                    available_expressions.clear();
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
                    TerminatorKind::SwitchInt { discr, .. } => {
                        self.record_operand_usage(discr, &mut used)
                    }
                    _ => {}
                }
            }
        }

        let mut changed = false;
        for bb in &mut func.basic_blocks {
            let old_len = bb.statements.len();
            bb.statements.retain(|stmt| {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    if matches!(rval, Rvalue::Call { .. }) {
                        return true;
                    }
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
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => {
                self.record_operand_usage(op, used)
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
                self.record_operand_usage(l, used);
                self.record_operand_usage(r, used);
            }
            Rvalue::Call { func, args } => {
                self.record_operand_usage(func, used);
                for arg in args {
                    self.record_operand_usage(arg, used);
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for op in ops {
                    self.record_operand_usage(op, used);
                }
            }
            Rvalue::Ref(l) | Rvalue::MutRef(l) => {
                used.insert(*l);
            }
        }
    }

    fn record_operand_usage(&self, op: &Operand, used: &mut std::collections::HashSet<Local>) {
        if let Operand::Copy(l) | Operand::Move(l) = op {
            used.insert(*l);
        }
    }

    /// Strength reduction: convert multiply/divide by powers of 2 to shifts.
    fn strength_reduction(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    use crate::parser::BinOp::*;
                    match rval {
                        // x * 2^n -> x << n
                        Rvalue::BinaryOp(Mul, op, Operand::Constant(Constant::Int(c)))
                        | Rvalue::BinaryOp(Mul, Operand::Constant(Constant::Int(c)), op)
                            if *c > 2 && (*c as u64).is_power_of_two() =>
                        {
                            let shift = (*c as u64).trailing_zeros() as i64;
                            let saved_op = op.clone();
                            *rval = Rvalue::BinaryOp(
                                Shl,
                                saved_op,
                                Operand::Constant(Constant::Int(shift)),
                            );
                            changed = true;
                        }
                        // x / 2^n -> x >> n (for positive divisor)
                        Rvalue::BinaryOp(Div, op, Operand::Constant(Constant::Int(c)))
                        | Rvalue::BinaryOp(FloorDiv, op, Operand::Constant(Constant::Int(c)))
                            if *c > 1 && (*c as u64).is_power_of_two() =>
                        {
                            let shift = (*c as u64).trailing_zeros() as i64;
                            let saved_op = op.clone();
                            *rval = Rvalue::BinaryOp(
                                Shr,
                                saved_op,
                                Operand::Constant(Constant::Int(shift)),
                            );
                            changed = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        changed
    }

    /// Branch simplification: fold branches on constant discriminants to gotos.
    fn branch_simplification(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            if let Some(term) = &mut bb.terminator {
                if let TerminatorKind::SwitchInt {
                    discr,
                    targets,
                    otherwise,
                } = &term.kind
                {
                    if let Operand::Constant(Constant::Bool(b)) = discr {
                        let val = if *b { 1 } else { 0 };
                        let goto_target = targets
                            .iter()
                            .find(|(v, _)| *v == val)
                            .map(|(_, t)| *t)
                            .unwrap_or(*otherwise);
                        term.kind = TerminatorKind::Goto {
                            target: goto_target,
                        };
                        changed = true;
                    } else if let Operand::Constant(Constant::Int(val)) = discr {
                        let v = *val;
                        let goto_target = targets
                            .iter()
                            .find(|(tv, _)| *tv == v)
                            .map(|(_, t)| *t)
                            .unwrap_or(*otherwise);
                        term.kind = TerminatorKind::Goto {
                            target: goto_target,
                        };
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    /// Remove unreachable blocks (no predecessors except block 0).
    fn unreachable_block_elimination(&self, func: &mut MirFunction) -> bool {
        let n = func.basic_blocks.len();
        if n <= 1 {
            return false;
        }

        let mut reachable = vec![false; n];
        reachable[0] = true;
        let mut worklist = vec![0usize];

        while let Some(idx) = worklist.pop() {
            if let Some(term) = &func.basic_blocks[idx].terminator {
                let succs: Vec<usize> = match &term.kind {
                    TerminatorKind::Goto { target } => vec![target.0],
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        let mut s: Vec<usize> = targets.iter().map(|(_, t)| t.0).collect();
                        s.push(otherwise.0);
                        s
                    }
                    _ => vec![],
                };
                for s in succs {
                    if s < n && !reachable[s] {
                        reachable[s] = true;
                        worklist.push(s);
                    }
                }
            }
        }

        let any_unreachable = reachable.iter().any(|r| !r);
        if !any_unreachable {
            return false;
        }

        // Build remapping for block indices.
        let mut remap = vec![0usize; n];
        let mut new_idx = 0;
        for i in 0..n {
            if reachable[i] {
                remap[i] = new_idx;
                new_idx += 1;
            }
        }

        // Remap terminators.
        for i in 0..n {
            if !reachable[i] {
                continue;
            }
            if let Some(term) = &mut func.basic_blocks[i].terminator {
                match &mut term.kind {
                    TerminatorKind::Goto { target } => {
                        target.0 = remap[target.0];
                    }
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        for (_, t) in targets.iter_mut() {
                            t.0 = remap[t.0];
                        }
                        otherwise.0 = remap[otherwise.0];
                    }
                    _ => {}
                }
            }
        }

        // Remove unreachable blocks.
        let mut idx = 0;
        func.basic_blocks.retain(|_| {
            let keep = reachable[idx];
            idx += 1;
            keep
        });

        true
    }

    fn inline_function(
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
                            if name == &func.name && current_depth >= 8 {
                                continue;
                            }
                            if let Some(target_fn) = fn_map.get(name) {
                                // Inline small, non-recursive functions.
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

        // 1. Copy callee locals to caller.
        for decl in &callee.locals {
            caller.locals.push(decl.clone());
        }

        // 2. Split the current block at the call site.
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

        // 3. Map callee blocks to caller.
        let block_offset = caller.basic_blocks.len() + 1; // +1 because we'll add the tail later
        let mut callee_bb_map = HashMap::default();
        for (i, _) in callee.basic_blocks.iter().enumerate() {
            callee_bb_map.insert(BasicBlockId(i), BasicBlockId(block_offset + i));
        }

        // 4. Connect caller's first half to callee's entry block.
        caller.basic_blocks[bb_idx].terminator = Some(Terminator {
            kind: TerminatorKind::Goto {
                target: BasicBlockId(block_offset),
            },
            span: call_stmt.span,
        });

        // 5. Connect parameters.
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

        // Mark locals as live.
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
                        // Replace return with goto tail.
                        if let Some(dest) = ret_local {
                            // Assign callee's _0 (return value) to caller's destination.
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
                // Implicit return.
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
            StatementKind::StorageLive(l)
            | StatementKind::StorageDead(l)
            | StatementKind::Drop(l) => {
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

    fn copy_propagation(&self, func: &mut MirFunction) -> bool {
        let mut assign_counts: HashMap<Local, usize> = HashMap::default();
        let mut copy_assignments: HashMap<Local, Local> = HashMap::default();

        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    *assign_counts.entry(*dest).or_insert(0) += 1;
                    if let Rvalue::Use(Operand::Copy(src) | Operand::Move(src)) = rval {
                        copy_assignments.insert(*dest, *src);
                    }
                }
            }
        }

        let mut safe_copies: HashMap<Local, Local> = HashMap::default();
        for (dest, src) in copy_assignments {
            if assign_counts.get(&dest) == Some(&1) {
                if *assign_counts.get(&src).unwrap_or(&0) <= 1 || src.0 <= func.arg_count {
                    safe_copies.insert(dest, src);
                }
            }
        }

        if safe_copies.is_empty() {
            return false;
        }

        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                match &mut stmt.kind {
                    StatementKind::Assign(_, rval) => {
                        changed |= self.propagate_copies_in_rvalue(rval, &safe_copies);
                    }
                    StatementKind::SetIndex(obj, idx, val) => {
                        changed |= self.propagate_copies_in_operand(obj, &safe_copies);
                        changed |= self.propagate_copies_in_operand(idx, &safe_copies);
                        changed |= self.propagate_copies_in_operand(val, &safe_copies);
                    }
                    StatementKind::SetAttr(obj, _, val) => {
                        changed |= self.propagate_copies_in_operand(obj, &safe_copies);
                        changed |= self.propagate_copies_in_operand(val, &safe_copies);
                    }
                    _ => {}
                }
            }
            if let Some(term) = &mut bb.terminator {
                match &mut term.kind {
                    TerminatorKind::SwitchInt { discr, .. } => {
                        changed |= self.propagate_copies_in_operand(discr, &safe_copies);
                    }
                    _ => {}
                }
            }
        }
        changed
    }

    fn propagate_copies_in_rvalue(&self, rval: &mut Rvalue, map: &HashMap<Local, Local>) -> bool {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => {
                self.propagate_copies_in_operand(op, map)
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
                let mut changed = self.propagate_copies_in_operand(l, map);
                changed |= self.propagate_copies_in_operand(r, map);
                changed
            }
            Rvalue::Call { func, args } => {
                let mut changed = self.propagate_copies_in_operand(func, map);
                for arg in args {
                    changed |= self.propagate_copies_in_operand(arg, map);
                }
                changed
            }
            Rvalue::Aggregate(_, ops) => {
                let mut changed = false;
                for op in ops {
                    changed |= self.propagate_copies_in_operand(op, map);
                }
                changed
            }
            _ => false,
        }
    }

    fn propagate_copies_in_operand(&self, op: &mut Operand, map: &HashMap<Local, Local>) -> bool {
        if let Operand::Copy(l) | Operand::Move(l) = op {
            if let Some(new_l) = map.get(l) {
                let old_kind_is_move = matches!(op, Operand::Move(_));
                if old_kind_is_move {
                    *op = Operand::Move(*new_l);
                } else {
                    *op = Operand::Copy(*new_l);
                }
                return true;
            }
        }
        false
    }
}
