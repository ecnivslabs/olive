use crate::parser::{BinOp, UnaryOp};
use crate::semantic::types::Type;
use crate::span::Span;

// local variable
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Local(pub usize);

// basic block id
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BasicBlockId(pub usize);

// Operands can be copies, moves, or constants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Operand {
    // shared access
    Copy(Local),
    // move
    Move(Local),
    // constant
    Constant(Constant),
}

// Constant values supported by the MIR.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Constant {
    Int(i64),
    Float(u64), // use bits for eq/hash
    Str(String),
    Bool(bool),
    Function(String),
    None,
}

// Kind of aggregate construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AggregateKind {
    Tuple,
    List,
    Set,
    Dict,
    EnumVariant(usize),
}

// Right-hand side expressions in an assignment.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Rvalue {
    // use operand
    Use(Operand),
    // binary op
    BinaryOp(BinOp, Operand, Operand),
    // unary op
    UnaryOp(UnaryOp, Operand),
    // function/method call
    Call { func: Operand, args: Vec<Operand> },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}
// Possible statements in a basic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementKind {
    // local = rvalue
    Assign(Local, Rvalue),
    // write to attribute
    SetAttr(Operand, String, Operand),
    // write to index
    SetIndex(Operand, Operand, Operand),
    // mark storage live
    StorageLive(Local),
    // mark storage dead
    StorageDead(Local),
    // drop local
    Drop(Local),
    // simd: vector store
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

// Declaration metadata for a local variable.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub vararg_idx: Option<usize>,
    pub kwarg_idx: Option<usize>,
    pub param_names: Vec<String>,
    pub is_async: bool,
}
