use crate::parser::{BinOp, UnaryOp};
use crate::semantic::types::Type;
use crate::span::Span;

// Local variable or temporary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Local(pub usize);

// Basic Block ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BasicBlockId(pub usize);

// Operand: constant or local reference.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Operand {
    // Shared access to a local.
    Copy(Local),
    // Ownership transfer (move).
    Move(Local),
    // Constant literal.
    Constant(Constant),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Constant {
    Int(i64),
    Float(u64), // Use bits to allow Eq/Hash
    Str(String),
    Bool(bool),
    Function(String),
    None,
}

// RHS expressions that compute a value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Rvalue {
    // Just use the operand.
    Use(Operand),
    /// Binary operation between two operands.
    BinaryOp(BinOp, Operand, Operand),
    /// Unary operation on an operand.
    UnaryOp(UnaryOp, Operand),
    /// Function or method call.
    Call {
        func: Operand,
        args: Vec<Operand>,
    },
    /// Constructs an aggregate value like a list, tuple, set, or dict.
    Aggregate(AggregateKind, Vec<Operand>),
    /// Reads a field or attribute from an object by name.
    GetAttr(Operand, String),
    /// Reads an element by index or key from a collection.
    GetIndex(Operand, Operand),
    /// Creates a shared reference to a local (&local).
    Ref(Local),
    /// Creates a mutable reference to a local (&mut local).
    MutRef(Local),
}

// Aggregate value kinds (list, tuple, etc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregateKind {
    Tuple,
    List,
    Set,
    /// Dictionaries are represented as alternating key and value operands.
    Dict,
}

// Block statement.
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
}

// Block terminator.
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
    /// Returns from the current function.
    Return,
    /// Indicates an unreachable path (e.g., after a panic).
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminator {
    pub kind: TerminatorKind,
    pub span: Span,
}

// Sequence of statements ending with a terminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub statements: Vec<Statement>,
    pub terminator: Option<Terminator>,
}

// Metadata for a local.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct LocalDecl {
    pub ty: Type,
    pub name: Option<String>,
    pub span: Span,
    pub is_mut: bool,
}

// MIR function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirFunction {
    pub name: String,
    pub locals: Vec<LocalDecl>,
    pub basic_blocks: Vec<BasicBlock>,
    pub arg_count: usize,
}
