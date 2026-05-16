use std::fmt;

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
    Struct(String, Vec<Type>),
    Enum(String, Vec<Type>),
    Param(String),
    Union(Vec<Type>),
    Fn(Vec<Type>, Box<Type>, Vec<Type>),
    Tuple(Vec<Type>),
    List(Box<Type>),
    Dict(Box<Type>, Box<Type>),
    Set(Box<Type>),
    Ref(Box<Type>),
    MutRef(Box<Type>),
    Ptr(Box<Type>),
    Var(usize),
    Any,
    Never,
    Vector(Box<Type>, usize),
    Future(Box<Type>),
}

impl Type {
    pub fn is_move_type(&self) -> bool {
        !matches!(
            self,
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
                | Type::Ptr(_)
                | Type::Vector(_, _)
                | Type::Future(_)
                | Type::Param(_)
        )
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
            Type::Struct(name, args) | Type::Enum(name, args) => {
                write!(f, "{}", name)?;
                if !args.is_empty() {
                    write!(f, "[")?;
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", arg)?;
                    }
                    write!(f, "]")?;
                }
                Ok(())
            }
            Type::Param(name) => write!(f, "{}", name),
            Type::Fn(params, ret, args) => {
                if !args.is_empty() {
                    write!(f, "[")?;
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", arg)?;
                    }
                    write!(f, "]")?;
                }
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
            Type::Ptr(t) => write!(f, "*{}", t),
            Type::Var(id) => write!(f, "?T{}", id),
            Type::Any => write!(f, "Any"),
            Type::Never => write!(f, "Never"),
            Type::Vector(t, w) => write!(f, "{}x{}", t, w),
            Type::Future(t) => write!(f, "Future[{}]", t),
        }
    }
}
