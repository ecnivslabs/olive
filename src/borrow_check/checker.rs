use crate::mir::*;
use crate::semantic::SemanticError;
use crate::span::Span;
use rustc_hash::FxHashMap as HashMap;
use rustc_hash::FxHashSet as HashSet;
use std::collections::VecDeque;

// Tracking state for each local variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalState {
    /// The local has been assigned a value and is usable.
    Initialized,
    /// The local has been moved and is no longer usable.
    Moved,
    /// The local's storage is not yet live.
    Dead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FlowState {
    locals: Vec<LocalState>,
    /// local -> (immutable_count, is_mutably_borrowed)
    borrows: Vec<(usize, bool)>,
}

impl FlowState {
    fn new(num_locals: usize) -> Self {
        Self {
            locals: vec![LocalState::Dead; num_locals],
            borrows: vec![(0, false); num_locals],
        }
    }

    fn get(&self, local: Local) -> LocalState {
        self.locals
            .get(local.0)
            .copied()
            .unwrap_or(LocalState::Dead)
    }

    fn set(&mut self, local: Local, state: LocalState) -> Result<(), String> {
        if local.0 < self.locals.len() {
            let current_borrows = self.borrows[local.0];
            if current_borrows.0 > 0 || current_borrows.1 {
                if state != LocalState::Initialized {
                    return Err("cannot move or drop variable while it is borrowed".to_string());
                } else {
                    return Err("cannot reassign variable while it is borrowed".to_string());
                }
            }
            self.locals[local.0] = state;
            // When state changes to Dead or Moved, borrows are cleared.
            if state != LocalState::Initialized {
                self.borrows[local.0] = (0, false);
            }
        }
        Ok(())
    }

    // Merge states at CFG join points. Returns true if anything changed.
    fn join(&mut self, other: &FlowState) -> bool {
        let mut changed = false;
        let len = self.locals.len().max(other.locals.len());
        if self.locals.len() < len {
            self.locals.resize(len, LocalState::Dead);
            self.borrows.resize(len, (0, false));
        }
        for (i, other_state) in other.locals.iter().enumerate() {
            let merged = merge_states(self.locals[i], *other_state);
            if merged != self.locals[i] {
                self.locals[i] = merged;
                changed = true;
            }
        }
        for (b1, b2) in self.borrows.iter_mut().zip(other.borrows.iter()) {
            let new_count = b1.0.max(b2.0);
            let new_mut = b1.1 || b2.1;
            if b1.0 != new_count || b1.1 != new_mut {
                *b1 = (new_count, new_mut);
                changed = true;
            }
        }
        changed
    }
}

// If either path says Moved, the result is Moved. If one is Dead and other Initialized, result is Dead.
fn merge_states(a: LocalState, b: LocalState) -> LocalState {
    match (a, b) {
        (LocalState::Moved, _) | (_, LocalState::Moved) => LocalState::Moved,
        (LocalState::Initialized, LocalState::Initialized) => LocalState::Initialized,
        _ => LocalState::Dead,
    }
}

pub struct BorrowChecker<'a> {
    pub func: &'a MirFunction,
    pub errors: Vec<SemanticError>,
    pub liveness: Liveness,
    /// Tracks which reference points to which local (e.g. let r = &a => provenance[r] = a)
    pub provenance: HashMap<Local, Local>,
}

impl<'a> BorrowChecker<'a> {
    pub fn new(func: &'a MirFunction) -> Self {
        let liveness = Liveness::compute(func);
        Self {
            func,
            errors: Vec::new(),
            liveness,
            provenance: HashMap::default(),
        }
    }

    pub fn check(&mut self) {
        if self.func.basic_blocks.is_empty() {
            return;
        }

        let num_blocks = self.func.basic_blocks.len();
        let num_locals = self.func.locals.len();

        // entry states. only bb0 starts initialized.
        let mut entry_states: Vec<Option<FlowState>> = vec![None; num_blocks];

        let mut init_state = FlowState::new(num_locals);
        // Function parameters are initialized by the caller.
        for i in 1..=self.func.arg_count {
            if i < num_locals {
                let _ = init_state.set(Local(i), LocalState::Initialized);
            }
        }
        entry_states[0] = Some(init_state);

        // Fixed-point iteration.
        let mut worklist: VecDeque<usize> = VecDeque::new();
        worklist.push_back(0);
        let mut in_worklist: Vec<bool> = vec![false; num_blocks];
        in_worklist[0] = true;

        while let Some(bb_idx) = worklist.pop_front() {
            in_worklist[bb_idx] = false;

            let state = match &entry_states[bb_idx] {
                Some(s) => s.clone(),
                None => continue, // Unreachable block.
            };

            let mut state = state;
            let bb = &self.func.basic_blocks[bb_idx];

            for (stmt_idx, stmt) in bb.statements.iter().enumerate() {
                self.transfer_stmt(stmt, &mut state);

                // NLL: Release borrows for references that are no longer live.
                let live_after = &self.liveness.live_after[bb_idx][stmt_idx];
                self.release_dead_borrows(&mut state, live_after);
            }

            let successors = self.successors(bb);
            let term_idx = bb.statements.len();
            if let Some(term) = &bb.terminator {
                self.check_terminator(term, &mut state);
            }

            let live_after_term = &self.liveness.live_after[bb_idx][term_idx];
            self.release_dead_borrows(&mut state, live_after_term);

            for succ in successors {
                let changed = match &mut entry_states[succ] {
                    None => {
                        entry_states[succ] = Some(state.clone());
                        true
                    }
                    Some(existing) => existing.join(&state),
                };
                if changed && !in_worklist[succ] {
                    worklist.push_back(succ);
                    in_worklist[succ] = true;
                }
            }
        }
    }

    fn transfer_stmt(&mut self, stmt: &Statement, state: &mut FlowState) {
        match &stmt.kind {
            StatementKind::Assign(lhs, rvalue) => {
                self.check_rvalue(rvalue, state, stmt.span);

                // Track provenance for references
                match rvalue {
                    Rvalue::Ref(rhs) | Rvalue::MutRef(rhs) => {
                        self.provenance.insert(*lhs, *rhs);
                    }
                    Rvalue::Use(Operand::Copy(rhs)) | Rvalue::Use(Operand::Move(rhs)) => {
                        // If we assign a reference to another variable, it inherits provenance
                        if let Some(prov) = self.provenance.get(rhs).cloned() {
                            self.provenance.insert(*lhs, prov);
                        }
                    }
                    _ => {
                        self.provenance.remove(lhs);
                    }
                }

                if let Err(msg) = state.set(*lhs, LocalState::Initialized) {
                    let name = self.local_name(*lhs);
                    self.errors.push(SemanticError::Custom {
                        msg: format!("{} `{}`", msg, name),
                        span: stmt.span,
                    });
                }
            }
            StatementKind::SetAttr(obj, _, val) => {
                self.check_mutation(obj, state, stmt.span);
                self.check_operand(obj, state, stmt.span);
                self.check_operand(val, state, stmt.span);
            }
            StatementKind::SetIndex(obj, idx, val) => {
                self.check_mutation(obj, state, stmt.span);
                self.check_operand(obj, state, stmt.span);
                self.check_operand(idx, state, stmt.span);
                self.check_operand(val, state, stmt.span);
            }
            StatementKind::StorageLive(local) => {
                let _ = state.set(*local, LocalState::Initialized);
            }
            StatementKind::StorageDead(local) => {
                let _ = state.set(*local, LocalState::Dead);
            }
            StatementKind::Drop(local) => {
                // Drop only happens if the variable is still initialized.
                // If it's already moved, drop is a no-op at runtime.
                // But for the borrow checker, we just ensure it was initialized at some point.
                if state.get(*local) == LocalState::Initialized
                    && let Err(msg) = state.set(*local, LocalState::Moved)
                {
                    let name = self.local_name(*local);
                    self.errors.push(SemanticError::Custom {
                        msg: format!("{} `{}` (lifetime error)", msg, name),
                        span: stmt.span,
                    });
                }
            }
        }
    }

    fn check_terminator(&mut self, term: &Terminator, state: &mut FlowState) {
        match &term.kind {
            TerminatorKind::SwitchInt { discr, .. } => {
                self.check_operand(discr, state, term.span);
            }
            TerminatorKind::Return | TerminatorKind::Goto { .. } | TerminatorKind::Unreachable => {}
        }
    }

    fn successors(&self, bb: &BasicBlock) -> Vec<usize> {
        match &bb.terminator {
            Some(t) => match &t.kind {
                TerminatorKind::Goto { target } => vec![target.0],
                TerminatorKind::SwitchInt {
                    targets, otherwise, ..
                } => {
                    let mut succs: Vec<usize> = targets.iter().map(|(_, bb)| bb.0).collect();
                    succs.push(otherwise.0);
                    succs
                }
                TerminatorKind::Return | TerminatorKind::Unreachable => vec![],
            },
            None => vec![],
        }
    }

    fn check_rvalue(&mut self, rvalue: &Rvalue, state: &mut FlowState, span: Span) {
        match rvalue {
            Rvalue::Use(op) => self.check_operand(op, state, span),
            Rvalue::BinaryOp(_, lhs, rhs) => {
                self.check_operand(lhs, state, span);
                self.check_operand(rhs, state, span);
            }
            Rvalue::UnaryOp(_, op) => self.check_operand(op, state, span),
            Rvalue::Call { func, args } => {
                self.check_operand(func, state, span);
                for arg in args {
                    self.check_operand(arg, state, span);
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for op in ops {
                    self.check_operand(op, state, span);
                }
            }
            Rvalue::GetAttr(op, _) => self.check_operand(op, state, span),
            Rvalue::GetIndex(obj, idx) => {
                self.check_operand(obj, state, span);
                self.check_operand(idx, state, span);
            }
            Rvalue::Ref(local) => {
                let s = state.get(*local);
                if s != LocalState::Initialized {
                    let name = self.local_name(*local);
                    self.errors.push(SemanticError::Custom {
                        msg: format!("cannot borrow uninitialized or moved variable `{}`", name),
                        span,
                    });
                }
                let borrow = &mut state.borrows[local.0];
                if borrow.1 {
                    let name = self.local_name(*local);
                    self.errors.push(SemanticError::Custom {
                        msg: format!("cannot borrow `{}` as immutable because it is also borrowed as mutable", name),
                        span,
                    });
                }
                borrow.0 += 1;
            }
            Rvalue::MutRef(local) => {
                let s = state.get(*local);
                if s != LocalState::Initialized {
                    let name = self.local_name(*local);
                    self.errors.push(SemanticError::Custom {
                        msg: format!(
                            "cannot mutably borrow uninitialized or moved variable `{}`",
                            name
                        ),
                        span,
                    });
                }
                let decl = &self.func.locals[local.0];
                if !decl.is_mut {
                    let name = self.local_name(*local);
                    self.errors.push(SemanticError::Custom {
                        msg: format!("cannot mutably borrow immutable variable `{}`", name),
                        span,
                    });
                }
                let borrow = &mut state.borrows[local.0];
                if borrow.0 > 0 || borrow.1 {
                    let name = self.local_name(*local);
                    self.errors.push(SemanticError::Custom {
                        msg: format!(
                            "cannot borrow `{}` as mutable because it is already borrowed",
                            name
                        ),
                        span,
                    });
                }
                borrow.1 = true;
            }
        }
    }

    fn check_mutation(&mut self, op: &Operand, state: &FlowState, span: Span) {
        if let Operand::Copy(local) | Operand::Move(local) = op {
            let borrow = state.borrows[local.0];
            if borrow.0 > 0 {
                let name = self.local_name(*local);
                self.errors.push(SemanticError::Custom {
                    msg: format!("cannot mutate `{}` because it is borrowed", name),
                    span,
                });
            }
        }
    }

    fn check_operand(&mut self, op: &Operand, state: &mut FlowState, span: Span) {
        match op {
            Operand::Copy(local) | Operand::Move(local) => match state.get(*local) {
                LocalState::Dead => {
                    let name = self.local_name(*local);
                    self.errors.push(SemanticError::Custom {
                        msg: format!("use of possibly uninitialized variable `{}`", name),
                        span,
                    });
                }
                LocalState::Moved => {
                    let name = self.local_name(*local);
                    self.errors.push(SemanticError::Custom {
                        msg: format!("use of moved variable `{}`", name),
                        span,
                    });
                }
                LocalState::Initialized => {
                    if let Operand::Move(_) = op {
                        let _ = state.set(*local, LocalState::Moved);
                    }
                }
            },
            Operand::Constant(_) => {}
        }
    }

    fn release_dead_borrows(&self, state: &mut FlowState, live_locals: &HashSet<Local>) {
        // We only release the borrow if ALL locals that hold this reference are dead.
        let mut still_borrowed = HashSet::default();
        for (ref_var, &pointed_var) in &self.provenance {
            if live_locals.contains(ref_var) {
                still_borrowed.insert(pointed_var);
            }
        }

        for (ref_var, &pointed_var) in &self.provenance {
            if !live_locals.contains(ref_var) && !still_borrowed.contains(&pointed_var) {
                // This specific reference is dead, and NO OTHER live local points here.
                let borrow = &mut state.borrows[pointed_var.0];
                if borrow.0 > 0 {
                    borrow.0 -= 1;
                } else {
                    borrow.1 = false;
                }
            }
        }
    }

    // Helper to get a readable name for a local (for errors).
    fn local_name(&self, local: Local) -> String {
        self.func
            .locals
            .get(local.0)
            .and_then(|decl| decl.name.as_ref())
            .cloned()
            .unwrap_or_else(|| format!("_{}", local.0))
    }
}
