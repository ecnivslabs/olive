use crate::parser::{BinOp, UnaryOp};
use crate::semantic::types::Type;
use crate::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Local(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BasicBlockId(pub usize);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Operand {
    Copy(Local),
    Move(Local),
    Constant(Constant),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Constant {
    Int(i64),
    Float(u64),
    Str(String),
    Bool(bool),
    Function(String),
    GlobalData(String),
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AggregateKind {
    Tuple,
    List,
    Set,
    Dict,
    EnumVariant(i64, usize),
    FatPtr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Rvalue {
    Use(Operand),
    BinaryOp(BinOp, Operand, Operand),
    UnaryOp(UnaryOp, Operand),
    Call {
        func: Operand,
        args: Vec<Operand>,
    },
    Aggregate(AggregateKind, Vec<Operand>),
    Cast(Operand, Type),
    GetAttr(Operand, String),
    /// Indexed read `obj[idx]`. The `bool` is `unchecked`: when set, the
    /// bounds-check elimination pass has proven `idx` is always in range, so
    /// codegen omits the per-access bounds check. Lowering always emits `false`.
    GetIndex(Operand, Operand, bool),
    GetTag(Operand),
    GetTypeId(Operand),
    Ref(Local),
    MutRef(Local),
    VectorSplat(Operand, usize),
    VectorLoad(Operand, Operand, usize),
    VectorFMA(Operand, Operand, Operand),
    /// Horizontal fold of a vector's lanes with an associative int op.
    VectorReduce(BinOp, Operand, usize),
    PtrLoad(Operand),
    FatPtrData(Operand),
    VTableLoad {
        vtable: Operand,
        method_idx: usize,
    },
    /// Generation word of a slab object, captured when a borrow is created.
    /// Emitted only by gen-check insertion, after every other pass has run.
    GenOf(Operand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementKind {
    Assign(Local, Rvalue),
    SetAttr(Operand, String, Operand),
    /// Indexed write `obj[idx] = val`. The trailing `bool` is `unchecked`, with
    /// the same meaning as on [`Rvalue::GetIndex`]: set only by bounds-check
    /// elimination once `idx` is proven in range. Lowering always emits `false`.
    SetIndex(Operand, Operand, Operand, bool),
    StorageLive(Local),
    StorageDead(Local),
    Drop(Local),
    VectorStore(Operand, Operand, Operand),
    PtrStore(Operand, Operand),
    /// Panics with a stale-reference fault unless `value` is null or its
    /// current generation still matches `generation`. Emitted only by
    /// gen-check insertion, after every other pass has run.
    GenCheck {
        value: Local,
        generation: Local,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminatorKind {
    Goto {
        target: BasicBlockId,
    },
    SwitchInt {
        discr: Operand,
        targets: Vec<(i64, BasicBlockId)>,
        otherwise: BasicBlockId,
    },
    Return,
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminator {
    pub kind: TerminatorKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub statements: Vec<Statement>,
    pub terminator: Option<Terminator>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDecl {
    pub ty: Type,
    pub name: Option<String>,
    pub span: Span,
    pub is_mut: bool,
    pub is_owning: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirFunction {
    pub name: String,
    pub locals: Vec<LocalDecl>,
    pub basic_blocks: Vec<BasicBlock>,
    pub arg_count: usize,
    pub vararg_idx: Option<usize>,
    pub kwarg_idx: Option<usize>,
    pub param_names: Vec<String>,
    pub is_async: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::BinOp;

    #[test]
    fn local_equality() {
        assert_eq!(Local(0), Local(0));
        assert_ne!(Local(0), Local(1));
    }

    #[test]
    fn local_hash() {
        use std::collections::HashSet;
        let mut s = HashSet::new();
        s.insert(Local(0));
        s.insert(Local(1));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn bb_id_equality() {
        assert_eq!(BasicBlockId(0), BasicBlockId(0));
        assert_ne!(BasicBlockId(1), BasicBlockId(2));
    }

    #[test]
    fn constant_int() {
        let c = Constant::Int(42);
        assert_eq!(c, Constant::Int(42));
        assert_ne!(c, Constant::Int(0));
    }

    #[test]
    fn constant_bool() {
        assert_eq!(Constant::Bool(true), Constant::Bool(true));
        assert_ne!(Constant::Bool(true), Constant::Bool(false));
    }

    #[test]
    fn constant_none() {
        assert_eq!(Constant::None, Constant::None);
    }

    #[test]
    fn operand_copy() {
        let o = Operand::Copy(Local(0));
        assert!(matches!(o, Operand::Copy(Local(0))));
    }

    #[test]
    fn operand_move() {
        let o = Operand::Move(Local(1));
        assert!(matches!(o, Operand::Move(Local(1))));
    }

    #[test]
    fn operand_constant() {
        let o = Operand::Constant(Constant::Int(5));
        assert!(matches!(o, Operand::Constant(Constant::Int(5))));
    }

    #[test]
    fn aggregate_kinds() {
        assert_eq!(AggregateKind::Tuple, AggregateKind::Tuple);
        assert_eq!(AggregateKind::List, AggregateKind::List);
        assert_eq!(
            AggregateKind::EnumVariant(1, 2),
            AggregateKind::EnumVariant(1, 2)
        );
        assert_ne!(
            AggregateKind::EnumVariant(1, 2),
            AggregateKind::EnumVariant(1, 3)
        );
    }

    #[test]
    fn rvalue_use() {
        let r = Rvalue::Use(Operand::Copy(Local(0)));
        assert!(matches!(r, Rvalue::Use(Operand::Copy(Local(0)))));
    }

    #[test]
    fn rvalue_binary_op() {
        let r = Rvalue::BinaryOp(BinOp::Add, Operand::Copy(Local(0)), Operand::Copy(Local(1)));
        let op = match r {
            Rvalue::BinaryOp(o, _, _) => o,
            _ => unreachable!(),
        };
        assert_eq!(op, BinOp::Add);
    }

    #[test]
    fn rvalue_call() {
        let r = Rvalue::Call {
            func: Operand::Constant(Constant::Function("f".into())),
            args: vec![],
        };
        let f = match r {
            Rvalue::Call { func, .. } => func,
            _ => unreachable!(),
        };
        assert_eq!(f, Operand::Constant(Constant::Function("f".into())));
    }

    #[test]
    fn rvalue_cast() {
        let r = Rvalue::Cast(Operand::Copy(Local(0)), Type::Int);
        assert!(matches!(r, Rvalue::Cast(_, Type::Int)));
    }

    #[test]
    fn rvalue_ref() {
        let r = Rvalue::Ref(Local(0));
        assert!(matches!(r, Rvalue::Ref(Local(0))));
    }

    #[test]
    fn rvalue_get_attr() {
        let r = Rvalue::GetAttr(Operand::Copy(Local(0)), "x".into());
        let name = match r {
            Rvalue::GetAttr(_, n) => n,
            _ => unreachable!(),
        };
        assert_eq!(name, "x");
    }

    #[test]
    fn rvalue_get_index() {
        let r = Rvalue::GetIndex(Operand::Copy(Local(0)), Operand::Copy(Local(1)), false);
        assert!(matches!(
            r,
            Rvalue::GetIndex(Operand::Copy(Local(0)), Operand::Copy(Local(1)), false)
        ));
    }

    #[test]
    fn rvalue_aggregate() {
        let r = Rvalue::Aggregate(AggregateKind::List, vec![Operand::Copy(Local(0))]);
        let (k, _) = match r {
            Rvalue::Aggregate(k, a) => (k, a),
            _ => unreachable!(),
        };
        assert_eq!(k, AggregateKind::List);
    }

    #[test]
    fn statement_assign() {
        let s = Statement {
            kind: StatementKind::Assign(Local(0), Rvalue::Use(Operand::Copy(Local(1)))),
            span: Span {
                file_id: 0,
                line: 1,
                col: 1,
                start: 0,
                end: 0,
            },
        };
        assert!(matches!(s.kind, StatementKind::Assign(Local(0), _)));
    }

    #[test]
    fn statement_storage() {
        assert!(matches!(
            StatementKind::StorageLive(Local(0)),
            StatementKind::StorageLive(_)
        ));
        assert!(matches!(
            StatementKind::StorageDead(Local(1)),
            StatementKind::StorageDead(_)
        ));
    }

    #[test]
    fn statement_set_attr() {
        let s =
            StatementKind::SetAttr(Operand::Copy(Local(0)), "f".into(), Operand::Copy(Local(1)));
        let name = match s {
            StatementKind::SetAttr(_, n, _) => n,
            _ => unreachable!(),
        };
        assert_eq!(name, "f");
    }

    #[test]
    fn statement_drop() {
        assert!(matches!(
            StatementKind::Drop(Local(0)),
            StatementKind::Drop(_)
        ));
    }

    #[test]
    fn terminator_goto() {
        let t = Terminator {
            kind: TerminatorKind::Goto {
                target: BasicBlockId(1),
            },
            span: Span {
                file_id: 0,
                line: 0,
                col: 0,
                start: 1,
                end: 1,
            },
        };
        let target = match t.kind {
            TerminatorKind::Goto { target } => target,
            _ => unreachable!(),
        };
        assert_eq!(target, BasicBlockId(1));
    }

    #[test]
    fn terminator_return() {
        assert_eq!(TerminatorKind::Return, TerminatorKind::Return);
    }

    #[test]
    fn terminator_switch_int() {
        let t = TerminatorKind::SwitchInt {
            discr: Operand::Copy(Local(0)),
            targets: vec![(1, BasicBlockId(2))],
            otherwise: BasicBlockId(0),
        };
        let targets = match t {
            TerminatorKind::SwitchInt { targets, .. } => targets,
            _ => unreachable!(),
        };
        assert_eq!(targets[0], (1, BasicBlockId(2)));
    }

    #[test]
    fn terminator_unreachable() {
        assert_eq!(TerminatorKind::Unreachable, TerminatorKind::Unreachable);
    }

    #[test]
    fn basic_block_empty() {
        let bb = BasicBlock {
            statements: vec![],
            terminator: None,
        };
        assert!(bb.statements.is_empty());
        assert!(bb.terminator.is_none());
    }

    #[test]
    fn basic_block_with_stmts() {
        let bb = BasicBlock {
            statements: vec![Statement {
                kind: StatementKind::StorageLive(Local(0)),
                span: Span {
                    file_id: 0,
                    line: 0,
                    col: 0,
                    start: 0,
                    end: 0,
                },
            }],
            terminator: Some(Terminator {
                kind: TerminatorKind::Return,
                span: Span {
                    file_id: 0,
                    line: 0,
                    col: 0,
                    start: 0,
                    end: 0,
                },
            }),
        };
        assert_eq!(bb.statements.len(), 1);
        assert!(bb.terminator.is_some());
    }

    #[test]
    fn local_decl_defaults() {
        let d = LocalDecl {
            ty: Type::Int,
            name: None,
            span: Span {
                file_id: 0,
                line: 0,
                col: 0,
                start: 0,
                end: 0,
            },
            is_mut: false,
            is_owning: false,
        };
        assert_eq!(d.ty, Type::Int);
        assert!(d.name.is_none());
    }

    #[test]
    fn mir_function_empty() {
        let f = MirFunction {
            name: "f".into(),
            locals: vec![],
            basic_blocks: vec![],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        };
        assert_eq!(f.name, "f");
        assert!(f.locals.is_empty());
        assert!(f.basic_blocks.is_empty());
    }

    #[test]
    fn mir_function_with_locals() {
        let f = MirFunction {
            name: "g".into(),
            locals: vec![LocalDecl {
                ty: Type::Int,
                name: Some("x".into()),
                span: Span {
                    file_id: 0,
                    line: 0,
                    col: 0,
                    start: 0,
                    end: 0,
                },
                is_mut: true,
                is_owning: true,
            }],
            basic_blocks: vec![BasicBlock {
                statements: vec![],
                terminator: Some(Terminator {
                    kind: TerminatorKind::Return,
                    span: Span {
                        file_id: 0,
                        line: 0,
                        col: 0,
                        start: 0,
                        end: 0,
                    },
                }),
            }],
            arg_count: 1,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec!["x".into()],
            is_async: false,
        };
        assert_eq!(f.locals.len(), 1);
        assert_eq!(f.basic_blocks.len(), 1);
        assert_eq!(f.arg_count, 1);
        assert_eq!(f.param_names[0], "x");
    }

    #[test]
    fn rvalue_vector_splat() {
        let r = Rvalue::VectorSplat(Operand::Copy(Local(0)), 4);
        let (_, n) = match r {
            Rvalue::VectorSplat(_, n) => ((), n),
            _ => unreachable!(),
        };
        assert_eq!(n, 4);
    }

    #[test]
    fn rvalue_vtable_load() {
        let r = Rvalue::VTableLoad {
            vtable: Operand::Copy(Local(0)),
            method_idx: 2,
        };
        let idx = match r {
            Rvalue::VTableLoad { method_idx, .. } => method_idx,
            _ => unreachable!(),
        };
        assert_eq!(idx, 2);
    }

    #[test]
    fn rvalue_fat_ptr_data() {
        let r = Rvalue::FatPtrData(Operand::Copy(Local(0)));
        assert!(matches!(r, Rvalue::FatPtrData(_)));
    }
}
