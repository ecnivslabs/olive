use super::TypeChecker;
use crate::parser::{BinOp, Expr, ExprKind, Stmt, StmtKind, UnaryOp};
use crate::semantic::types::Type;

/// Narrow facts as (binding name, narrowed type) pairs.
type FactSet = Vec<(String, Type)>;

impl TypeChecker {
    /// `T | None` with `Null` removed, collapsing to the sole remaining
    /// member. `None` when nothing non-null remains (a bare `Null` type);
    /// callers decide the fallback for that case.
    pub(super) fn non_null_member(&self, ty: &Type) -> Option<Type> {
        match ty {
            Type::Union(members) => {
                let filtered: Vec<Type> = members
                    .iter()
                    .filter(|m| **m != Type::Null)
                    .cloned()
                    .collect();
                match filtered.len() {
                    0 => None,
                    1 => Some(filtered.into_iter().next().unwrap()),
                    _ => Some(Type::Union(filtered)),
                }
            }
            Type::Null => None,
            other => Some(other.clone()),
        }
    }

    /// Narrow facts a condition proves in its true branch and, separately,
    /// its false branch. Recognizes plain-identifier `x != None` / `x ==
    /// None` and `and`-chains of them; `not (...)` swaps which side of the
    /// wrapped condition each fact set describes. Everything else yields no
    /// facts (v1: no `or`, no field/index targets, no reassigned bindings
    /// mid-region -- reassignment is handled separately via `kill_narrow`).
    pub(super) fn narrow_facts(&mut self, cond: &Expr) -> (FactSet, FactSet) {
        match &cond.kind {
            ExprKind::UnaryOp {
                op: UnaryOp::Not,
                operand,
            } => {
                let (t, f) = self.narrow_facts(operand);
                (f, t)
            }
            ExprKind::BinOp {
                left,
                op: BinOp::And,
                right,
            } => {
                let (mut lt, _) = self.narrow_facts(left);
                let (rt, _) = self.narrow_facts(right);
                lt.extend(rt);
                (lt, Vec::new())
            }
            ExprKind::BinOp {
                left,
                op: op @ (BinOp::Eq | BinOp::NotEq),
                right,
            } => {
                let name = match (&left.kind, &right.kind) {
                    (ExprKind::Identifier(n), ExprKind::Null) => n,
                    (ExprKind::Null, ExprKind::Identifier(n)) => n,
                    _ => return (Vec::new(), Vec::new()),
                };
                let Some(declared) = self.lookup_type(name) else {
                    return (Vec::new(), Vec::new());
                };
                let resolved = self.apply_subst(declared);
                let Some(narrowed) = self.non_null_member(&resolved) else {
                    return (Vec::new(), Vec::new());
                };
                let fact = vec![(name.clone(), narrowed)];
                match op {
                    BinOp::NotEq => (fact, Vec::new()),
                    BinOp::Eq => (Vec::new(), fact),
                    _ => unreachable!(),
                }
            }
            _ => (Vec::new(), Vec::new()),
        }
    }

    /// Checks a block in a fresh scope pre-seeded with narrow facts (the
    /// branch form: `if x != None:` narrows the `then` body).
    pub(super) fn check_block_narrowed(&mut self, stmts: &[Stmt], facts: &[(String, Type)]) {
        self.enter_scope();
        if let Some(frame) = self.narrow_env.last_mut() {
            for (name, ty) in facts {
                frame.insert(name.clone(), ty.clone());
            }
        }
        for s in stmts {
            self.check_stmt(s);
        }
        self.leave_scope();
    }

    /// Applies narrow facts to the current scope in place (the guard form:
    /// `if x == None: return` narrows `x` for the rest of the scope).
    pub(super) fn apply_narrow_facts(&mut self, facts: &[(String, Type)]) {
        if let Some(frame) = self.narrow_env.last_mut() {
            for (name, ty) in facts {
                frame.insert(name.clone(), ty.clone());
            }
        }
    }

    /// Removes any live narrow fact for `name`. A write to a narrowed
    /// binding may reintroduce `None`, so the fact cannot survive it.
    pub(super) fn kill_narrow(&mut self, name: &str) {
        for scope in self.narrow_env.iter_mut() {
            scope.remove(name);
        }
    }

    /// True when every path through `stmts` exits via `return`/`break`/
    /// `continue`/`panic(...)`, so code after the block is unreachable.
    pub(super) fn always_diverges(&self, stmts: &[Stmt]) -> bool {
        stmts.last().is_some_and(|s| self.stmt_always_diverges(s))
    }

    fn stmt_always_diverges(&self, stmt: &Stmt) -> bool {
        match &stmt.kind {
            StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue => true,
            StmtKind::ExprStmt(e) => is_panic_call(e),
            StmtKind::UnsafeBlock(body) => self.always_diverges(body),
            StmtKind::If {
                then_body,
                elif_clauses,
                else_body,
                ..
            } => {
                let Some(else_body) = else_body else {
                    return false;
                };
                self.always_diverges(then_body)
                    && elif_clauses.iter().all(|(_, b)| self.always_diverges(b))
                    && self.always_diverges(else_body)
            }
            _ => false,
        }
    }
}

fn is_panic_call(expr: &Expr) -> bool {
    matches!(
        &expr.kind,
        ExprKind::Call { callee, .. }
            if matches!(&callee.kind, ExprKind::Identifier(n) if n == "panic")
    )
}
