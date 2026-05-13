use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

static TYPE_VAR_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Int,
    I8,
    I16,
    I32,
    U8,
    U16,
    U32,
    U64,
    Float,
    F32,
    Str,
    Bool,
    Null,
    // Named user-defined type (struct)
    Struct(String),
    // Enum type
    Enum(String),
    // Union type: A | B
    Union(Vec<Type>),
    // Function type: (params) -> return_type
    Fn(Vec<Type>, Box<Type>),
    // Tuple type: (T1, T2, ...)
    Tuple(Vec<Type>),
    // List type: [T]
    List(Box<Type>),
    // Dict type: {K: V}
    Dict(Box<Type>, Box<Type>),
    // Set type: {T}
    Set(Box<Type>),
    // Reference type: &T
    Ref(Box<Type>),
    // Mutable reference type: &mut T
    MutRef(Box<Type>),
    // Type variable for inference
    Var(usize),
    // "Any" type for dynamic fallback
    Any,
    // "Never" type for unreachable paths
    Never,
    // Vector type for SIMD: Vector(element_type, width)
    Vector(Box<Type>, usize),
    // Future[T]: produced by async fn / async: blocks
    Future(Box<Type>),
}

impl Type {
    // Allocate a globally unique type variable ID.
    pub fn new_var() -> Self {
        let id = TYPE_VAR_COUNTER.fetch_add(1, Ordering::Relaxed);
        Type::Var(id)
    }

    /// Returns true if this type has move semantics (heap allocated or complex).
    pub fn is_move_type(&self) -> bool {
        match self {
            Type::Int
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Float
            | Type::F32
            | Type::Bool
            | Type::Null
            | Type::Never
            | Type::Any
            | Type::Str
            | Type::Ref(_)
            | Type::MutRef(_)
            | Type::Vector(_, _)
            | Type::Future(_) => false,
            _ => true,
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Int => write!(f, "int"),
            Type::I8 => write!(f, "i8"),
            Type::I16 => write!(f, "i16"),
            Type::I32 => write!(f, "i32"),
            Type::U8 => write!(f, "u8"),
            Type::U16 => write!(f, "u16"),
            Type::U32 => write!(f, "u32"),
            Type::U64 => write!(f, "u64"),
            Type::Float => write!(f, "float"),
            Type::F32 => write!(f, "f32"),
            Type::Str => write!(f, "str"),
            Type::Bool => write!(f, "bool"),
            Type::Null => write!(f, "None"),
            Type::Union(variants) => {
                for (i, v) in variants.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{}", v)?;
                }
                Ok(())
            }
            Type::Struct(name) | Type::Enum(name) => write!(f, "{}", name),
            Type::Fn(params, ret) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ret)
            }
            Type::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", e)?;
                }
                if elems.len() == 1 {
                    write!(f, ",")?;
                }
                write!(f, ")")
            }
            Type::List(t) => write!(f, "[{}]", t),
            Type::Dict(k, v) => write!(f, "{{{}: {}}}", k, v),
            Type::Set(t) => write!(f, "{{{}}}", t),
            Type::Ref(t) => write!(f, "&{}", t),
            Type::MutRef(t) => write!(f, "&mut {}", t),
            Type::Var(id) => write!(f, "?T{}", id),
            Type::Any => write!(f, "Any"),
            Type::Never => write!(f, "Never"),
            Type::Vector(t, w) => write!(f, "{}x{}", t, w),
            Type::Future(t) => write!(f, "Future[{}]", t),
        }
    }
}
