use std::sync::atomic::{AtomicUsize, Ordering};

static NODE_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

fn next_node_id() -> usize {
    NODE_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

use crate::span::Span;

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: String,
    pub types: Vec<TypeExpr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

impl TypeExpr {
    pub fn new(kind: TypeExprKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExprKind {
    Name(String),
    Generic(String, Vec<TypeExpr>),
    Tuple(Vec<TypeExpr>),
    List(Box<TypeExpr>),
    Dict(Box<TypeExpr>, Box<TypeExpr>),
    #[allow(dead_code)]
    Fn {
        params: Vec<TypeExpr>,
        ret: Box<TypeExpr>,
    },
    #[allow(dead_code)]
    Ref(Box<TypeExpr>),
    #[allow(dead_code)]
    MutRef(Box<TypeExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    In,
    NotIn,
    Is,
    IsNot,
    Shl,
    Shr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Neg,
    Pos,
    Not,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AugOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamKind {
    Regular,
    VarArg,
    KwArg,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Param {
    pub name: String,
    pub type_ann: Option<TypeExpr>,
    pub default: Option<Expr>,
    pub kind: ParamKind,
    pub is_mut: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ForTarget {
    Name(String, Span),
    Tuple(Vec<(String, Span)>, Span),
}

#[derive(Debug, Clone)]
pub struct CompClause {
    pub target: ForTarget,
    pub iter: Expr,
    pub condition: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct ExceptHandler {
    pub exc_type: Option<Expr>,
    pub name: Option<String>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub id: usize,
    pub kind: ExprKind,
    pub span: Span,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Self {
            id: next_node_id(),
            kind,
            span,
        }
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

    BinOp {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
    },

    Call {
        callee: Box<Expr>,
        args: Vec<CallArg>,
    },
    Index {
        obj: Box<Expr>,
        index: Box<Expr>,
    },
    Attr {
        obj: Box<Expr>,
        attr: String,
    },

    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Set(Vec<Expr>),
    Dict(Vec<(Expr, Expr)>),

    ListComp {
        elt: Box<Expr>,
        clauses: Vec<CompClause>,
    },
    SetComp {
        elt: Box<Expr>,
        clauses: Vec<CompClause>,
    },
    DictComp {
        key: Box<Expr>,
        value: Box<Expr>,
        clauses: Vec<CompClause>,
    },

    Walrus {
        name: String,
        value: Box<Expr>,
    },

    Borrow(Box<Expr>),
    MutBorrow(Box<Expr>),

    Match {
        expr: Box<Expr>,
        cases: Vec<MatchCase>,
    },
}

#[derive(Debug, Clone)]
pub struct MatchCase {
    pub pattern: MatchPattern,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum MatchPattern {
    Variant(String, Vec<MatchPattern>),
    Identifier(String),
    Wildcard,
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
        Self {
            id: next_node_id(),
            kind,
            span,
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum StmtKind {
    Fn {
        name: String,
        params: Vec<Param>,
        return_type: Option<TypeExpr>,
        body: Vec<Stmt>,
        decorators: Vec<String>,
    },
    Struct {
        name: String,
        fields: Vec<Param>,  // named fields with optional type annotations
        body: Vec<Stmt>,     // associated consts / nested types inside struct block
    },
    Impl {
        type_name: String,   // which struct this impl is for
        body: Vec<Stmt>,     // fn definitions
    },
    Enum {
        name: String,
        variants: Vec<EnumVariant>,
    },
    If {
        condition: Expr,
        then_body: Vec<Stmt>,
        elif_clauses: Vec<(Expr, Vec<Stmt>)>,
        else_body: Option<Vec<Stmt>>,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
        else_body: Option<Vec<Stmt>>,
    },
    For {
        target: ForTarget,
        iter: Expr,
        body: Vec<Stmt>,
        else_body: Option<Vec<Stmt>>,
    },
    Try {
        body: Vec<Stmt>,
        handlers: Vec<ExceptHandler>,
        else_body: Option<Vec<Stmt>>,
        finally_body: Option<Vec<Stmt>>,
    },
    Return(Option<Expr>),
    Raise(Option<Expr>),
    Assert {
        test: Expr,
        msg: Option<Expr>,
    },
    Import(Vec<String>),
    FromImport {
        module: Vec<String>,
        names: Vec<String>,
    },
    Let {
        name: String,
        type_ann: Option<TypeExpr>,
        value: Expr,
        is_mut: bool,
    },
    Const {
        name: String,
        type_ann: Option<TypeExpr>,
        value: Expr,
    },
    Assign {
        target: Expr,
        value: Expr,
    },
    AugAssign {
        target: Expr,
        op: AugOp,
        value: Expr,
    },
    Pass,
    Break,
    Continue,
    ExprStmt(Expr),
}

#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}
