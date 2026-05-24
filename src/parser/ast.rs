use std::sync::atomic::{AtomicUsize, Ordering};

static NODE_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

fn next_node_id() -> usize {
    NODE_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

use crate::span::Span;

#[derive(Debug, Clone)]
pub struct FfiParam {
    pub ty: TypeExpr,
}

#[derive(Debug, Clone)]
pub struct FfiVarDef {
    pub name: String,
    pub ty: TypeExpr,
}

#[derive(Debug, Clone)]
pub struct FfiFnSig {
    pub name: String,
    pub params: Vec<FfiParam>,
    pub ret: Option<TypeExpr>,
    pub is_vararg: bool,
    pub decorators: Vec<Decorator>,
    pub call_conv: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FfiStructField {
    pub name: String,
    pub ty: TypeExpr,
    pub bits: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct FfiStructDef {
    pub name: String,
    pub fields: Vec<FfiStructField>,
    pub is_union: bool,
    pub destructor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FfiConstDef {
    pub name: String,
    pub value: i64,
}

#[derive(Debug, Clone)]
pub struct Decorator {
    pub name: String,
    pub is_directive: bool,
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: String,
    pub types: Vec<TypeExpr>,
    pub value: Option<Expr>,
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
    Union(Box<TypeExpr>, Box<TypeExpr>),
    Fn {
        params: Vec<TypeExpr>,
        ret: Box<TypeExpr>,
    },
    Ref(Box<TypeExpr>),
    MutRef(Box<TypeExpr>),
    Ptr(Box<TypeExpr>),
    FixedArray(Box<TypeExpr>, usize),
}

impl std::fmt::Display for TypeExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.kind)
    }
}

impl std::fmt::Display for TypeExprKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeExprKind::Name(n) => write!(f, "{}", n),
            TypeExprKind::Generic(n, args) => {
                write!(f, "{}[", n)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arg)?;
                }
                write!(f, "]")
            }
            TypeExprKind::Tuple(args) => {
                write!(f, "(")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arg)?;
                }
                write!(f, ")")
            }
            TypeExprKind::List(t) => write!(f, "[{}]", t),
            TypeExprKind::Dict(k, v) => write!(f, "{{{}: {}}}", k, v),
            TypeExprKind::Union(a, b) => write!(f, "{} | {}", a, b),
            TypeExprKind::Fn { params, ret } => {
                write!(f, "fn(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ret)
            }
            TypeExprKind::Ref(t) => write!(f, "&{}", t),
            TypeExprKind::MutRef(t) => write!(f, "&mut {}", t),
            TypeExprKind::Ptr(t) => write!(f, "*{}", t),
            TypeExprKind::FixedArray(t, n) => write!(f, "[{}; {}]", t, n),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
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
    Shl,
    Shr,
    BitOr,
    BitAnd,
    BitXor,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Neg,
    Pos,
    Not,
    BitNot,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AugOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Shl,
    Shr,
    BitOr,
    BitAnd,
    BitXor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamKind {
    Regular,
    VarArg,
    KwArg,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub type_ann: Option<TypeExpr>,
    pub default: Option<Expr>,
    pub kind: ParamKind,
    pub is_mut: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ForTarget {
    Name(String, Span),
    Tuple(Vec<(String, Span)>),
}

#[derive(Debug, Clone)]
pub struct CompClause {
    pub target: ForTarget,
    pub iter: Expr,
    pub condition: Option<Expr>,
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
    Cast(Box<Expr>, TypeExpr),

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

    Borrow(Box<Expr>),
    MutBorrow(Box<Expr>),
    Deref(Box<Expr>),

    Try(Box<Expr>),
    Await(Box<Expr>),
    AsyncBlock(Vec<Stmt>),

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
    Literal(Expr),
    Wildcard,
}

#[derive(Debug, Clone)]
pub enum CallArg {
    Positional(Expr),
    Keyword(String, Expr),
    Splat(Expr),
    KwSplat(Expr),
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

impl Stmt {
    pub fn new(kind: StmtKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Fn {
        name: String,
        type_params: Vec<String>,
        params: Vec<Param>,
        return_type: Option<TypeExpr>,
        body: Vec<Stmt>,
        decorators: Vec<Decorator>,
        is_async: bool,
    },
    Struct {
        name: String,
        type_params: Vec<String>,
        fields: Vec<Param>,
        body: Vec<Stmt>,
        decorators: Vec<Decorator>,
    },
    Impl {
        type_params: Vec<String>,
        trait_name: Option<TypeExpr>,
        type_name: TypeExpr,
        body: Vec<Stmt>,
    },
    Trait {
        name: String,
        type_params: Vec<String>,
        methods: Vec<Stmt>,
    },
    Enum {
        name: String,
        type_params: Vec<String>,
        variants: Vec<EnumVariant>,
        body: Vec<Stmt>,
        decorators: Vec<Decorator>,
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
    Return(Option<Expr>),
    Assert {
        test: Expr,
        msg: Option<Expr>,
    },
    Import {
        module: Vec<String>,
        alias: Option<String>,
    },
    NativeImport {
        path: String,
        alias: String,
        functions: Vec<FfiFnSig>,
        structs: Vec<FfiStructDef>,
        vars: Vec<FfiVarDef>,
        consts: Vec<FfiConstDef>,
        block_safe: bool,
    },
    FromImport {
        module: Vec<String>,
        names: Vec<(String, Option<String>)>,
        is_star: bool,
    },
    PyImport {
        module: String,
        alias: String,
    },
    Let {
        name: String,
        type_ann: Option<TypeExpr>,
        value: Expr,
        is_mut: bool,
    },
    MultiLet {
        names: Vec<String>,
        type_ann: Option<TypeExpr>,
        value: Expr,
        is_mut: bool,
    },
    Const {
        name: String,
        type_ann: Option<TypeExpr>,
        value: Expr,
    },
    MultiConst {
        names: Vec<String>,
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
    UnsafeBlock(Vec<Stmt>),
    Defer(Expr),
}

#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}
