use super::Transform;
use crate::mir::*;
use crate::parser::ast::UnaryOp;
use crate::semantic::types::Type as OliveType;

/// Whether an operand is statically an integer-family scalar (constants count:
/// these patterns only ever carry `Constant::Int`). Identities that hold over
/// the integers do not hold over floats -- NaN breaks every reflexive
/// comparison -- nor over Python objects, whose operators dispatch to user
/// overloads (`v + 0` must keep calling `__add__`); those rewrites are gated
/// on this. `<`/`>` self-comparisons are IEEE-exact for floats and stay.
fn is_int_like(local_types: &[OliveType], op: &Operand) -> bool {
    match op {
        Operand::Constant(_) => true,
        Operand::Copy(l) | Operand::Move(l) => matches!(
            local_types.get(l.0),
            Some(OliveType::Int)
                | Some(OliveType::I8)
                | Some(OliveType::I16)
                | Some(OliveType::I32)
                | Some(OliveType::U8)
                | Some(OliveType::U16)
                | Some(OliveType::U32)
                | Some(OliveType::U64)
                | Some(OliveType::Usize)
                | Some(OliveType::Bool)
        ),
    }
}

fn is_float_like(local_types: &[OliveType], op: &Operand) -> bool {
    match op {
        Operand::Constant(_) => true,
        Operand::Copy(l) | Operand::Move(l) => matches!(
            local_types.get(l.0),
            Some(OliveType::Float) | Some(OliveType::F32)
        ),
    }
}

pub struct PeepholeOptimize;

impl PeepholeOptimize {
    fn eliminate_double_not(bb: &mut BasicBlock) -> bool {
        use rustc_hash::FxHashMap;
        let mut not_defs: FxHashMap<Local, Operand> = FxHashMap::default();
        let mut changed = false;
        for stmt in &mut bb.statements {
            match &mut stmt.kind {
                StatementKind::Assign(out, Rvalue::UnaryOp(UnaryOp::Not, inner)) => {
                    let inner_local = match inner {
                        Operand::Copy(l) | Operand::Move(l) => *l,
                        _ => {
                            let _ = not_defs;
                            return changed;
                        }
                    };
                    if let Some(src) = not_defs.get(&inner_local).cloned() {
                        *stmt = Statement {
                            kind: StatementKind::Assign(*out, Rvalue::Use(src)),
                            span: stmt.span,
                        };
                        changed = true;
                    } else {
                        not_defs.insert(*out, inner.clone());
                    }
                }
                StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {}
                _ => {
                    not_defs.clear();
                }
            }
        }
        changed
    }
}

impl Transform for PeepholeOptimize {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            changed |= Self::eliminate_double_not(bb);
        }
        let local_types: Vec<OliveType> = func.locals.iter().map(|l| l.ty.clone()).collect();
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
                            if is_int_like(&local_types, op) =>
                        {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(Mul, other, op @ Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(Mul, op @ Operand::Constant(Constant::Int(0)), other)
                            if is_int_like(&local_types, other) =>
                        {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        }
                        // `x / x` faults when `x == 0`; folding it to 1 silently
                        // deletes a runtime fault, so it is never rewritten.
                        Rvalue::BinaryOp(Eq, l, r) if l == r && is_int_like(&local_types, l) => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(true)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(NotEq, l, r) if l == r && is_int_like(&local_types, l) => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(false)));
                            changed = true;
                        }
                        // `<` and `>` are reflexively false over floats too
                        // (NaN < NaN is false), unlike the inclusive forms.
                        Rvalue::BinaryOp(Lt, l, r)
                            if l == r
                                && (is_int_like(&local_types, l)
                                    || is_float_like(&local_types, l)) =>
                        {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(false)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Gt, l, r)
                            if l == r
                                && (is_int_like(&local_types, l)
                                    || is_float_like(&local_types, l)) =>
                        {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(false)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(LtEq, l, r) if l == r && is_int_like(&local_types, l) => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(true)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(GtEq, l, r) if l == r && is_int_like(&local_types, l) => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(true)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Sub, l, r) if l == r && is_int_like(&local_types, l) => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Int(0)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Shl, op, Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(Shr, op, Operand::Constant(Constant::Int(0)))
                            if is_int_like(&local_types, op) =>
                        {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(And, other, Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(And, Operand::Constant(Constant::Int(0)), other)
                            if is_int_like(&local_types, other) =>
                        {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Int(0)));
                            changed = true;
                        }
                        Rvalue::BinaryOp(Or, op, Operand::Constant(Constant::Int(0)))
                        | Rvalue::BinaryOp(Or, Operand::Constant(Constant::Int(0)), op)
                            if is_int_like(&local_types, op) =>
                        {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(And, l, r) if l == r && is_int_like(&local_types, l) => {
                            *rval = Rvalue::Use(l.clone());
                            changed = true;
                        }
                        Rvalue::BinaryOp(Or, l, r) if l == r && is_int_like(&local_types, l) => {
                            *rval = Rvalue::Use(l.clone());
                            changed = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::BinOp;

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

    fn func(locals: Vec<OliveType>, stmts: Vec<Statement>) -> MirFunction {
        MirFunction {
            name: "f".into(),
            locals: locals
                .into_iter()
                .map(|ty| LocalDecl {
                    ty,
                    name: None,
                    span: sp(),
                    is_mut: false,
                    is_owning: false,
                })
                .collect(),
            basic_blocks: vec![BasicBlock {
                statements: stmts,
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

    fn locals_of(types: OliveType) -> Vec<OliveType> {
        vec![types.clone(), types]
    }

    #[test]
    fn add_zero() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    BinOp::Add,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(0)),
                ),
            )],
        );
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(_))
        ));
    }

    #[test]
    fn mul_one() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    BinOp::Mul,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(1)),
                ),
            )],
        );
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(_))
        ));
    }

    #[test]
    fn mul_zero() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    BinOp::Mul,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(0)),
                ),
            )],
        );
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Int(0))))
        ));
    }

    #[test]
    fn div_one() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    BinOp::Div,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(1)),
                ),
            )],
        );
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(_))
        ));
    }

    #[test]
    fn sub_self() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(BinOp::Sub, Operand::Copy(Local(1)), Operand::Copy(Local(1))),
            )],
        );
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Int(0))))
        ));
    }

    #[test]
    fn eq_self() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(BinOp::Eq, Operand::Copy(Local(1)), Operand::Copy(Local(1))),
            )],
        );
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Bool(true))))
        ));
    }

    #[test]
    fn neq_self() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    BinOp::NotEq,
                    Operand::Copy(Local(1)),
                    Operand::Copy(Local(1)),
                ),
            )],
        );
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Bool(false))))
        ));
    }

    /// `x / x` faults for `x == 0` (JIT emits unconditional div-zero checks);
    /// folding it to 1 would delete that fault.
    #[test]
    fn div_self_preserved() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(BinOp::Div, Operand::Copy(Local(1)), Operand::Copy(Local(1))),
            )],
        );
        assert!(!PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::BinaryOp(BinOp::Div, _, _))
        ));
    }

    #[test]
    fn and_zero() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    BinOp::And,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(0)),
                ),
            )],
        );
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Int(0))))
        ));
    }

    #[test]
    fn or_zero() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    BinOp::Or,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(0)),
                ),
            )],
        );
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Copy(Local(1))))
        ));
    }

    #[test]
    fn no_change_for_non_pattern() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(BinOp::Add, Operand::Copy(Local(1)), Operand::Copy(Local(2))),
            )],
        );
        assert!(!PeepholeOptimize.run(&mut f));
    }

    #[test]
    fn shl_zero() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    BinOp::Shl,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(0)),
                ),
            )],
        );
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(_))
        ));
    }

    #[test]
    fn shr_zero() {
        let mut f = func(
            locals_of(OliveType::Int),
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    BinOp::Shr,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(0)),
                ),
            )],
        );
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(_))
        ));
    }

    /// NaN - NaN is NaN, not 0.
    #[test]
    fn float_sub_self_preserved() {
        let mut f = func(
            locals_of(OliveType::Float),
            vec![assign(
                0,
                Rvalue::BinaryOp(BinOp::Sub, Operand::Copy(Local(1)), Operand::Copy(Local(1))),
            )],
        );
        assert!(!PeepholeOptimize.run(&mut f));
    }

    /// NaN == NaN is false.
    #[test]
    fn float_eq_self_preserved() {
        let mut f = func(
            locals_of(OliveType::Float),
            vec![assign(
                0,
                Rvalue::BinaryOp(BinOp::Eq, Operand::Copy(Local(1)), Operand::Copy(Local(1))),
            )],
        );
        assert!(!PeepholeOptimize.run(&mut f));
    }

    /// NaN <= NaN is false, so the reflexive-true fold must not fire.
    #[test]
    fn float_lte_self_preserved() {
        let mut f = func(
            locals_of(OliveType::Float),
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    BinOp::LtEq,
                    Operand::Copy(Local(1)),
                    Operand::Copy(Local(1)),
                ),
            )],
        );
        assert!(!PeepholeOptimize.run(&mut f));
    }

    /// NaN < NaN is false, matching the fold -- this one stays.
    #[test]
    fn float_lt_self_folded() {
        let mut f = func(
            locals_of(OliveType::Float),
            vec![assign(
                0,
                Rvalue::BinaryOp(BinOp::Lt, Operand::Copy(Local(1)), Operand::Copy(Local(1))),
            )],
        );
        assert!(PeepholeOptimize.run(&mut f));
        assert!(matches!(
            f.basic_blocks[0].statements[0].kind,
            StatementKind::Assign(_, Rvalue::Use(Operand::Constant(Constant::Bool(false))))
        ));
    }

    /// `v + 0` on a PyObject dispatches to `__add__`; the call must survive.
    #[test]
    fn pyobj_add_zero_preserved() {
        let mut f = func(
            locals_of(OliveType::PyObject),
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    BinOp::Add,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(0)),
                ),
            )],
        );
        assert!(!PeepholeOptimize.run(&mut f));
    }

    /// `v * 0` on a PyObject dispatches to `__mul__`; replacing it with the
    /// integer 0 would also type-confuse the destination slot.
    #[test]
    fn pyobj_mul_zero_preserved() {
        let mut f = func(
            locals_of(OliveType::PyObject),
            vec![assign(
                0,
                Rvalue::BinaryOp(
                    BinOp::Mul,
                    Operand::Copy(Local(1)),
                    Operand::Constant(Constant::Int(0)),
                ),
            )],
        );
        assert!(!PeepholeOptimize.run(&mut f));
    }

    #[test]
    fn unknown_type_operand_preserved() {
        let mut f = func(
            locals_of(OliveType::Any),
            vec![assign(
                0,
                Rvalue::BinaryOp(BinOp::Sub, Operand::Copy(Local(1)), Operand::Copy(Local(1))),
            )],
        );
        assert!(!PeepholeOptimize.run(&mut f));
    }
}
