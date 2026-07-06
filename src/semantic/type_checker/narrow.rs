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
    /// None`, plain-identifier `x != <scalar literal>` / `x == <scalar
    /// literal>` against a `T | int`-style union (the sentinel-return idiom
    /// used throughout the stdlib), and `and`-chains of either; `not (...)`
    /// swaps which side of the wrapped condition each fact set describes.
    /// Everything else yields no facts (v1: no `or`, no field/index
    /// targets, no reassigned bindings mid-region -- reassignment is
    /// handled separately via `kill_narrow`).
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
                    _ => return self.narrow_facts_scalar(left, op.clone(), right),
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

    /// Handles `x != <scalar literal>` / `x == <scalar literal>` against a
    /// declared union that mixes a scalar arm (`int`/`float`/`str`/`bool`)
    /// with one or more non-scalar arms -- the `Struct | int` sentinel
    /// idiom the stdlib uses for fallible constructors (`net.connect`,
    /// `io.bufread`, `process.Command.spawn`, ...). Splits the union into
    /// the literal's scalar type and everything else, so calling a method
    /// on the narrowed branch resolves to the concrete struct instead of
    /// the union.
    fn narrow_facts_scalar(&mut self, left: &Expr, op: BinOp, right: &Expr) -> (FactSet, FactSet) {
        let (name, literal) = match (&left.kind, &right.kind) {
            (ExprKind::Identifier(n), lit) => (n, lit),
            (lit, ExprKind::Identifier(n)) => (n, lit),
            _ => return (Vec::new(), Vec::new()),
        };
        let Some(scalar) = scalar_literal_type(literal) else {
            return (Vec::new(), Vec::new());
        };
        let Some(declared) = self.lookup_type(name) else {
            return (Vec::new(), Vec::new());
        };
        let resolved = self.apply_subst(declared);
        let Type::Union(members) = &resolved else {
            return (Vec::new(), Vec::new());
        };
        if !members.contains(&scalar) {
            return (Vec::new(), Vec::new());
        }

        let matching: Vec<Type> = members.iter().filter(|m| **m == scalar).cloned().collect();
        let rest: Vec<Type> = members.iter().filter(|m| **m != scalar).cloned().collect();

        // Only the `Struct | int`-style sentinel idiom is safe to narrow
        // here: the scalar arm is `int` and every other arm is a
        // pointer-shaped type (struct/enum/list/...), matching the boxed
        // representation the None-narrowing path already relies on. A
        // union mixing two raw scalars (`int | str`) is stored unboxed and
        // narrowing its *type* alone would not change how the value's
        // bits are read, so leave those untouched.
        if scalar != Type::Int || rest.iter().any(is_scalar_type) {
            return (Vec::new(), Vec::new());
        }

        let collapse = |v: Vec<Type>| -> Option<Type> {
            match v.len() {
                0 => None,
                1 => Some(v.into_iter().next().unwrap()),
                _ => Some(Type::Union(v)),
            }
        };

        let matching_fact = collapse(matching).map(|t| vec![(name.clone(), t)]);
        let rest_fact = collapse(rest).map(|t| vec![(name.clone(), t)]);

        match op {
            BinOp::Eq => (
                matching_fact.unwrap_or_default(),
                rest_fact.unwrap_or_default(),
            ),
            BinOp::NotEq => (
                rest_fact.unwrap_or_default(),
                matching_fact.unwrap_or_default(),
            ),
            _ => unreachable!(),
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

/// True for raw, unboxed scalar types (as opposed to pointer-shaped types
/// like structs, enums, and lists).
fn is_scalar_type(t: &Type) -> bool {
    matches!(
        t,
        Type::Int
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize
            | Type::Float
            | Type::F32
            | Type::Str
            | Type::Bytes
            | Type::Bool
            | Type::Null
    )
}

/// The scalar `Type` a literal expression denotes, for matching against a
/// union member. `None` for anything that isn't a plain scalar literal.
fn scalar_literal_type(expr: &ExprKind) -> Option<Type> {
    match expr {
        ExprKind::Integer(_) => Some(Type::Int),
        ExprKind::Float(_) => Some(Type::Float),
        ExprKind::Str(_) => Some(Type::Str),
        ExprKind::Bool(_) => Some(Type::Bool),
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => scalar_literal_type(&operand.kind),
        _ => None,
    }
}

fn is_panic_call(expr: &Expr) -> bool {
    matches!(
        &expr.kind,
        ExprKind::Call { callee, .. }
            if matches!(&callee.kind, ExprKind::Identifier(n) if n == "panic")
    )
}
