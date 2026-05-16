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
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AggregateKind {
    Tuple,
    List,
    Set,
    Dict,
    EnumVariant(i64, usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Rvalue {
    Use(Operand),
    BinaryOp(BinOp, Operand, Operand),
    UnaryOp(UnaryOp, Operand),
    Call { func: Operand, args: Vec<Operand> },
    Aggregate(AggregateKind, Vec<Operand>),
    GetAttr(Operand, String),
    GetIndex(Operand, Operand),
    GetTag(Operand),
    GetTypeId(Operand),
    Ref(Local),
    MutRef(Local),
    VectorSplat(Operand, usize),
    VectorLoad(Operand, Operand, usize),
    VectorFMA(Operand, Operand, Operand),
    PtrLoad(Operand),
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
    SetIndex(Operand, Operand, Operand),
    StorageLive(Local),
    StorageDead(Local),
    Drop(Local),
    VectorStore(Operand, Operand, Operand),
    PtrStore(Operand, Operand),
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
