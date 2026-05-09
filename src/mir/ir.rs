use crate::parser::{BinOp, UnaryOp};
use crate::semantic::types::Type;
use crate::span::Span;

// local variable
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Local(pub usize);

// basic block id
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BasicBlockId(pub usize);

// operand
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum Operand {
    // shared access
    Copy(Local),
    // move
    Move(Local),
    // constant
    Constant(Constant),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum Constant {
    Int(i64),
    Float(u64), // use bits for eq/hash
    Str(String),
    Bool(bool),
    Function(String),
    None,
}

// rhs expressions
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AggregateKind {
    Tuple,
    List,
    Set,
    Dict,
    EnumVariant(usize),
}

// rhs expressions
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum Rvalue {
    // use operand
    Use(Operand),
    // binary op
    BinaryOp(BinOp, Operand, Operand),
    // unary op
    UnaryOp(UnaryOp, Operand),
    // function/method call
    Call {
        func: Operand,
        args: Vec<Operand>,
    },
    // aggregate construction
    Aggregate(AggregateKind, Vec<Operand>),
    // attribute access
    GetAttr(Operand, String),
    // index access
    GetIndex(Operand, Operand),
    // get enum tag
    GetTag(Operand),
    // shared reference
    Ref(Local),
    // mutable reference
    MutRef(Local),
    // simd: splat
    VectorSplat(Operand, usize),
    // simd: load
    VectorLoad(Operand, Operand, usize),
    // simd: fma
    VectorFMA(Operand, Operand, Operand),
}

// block statement
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub kind: StatementKind,
    #[allow(dead_code)]
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum StatementKind {
    // local = rvalue
    Assign(Local, Rvalue),
    /// Writes a value to an object attribute.
    SetAttr(Operand, String, Operand),
    /// Writes a value to a collection index.
    SetIndex(Operand, Operand, Operand),
    /// Marks a local's storage as live.
    StorageLive(Local),
    /// Marks a local's storage as dead.
    StorageDead(Local),
    /// Explicitly drops the value in a local.
    Drop(Local),
    /// SIMD: Store a vector to a collection at index.
    VectorStore(Operand, Operand, Operand),
}

// block terminator
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminatorKind {
    // Unconditional branch.
    Goto {
        target: BasicBlockId,
    },
    // Conditional branch.
    SwitchInt {
        discr: Operand,
        targets: Vec<(i64, BasicBlockId)>,
        otherwise: BasicBlockId,
    },
    // return
    Return,
    // unreachable
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminator {
    pub kind: TerminatorKind,
    pub span: Span,
}

// basic block sequence
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub statements: Vec<Statement>,
    pub terminator: Option<Terminator>,
}

// local metadata
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct LocalDecl {
    pub ty: Type,
    pub name: Option<String>,
    pub span: Span,
    pub is_mut: bool,
}

// mir function
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirFunction {
    pub name: String,
    pub locals: Vec<LocalDecl>,
    pub basic_blocks: Vec<BasicBlock>,
    pub arg_count: usize,
}
