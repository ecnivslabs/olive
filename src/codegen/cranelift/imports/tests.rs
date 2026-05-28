use super::*;
use crate::mir::{Constant, Local, LocalDecl, MirFunction, Operand};
use crate::semantic::types::Type;
use cranelift::prelude::types;

fn make_func(locals: Vec<Type>) -> MirFunction {
    MirFunction {
        name: "test".into(),
        locals: locals
            .into_iter()
            .map(|ty| LocalDecl {
                ty,
                name: None,
                span: Default::default(),
                is_mut: false,
                is_owning: false,
            })
            .collect(),
        basic_blocks: vec![],
        arg_count: 0,
        vararg_idx: None,
        kwarg_idx: None,
        param_names: vec![],
        is_async: false,
    }
}

#[test]
fn test_cl_type_primitive() {
    assert_eq!(cl_type(&Type::Int), types::I64);
    assert_eq!(cl_type(&Type::U64), types::I64);
    assert_eq!(cl_type(&Type::Usize), types::I64);
    assert_eq!(cl_type(&Type::I32), types::I32);
    assert_eq!(cl_type(&Type::U32), types::I32);
    assert_eq!(cl_type(&Type::I16), types::I16);
    assert_eq!(cl_type(&Type::U16), types::I16);
    assert_eq!(cl_type(&Type::I8), types::I8);
    assert_eq!(cl_type(&Type::U8), types::I8);
    assert_eq!(cl_type(&Type::Bool), types::I8);
    assert_eq!(cl_type(&Type::Float), types::F64);
    assert_eq!(cl_type(&Type::F32), types::F32);
    assert_eq!(cl_type(&Type::Str), types::I64);
    assert_eq!(cl_type(&Type::PyObject), types::I64);
}

#[test]
fn test_cl_type_vector() {
    assert_eq!(
        cl_type(&Type::Vector(Box::new(Type::Float), 4)),
        types::F64.by(4).unwrap()
    );
    assert_eq!(
        cl_type(&Type::Vector(Box::new(Type::I32), 8)),
        types::I32.by(8).unwrap()
    );
}

#[test]
fn test_cl_type_unknown_defaults_i64() {
    assert_eq!(cl_type(&Type::Any), types::I64);
    assert_eq!(cl_type(&Type::Never), types::I64);
}

#[test]
fn test_map_builtin_to_runtime_len() {
    assert_eq!(
        map_builtin_to_runtime("len", &Type::Str),
        Some("__olive_str_len")
    );
    assert_eq!(
        map_builtin_to_runtime("len", &Type::List(Box::new(Type::Int))),
        Some("__olive_list_len")
    );
    assert_eq!(
        map_builtin_to_runtime("len", &Type::Int),
        Some("__olive_list_len")
    );
}

#[test]
fn test_map_builtin_to_runtime_print() {
    assert_eq!(
        map_builtin_to_runtime("print", &Type::Str),
        Some("__olive_print_str")
    );
    assert_eq!(
        map_builtin_to_runtime("print", &Type::Float),
        Some("__olive_print_float")
    );
    assert_eq!(
        map_builtin_to_runtime("print", &Type::Int),
        Some("__olive_print_int")
    );
    assert_eq!(
        map_builtin_to_runtime("print", &Type::Bool),
        Some("__olive_print_bool")
    );
}

#[test]
fn test_map_builtin_to_runtime_str() {
    assert_eq!(
        map_builtin_to_runtime("str", &Type::Str),
        Some("__olive_copy")
    );
    assert_eq!(
        map_builtin_to_runtime("str", &Type::Float),
        Some("__olive_float_to_str")
    );
    assert_eq!(
        map_builtin_to_runtime("str", &Type::Int),
        Some("__olive_str")
    );
}

#[test]
fn test_map_builtin_to_runtime_int() {
    assert_eq!(
        map_builtin_to_runtime("int", &Type::Float),
        Some("__olive_float_to_int")
    );
    assert_eq!(
        map_builtin_to_runtime("int", &Type::Str),
        Some("__olive_str_to_int")
    );
    assert_eq!(
        map_builtin_to_runtime("int", &Type::Int),
        Some("__olive_int")
    );
}

#[test]
fn test_map_builtin_to_runtime_float() {
    assert_eq!(
        map_builtin_to_runtime("float", &Type::Float),
        Some("__olive_copy_float")
    );
    assert_eq!(
        map_builtin_to_runtime("float", &Type::Int),
        Some("__olive_int_to_float")
    );
    assert_eq!(
        map_builtin_to_runtime("float", &Type::Str),
        Some("__olive_str_to_float")
    );
}

#[test]
fn test_map_builtin_to_runtime_bool() {
    assert_eq!(
        map_builtin_to_runtime("bool", &Type::Float),
        Some("__olive_bool_from_float")
    );
    assert_eq!(
        map_builtin_to_runtime("bool", &Type::Int),
        Some("__olive_bool")
    );
}

#[test]
fn test_map_builtin_to_runtime_iter_next() {
    assert_eq!(
        map_builtin_to_runtime("iter", &Type::Any),
        Some("__olive_iter")
    );
    assert_eq!(
        map_builtin_to_runtime("next", &Type::Any),
        Some("__olive_next")
    );
    assert_eq!(
        map_builtin_to_runtime("has_next", &Type::Any),
        Some("__olive_has_next")
    );
}

#[test]
fn test_map_builtin_to_runtime_keys_values_remove() {
    assert_eq!(
        map_builtin_to_runtime("keys", &Type::Any),
        Some("__olive_obj_keys")
    );
    assert_eq!(
        map_builtin_to_runtime("values", &Type::Any),
        Some("__olive_obj_values")
    );
    assert_eq!(
        map_builtin_to_runtime("remove", &Type::Any),
        Some("__olive_obj_remove")
    );
}

#[test]
fn test_map_builtin_to_runtime_unknown_name() {
    assert_eq!(map_builtin_to_runtime("nonexistent", &Type::Int), None);
}

#[test]
fn test_map_builtin_ref_deref() {
    assert_eq!(
        map_builtin_to_runtime("len", &Type::Ref(Box::new(Type::Str))),
        Some("__olive_str_len")
    );
    assert_eq!(
        map_builtin_to_runtime("len", &Type::MutRef(Box::new(Type::Str))),
        Some("__olive_str_len")
    );
}

#[test]
fn test_resolve_builtin_import_direct() {
    let f = make_func(vec![]);
    assert_eq!(
        resolve_builtin_import(&f, "__olive_print_int", &[]),
        Some("__olive_print_int")
    );
    assert_eq!(
        resolve_builtin_import(&f, "__olive_str_concat", &[]),
        Some("__olive_str_concat")
    );
    assert_eq!(
        resolve_builtin_import(&f, "__olive_panic", &[]),
        Some("__olive_panic")
    );
}

#[test]
fn test_resolve_builtin_import_ffi_errno() {
    let f = make_func(vec![]);
    assert_eq!(
        resolve_builtin_import(&f, "ffi_errno", &[]),
        Some("__olive_ffi_errno")
    );
}

#[test]
fn test_resolve_builtin_import_unknown() {
    let f = make_func(vec![]);
    assert_eq!(resolve_builtin_import(&f, "fully_unknown", &[]), None);
}

#[test]
fn test_resolve_builtin_import_print_str() {
    let f = make_func(vec![]);
    let args = [Operand::Constant(Constant::Str("hello".into()))];
    assert_eq!(
        resolve_builtin_import(&f, "print", &args),
        Some("__olive_print_str")
    );
}

#[test]
fn test_resolve_builtin_import_list_new() {
    let f = make_func(vec![]);
    assert_eq!(
        resolve_builtin_import(&f, "list_new", &[]),
        Some("__olive_list_new")
    );
}

#[test]
fn test_is_str_op_with_constant_str() {
    let f = make_func(vec![]);
    assert!(is_str_op(
        &f,
        &Operand::Constant(Constant::Str("test".into()))
    ));
    assert!(!is_str_op(&f, &Operand::Constant(Constant::Int(42))));
}

#[test]
fn test_is_str_op_with_local() {
    let f = make_func(vec![Type::Str, Type::Int]);
    assert!(is_str_op(&f, &Operand::Copy(Local(0))));
    assert!(!is_str_op(&f, &Operand::Copy(Local(1))));
}

#[test]
fn test_is_float_op_with_constant() {
    let f = make_func(vec![]);
    assert!(is_float_op(
        &f,
        &Operand::Constant(Constant::Float(0x3FF0000000000000))
    ));
    assert!(!is_float_op(&f, &Operand::Constant(Constant::Int(42))));
}

#[test]
fn test_is_float_op_with_local() {
    let f = make_func(vec![Type::Float, Type::Int]);
    assert!(is_float_op(&f, &Operand::Copy(Local(0))));
    assert!(!is_float_op(&f, &Operand::Copy(Local(1))));
}

#[test]
fn test_is_pyobj_op() {
    let f = make_func(vec![Type::PyObject, Type::Int]);
    assert!(is_pyobj_op(&f, &Operand::Copy(Local(0))));
    assert!(!is_pyobj_op(&f, &Operand::Copy(Local(1))));
    assert!(!is_pyobj_op(&f, &Operand::Constant(Constant::Int(0))));
}

#[test]
fn test_is_list_op() {
    let f = make_func(vec![Type::List(Box::new(Type::Int)), Type::Int]);
    assert!(is_list_op(&f, &Operand::Copy(Local(0))));
    assert!(!is_list_op(&f, &Operand::Copy(Local(1))));
}

#[test]
fn test_is_u64_op() {
    let f = make_func(vec![Type::U64, Type::Int]);
    assert!(is_u64_op(&f, &Operand::Copy(Local(0))));
    assert!(!is_u64_op(&f, &Operand::Copy(Local(1))));
}

#[test]
fn test_resolve_builtin_import_builtins_with_arg() {
    let f = make_func(vec![Type::Int]);
    let args = [Operand::Copy(Local(0))];
    assert_eq!(
        resolve_builtin_import(&f, "str", &args),
        Some("__olive_str")
    );
    assert_eq!(
        resolve_builtin_import(&f, "int", &args),
        Some("__olive_int")
    );
    assert_eq!(
        resolve_builtin_import(&f, "float", &args),
        Some("__olive_int_to_float")
    );
    assert_eq!(
        resolve_builtin_import(&f, "bool", &args),
        Some("__olive_bool")
    );
    assert_eq!(
        resolve_builtin_import(&f, "len", &args),
        Some("__olive_list_len")
    );
}

#[test]
fn test_scan_rvalue_imports_get_attr() {
    let mut needed = std::collections::HashSet::new();
    let f = make_func(vec![Type::Any]);
    scan_rvalue_imports(
        &f,
        &Rvalue::GetAttr(Operand::Copy(Local(0)), "x".into()),
        &mut needed,
    );
    assert!(needed.contains("__olive_obj_get_checked"));
}

#[test]
fn test_scan_rvalue_imports_get_tag() {
    let mut needed = std::collections::HashSet::new();
    let f = make_func(vec![Type::Int]);
    scan_rvalue_imports(&f, &Rvalue::GetTag(Operand::Copy(Local(0))), &mut needed);
    assert!(needed.contains("__olive_enum_tag"));
}
