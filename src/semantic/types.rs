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
    Usize,
    Float,
    F32,
    Str,
    Bytes,
    Bool,
    Null,
    Struct(String, Vec<Type>, bool),
    Enum(String, Vec<Type>),
    TraitObject(String, Vec<Type>),
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
    PyObject,
    PyNamed(String, String),
    IntegerLiteral(usize),
    FloatLiteral(usize),
}

impl Type {
    pub fn is_py_value(&self) -> bool {
        match self {
            Type::PyObject | Type::PyNamed(_, _) => true,
            Type::Union(members) => members.iter().any(|m| m.is_py_value()),
            Type::Ref(inner) | Type::MutRef(inner) => inner.is_py_value(),
            _ => false,
        }
    }

    /// Scalar members plus None. Stored Any-tag encoded so a real zero
    /// stays distinct from None at runtime.
    pub fn is_scalar_nullable_union(&self) -> bool {
        let is_scalar = |m: &Type| {
            matches!(
                m,
                Type::Int
                    | Type::I8
                    | Type::I16
                    | Type::I32
                    | Type::U8
                    | Type::U16
                    | Type::U32
                    | Type::U64
                    | Type::Usize
                    | Type::Float
                    | Type::F32
                    | Type::Bool
            )
        };
        match self {
            Type::Union(members) => {
                members.iter().any(|m| matches!(m, Type::Null))
                    && members.iter().any(is_scalar)
                    && members
                        .iter()
                        .all(|m| matches!(m, Type::Null) || is_scalar(m))
            }
            _ => false,
        }
    }

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
                | Type::Usize
                | Type::Float
                | Type::F32
                | Type::Bool
                | Type::Null
                | Type::Never
                | Type::Any
                | Type::Ref(_)
                | Type::MutRef(_)
                | Type::Ptr(_)
                | Type::Vector(_, _)
                | Type::Future(_)
                | Type::Param(_)
                | Type::IntegerLiteral(_)
                | Type::FloatLiteral(_)
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
            Type::Usize => write!(f, "usize"),
            Type::Float => write!(f, "float"),
            Type::F32 => write!(f, "f32"),
            Type::Str => write!(f, "str"),
            Type::Bytes => write!(f, "bytes"),
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
            Type::Struct(name, args, _)
            | Type::Enum(name, args)
            | Type::TraitObject(name, args) => {
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
            Type::PyObject => write!(f, "PyObject"),
            Type::PyNamed(module, name) => write!(f, "{}.{}", module, name),
            Type::IntegerLiteral(_) => write!(f, "{{integer}}"),
            Type::FloatLiteral(_) => write!(f, "{{float}}"),
        }
    }
}

/// Result of checking whether a cast from `src` to `dst` is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastKind {
    /// Numeric-to-numeric or numeric-to-float (already implemented).
    NumericExisting,
    /// Scalar (int/float/bool) to str, via the runtime `olive_str` etc.
    ScalarToStr,
    /// Not a valid cast.
    Invalid,
}

/// Classifies a cast from type `src` to type `dst`. Both sides must be fully
/// resolved (no type variables) before calling; the checker calls
/// `apply_subst` first.
pub fn cast_kind(src: &Type, dst: &Type) -> CastKind {
    // Helper: is the type a numeric kind (int or float)?
    fn is_numeric(t: &Type) -> bool {
        matches!(
            t,
            Type::Int
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Usize
                | Type::Float
                | Type::F32
                | Type::Bool
        )
    }
    fn is_int(t: &Type) -> bool {
        matches!(
            t,
            Type::Int
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Usize
                | Type::Bool
        )
    }
    fn is_float(t: &Type) -> bool {
        matches!(t, Type::Float | Type::F32)
    }
    // Unresolved type variables may remain after substitution if nothing
    // constrained the literal; treat them as numeric.
    //
    //                 source ↓                →  destination
    match (src, dst) {
        // int-like → int-like (widening, narrowing, sign change)
        (s, d) if is_int(s) && is_int(d) => CastKind::NumericExisting,
        // int-like ↔ float
        (s, d) if (is_int(s) && is_float(d)) || (is_float(s) && is_int(d)) => {
            CastKind::NumericExisting
        }
        // float ↔ float
        (s, d) if is_float(s) && is_float(d) => CastKind::NumericExisting,
        // numeric → str
        (s, Type::Str) if is_numeric(s) => CastKind::ScalarToStr,
        // unconstrained integer literal → int/float (format resolved at codegen)
        (Type::IntegerLiteral(_), d) if is_numeric(d) => CastKind::NumericExisting,
        // unconstrained integer/float literal → str
        (Type::IntegerLiteral(_) | Type::FloatLiteral(_), Type::Str) => CastKind::ScalarToStr,
        // PyObject → native (int/float/str/bool): runtime conversion
        (Type::PyObject | Type::PyNamed(_, _), _)
            if is_numeric(dst) || *dst == Type::Str || *dst == Type::Bool =>
        {
            CastKind::NumericExisting
        }
        // Any → native: runtime unboxing
        (Type::Any, _) if is_numeric(dst) || *dst == Type::Str || *dst == Type::Bool => {
            CastKind::NumericExisting
        }
        // Unresolved type variable / generic param: could be anything, allow
        (Type::Var(_) | Type::Param(_), _) => CastKind::NumericExisting,
        _ => CastKind::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_type() {
        assert_eq!(Type::Int, Type::Int);
        assert_ne!(Type::Int, Type::Bool);
    }

    #[test]
    fn type_var_eq() {
        assert_eq!(Type::Var(0), Type::Var(0));
        assert_ne!(Type::Var(0), Type::Var(1));
    }

    #[test]
    fn fn_type_3_args() {
        let t = Type::Fn(vec![Type::Int], Box::new(Type::Bool), vec![]);
        let (params, ret, _) = match &t {
            Type::Fn(p, r, c) => (p, r, c),
            _ => panic!(),
        };
        assert_eq!(params.len(), 1);
        assert_eq!(**ret, Type::Bool);
    }

    #[test]
    fn list_type() {
        let t = Type::List(Box::new(Type::Int));
        let inner = match &t {
            Type::List(i) => i,
            _ => panic!(),
        };
        assert_eq!(**inner, Type::Int);
    }

    #[test]
    fn dict_type() {
        let t = Type::Dict(Box::new(Type::Str), Box::new(Type::Int));
        let (k, v) = match &t {
            Type::Dict(k, v) => (k, v),
            _ => panic!(),
        };
        assert_eq!(**k, Type::Str);
        assert_eq!(**v, Type::Int);
    }

    #[test]
    fn ref_types() {
        let t = Type::Ref(Box::new(Type::Int));
        let inner = match &t {
            Type::Ref(i) => i,
            _ => panic!(),
        };
        assert_eq!(**inner, Type::Int);
        let t2 = Type::MutRef(Box::new(Type::Int));
        assert!(matches!(t2, Type::MutRef(_)));
    }

    #[test]
    fn ptr_type() {
        let t = Type::Ptr(Box::new(Type::Int));
        assert!(matches!(t, Type::Ptr(_)));
    }

    #[test]
    fn tuple_type_multi() {
        let t = Type::Tuple(vec![Type::Int, Type::Bool, Type::Str]);
        let elems = match &t {
            Type::Tuple(e) => e,
            _ => panic!(),
        };
        assert_eq!(elems.len(), 3);
    }

    #[test]
    fn struct_type() {
        let t = Type::Struct("Point".into(), vec![Type::Int, Type::Int], false);
        let (name, _fields) = match &t {
            Type::Struct(n, f, _) => (n, f),
            _ => panic!(),
        };
        assert_eq!(name, "Point");
    }

    #[test]
    fn enum_type() {
        let t = Type::Enum("Opt".into(), vec![Type::Int]);
        let (name, variants) = match &t {
            Type::Enum(n, v) => (n, v),
            _ => panic!(),
        };
        assert_eq!(name, "Opt");
        assert_eq!(variants.len(), 1);
    }

    #[test]
    fn param_type() {
        let t = Type::Param("T".into());
        let n = match &t {
            Type::Param(n) => n,
            _ => panic!(),
        };
        assert_eq!(n, "T");
    }

    #[test]
    fn union_type() {
        let t = Type::Union(vec![Type::Int, Type::Str]);
        let variants = match &t {
            Type::Union(v) => v,
            _ => panic!(),
        };
        assert_eq!(variants.len(), 2);
    }

    #[test]
    fn set_type() {
        let t = Type::Set(Box::new(Type::Int));
        assert!(matches!(t, Type::Set(_)));
    }

    #[test]
    fn vector_type() {
        let t = Type::Vector(Box::new(Type::Int), 4);
        let (inner, n) = match &t {
            Type::Vector(i, n) => (i, n),
            _ => panic!(),
        };
        assert_eq!(**inner, Type::Int);
        assert_eq!(*n, 4);
    }

    #[test]
    fn future_type() {
        let t = Type::Future(Box::new(Type::Int));
        let inner = match &t {
            Type::Future(i) => i,
            _ => panic!(),
        };
        assert_eq!(**inner, Type::Int);
    }

    #[test]
    fn never_type() {
        assert_eq!(Type::Never, Type::Never);
    }

    #[test]
    fn is_move_type_ints() {
        assert!(!Type::Int.is_move_type());
        assert!(!Type::Bool.is_move_type());
    }

    #[test]
    fn is_move_type_complex() {
        assert!(Type::Tuple(vec![Type::Int, Type::Bool]).is_move_type());
        assert!(Type::List(Box::new(Type::Int)).is_move_type());
    }

    #[test]
    fn special_types() {
        assert_eq!(Type::Any, Type::Any);
        assert_eq!(Type::Null, Type::Null);
        assert_eq!(Type::Bytes, Type::Bytes);
    }

    #[test]
    fn trait_obj() {
        let t = Type::TraitObject("Display".into(), vec![Type::Int]);
        let (name, params) = match &t {
            Type::TraitObject(n, p) => (n, p),
            _ => panic!(),
        };
        assert_eq!(name, "Display");
        assert_eq!(params.len(), 1);
    }
}
