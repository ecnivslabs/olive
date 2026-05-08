use std::sync::atomic::{AtomicUsize, Ordering};

static NODE_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

fn next_node_id() -> usize {
    NODE_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

use crate::span::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExpr {
    Named(String),
    Generic { name: String, args: Vec<TypeExpr> },
    Tuple(Vec<TypeExpr>),
    Fn { params: Vec<TypeExpr>, ret: Box<TypeExpr> },
    Ref(Box<TypeExpr>),
    MutRef(Box<TypeExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinOp {
    Add, Sub, Mul, Div, FloorDiv, Mod, Pow,
    Eq, NotEq, Lt, LtEq, Gt, GtEq,
    And, Or,
    In, NotIn,
    Is, IsNot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnaryOp { Neg, Pos, Not }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AugOp { Add, Sub, Mul, Div, FloorDiv, Mod, Pow }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamKind { Regular, VarArg, KwArg }

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Param {
    pub name:     String,
    pub type_ann: Option<TypeExpr>,
    pub default:  Option<Expr>,
    pub kind:     ParamKind,
    pub is_mut:   bool,
    pub span:     Span,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ForTarget {
    Name(String, Span),
    Tuple(Vec<(String, Span)>, Span),
}

#[derive(Debug, Clone)]
pub struct CompClause {
    pub target:    ForTarget,
    pub iter:      Expr,
    pub condition: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct ExceptHandler {
    pub exc_type: Option<Expr>,
    pub name:     Option<String>,
    pub body:     Vec<Stmt>,
    pub span:     Span,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub id: usize,
    pub kind: ExprKind,
    pub span: Span,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Self { id: next_node_id(), kind, span }
    }
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Integer(i64),
    Float(f64),
    Str(String),
    FStr(Vec<Expr>),
    Bool(bool),
    Null,
    Identifier(String),

    BinOp   { left: Box<Expr>, op: BinOp, right: Box<Expr> },
    UnaryOp { op: UnaryOp, operand: Box<Expr> },

    Call  { callee: Box<Expr>, args: Vec<CallArg> },
    Index { obj: Box<Expr>, index: Box<Expr> },
    Attr  { obj: Box<Expr>, attr: String },

    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Set(Vec<Expr>),
    Dict(Vec<(Expr, Expr)>),

    ListComp { elt: Box<Expr>, clauses: Vec<CompClause> },
    SetComp  { elt: Box<Expr>, clauses: Vec<CompClause> },
    DictComp { key: Box<Expr>, value: Box<Expr>, clauses: Vec<CompClause> },

    Walrus { name: String, value: Box<Expr> },

    Borrow(Box<Expr>),
    MutBorrow(Box<Expr>),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum CallArg {
    Positional(Expr),
    Keyword(String, Expr),
    Splat(Expr),
    KwSplat(Expr),
}

#[derive(Debug, Clone)]
pub struct Stmt {
    #[allow(dead_code)]
    pub id: usize,
    pub kind: StmtKind,
    pub span: Span,
}

impl Stmt {
    pub fn new(kind: StmtKind, span: Span) -> Self {
        Self { id: next_node_id(), kind, span }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum StmtKind {
    Fn {
        name:        String,
        params:      Vec<Param>,
        return_type: Option<TypeExpr>,
        body:        Vec<Stmt>,
        decorators:  Vec<String>,
    },
    Class {
        name:  String,
        bases: Vec<Expr>,
        body:  Vec<Stmt>,
    },
    If {
        condition:    Expr,
        then_body:    Vec<Stmt>,
        elif_clauses: Vec<(Expr, Vec<Stmt>)>,
        else_body:    Option<Vec<Stmt>>,
    },
    While {
        condition: Expr,
        body:      Vec<Stmt>,
        else_body: Option<Vec<Stmt>>,
    },
    For {
        target:    ForTarget,
        iter:      Expr,
        body:      Vec<Stmt>,
        else_body: Option<Vec<Stmt>>,
    },
    Try {
        body:         Vec<Stmt>,
        handlers:     Vec<ExceptHandler>,
        else_body:    Option<Vec<Stmt>>,
        finally_body: Option<Vec<Stmt>>,
    },
    Return(Option<Expr>),
    Raise(Option<Expr>),
    Assert { test: Expr, msg: Option<Expr> },
    Import(Vec<String>),
    FromImport { module: Vec<String>, names: Vec<String> },
    Let { name: String, type_ann: Option<TypeExpr>, value: Expr, is_mut: bool },
    Assign    { target: Expr, value: Expr },
    AugAssign { target: Expr, op: AugOp, value: Expr },
    Pass,
    Break,
    Continue,
    ExprStmt(Expr),
}

#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}
