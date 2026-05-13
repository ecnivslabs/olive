use crate::mir::{Constant, MirFunction, Operand, Rvalue, StatementKind};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::types;

pub(super) fn collect_needed_imports(
    functions: &[MirFunction],
) -> std::collections::HashSet<&'static str> {
    let mut needed = std::collections::HashSet::new();
    for func in functions {
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                match &stmt.kind {
                    StatementKind::Assign(_, rval) => scan_rvalue_imports(func, rval, &mut needed),
                    StatementKind::SetAttr(..) => {
                        needed.insert("__olive_obj_set");
                    }
                    StatementKind::SetIndex(..) => {
                        needed.insert("__olive_list_set");
                        needed.insert("__olive_obj_set");
                        needed.insert("__olive_set_index_any");
                    }
                    StatementKind::Drop(local) => {
                        let ty = &func.locals[local.0].ty;
                        if ty.is_move_type() {
                            match ty {
                                OliveType::Str => {
                                    needed.insert("__olive_free_str");
                                }
                                OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_) => {
                                    needed.insert("__olive_free_list");
                                }
                                OliveType::Dict(_, _) | OliveType::Struct(_) => {
                                    needed.insert("__olive_free_obj");
                                }
                                OliveType::Enum(_) => {
                                    needed.insert("__olive_free_enum");
                                }
                                _ => {
                                    needed.insert("__olive_free");
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    needed
}

pub(super) fn scan_rvalue_imports(
    func_mir: &MirFunction,
    rval: &Rvalue,
    needed: &mut std::collections::HashSet<&'static str>,
) {
    match rval {
        Rvalue::Call {
            func: Operand::Constant(Constant::Function(name)),
            args,
        } => {
            if let Some(r) = resolve_builtin_import(func_mir, name, args) {
                needed.insert(r);
            }
        }
        Rvalue::Call { .. } => {}
        Rvalue::BinaryOp(op, lhs, _) => {
            use crate::parser::BinOp::*;
            match op {
                Add => {
                    if is_str_op(func_mir, lhs) {
                        needed.insert("__olive_str_concat");
                    } else if is_list_op(func_mir, lhs) {
                        needed.insert("__olive_list_concat");
                    }
                }
                Eq => {
                    if is_str_op(func_mir, lhs) {
                        needed.insert("__olive_str_eq");
                    }
                }
                Pow => {
                    if is_float_op(func_mir, lhs) {
                        needed.insert("__olive_pow_float");
                    } else {
                        needed.insert("__olive_pow");
                    }
                }
                In => {
                    needed.insert("__olive_in_list");
                    needed.insert("__olive_in_obj");
                }
                NotIn => {
                    needed.insert("__olive_in_list");
                    needed.insert("__olive_in_obj");
                }
                _ => {}
            }
        }
        Rvalue::GetAttr(..) => {
            needed.insert("__olive_obj_get");
        }
        Rvalue::GetTag(..) => {
            needed.insert("__olive_enum_tag");
        }
        Rvalue::GetTypeId(..) => {
            needed.insert("__olive_enum_type_id");
        }
        Rvalue::GetIndex(obj, _) => {
            needed.insert("__olive_list_get");
            needed.insert("__olive_obj_get");
            needed.insert("__olive_get_index_any");
            if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                let ty = &func_mir.locals[loc.0].ty;
                if matches!(ty, OliveType::Str) {
                    needed.insert("__olive_str_get");
                } else if matches!(ty, OliveType::Enum(_) | OliveType::Union(_)) {
                    needed.insert("__olive_enum_get");
                }
            }
        }
        Rvalue::Aggregate(kind, _) => {
            use crate::mir::ir::AggregateKind;
            match kind {
                AggregateKind::Dict => {
                    needed.insert("__olive_obj_new");
                    needed.insert("__olive_obj_set");
                }
                AggregateKind::Set => {
                    needed.insert("__olive_list_new");
                    needed.insert("__olive_set_add");
                    needed.insert("__olive_set_new");
                }
                AggregateKind::EnumVariant(_, _) => {
                    needed.insert("__olive_enum_new");
                    needed.insert("__olive_enum_set");
                }
                _ => {
                    needed.insert("__olive_list_new");
                    needed.insert("__olive_list_append");
                    needed.insert("__olive_set_index_any");
                }
            }
        }
        _ => {}
    }
}

pub(super) fn resolve_builtin_import(
    func_mir: &MirFunction,
    name: &str,
    args: &[Operand],
) -> Option<&'static str> {
    if name.starts_with("__olive_") {
        return match name {
            "__olive_print_int" => Some("__olive_print_int"),
            "__olive_print_str" => Some("__olive_print_str"),
            "__olive_print_float" => Some("__olive_print_float"),
            "__olive_print_list" => Some("__olive_print_list"),
            "__olive_print_obj" => Some("__olive_print_obj"),
            "__olive_str" => Some("__olive_str"),
            "__olive_int" => Some("__olive_int"),
            "__olive_bool" => Some("__olive_bool"),
            "__olive_float" => Some("__olive_float"),
            "__olive_str_to_int" => Some("__olive_str_to_int"),
            "__olive_str_to_float" => Some("__olive_str_to_float"),
            "__olive_float_to_int" => Some("__olive_float_to_int"),
            "__olive_float_to_str" => Some("__olive_float_to_str"),
            "__olive_int_to_float" => Some("__olive_int_to_float"),
            "__olive_bool_from_float" => Some("__olive_bool_from_float"),
            "__olive_copy" => Some("__olive_copy"),
            "__olive_copy_float" => Some("__olive_copy_float"),
            "__olive_list_new" => Some("__olive_list_new"),
            "__olive_list_get" => Some("__olive_list_get"),
            "__olive_list_set" => Some("__olive_list_set"),
            "__olive_list_append" => Some("__olive_list_append"),
            "__olive_str_len" => Some("__olive_str_len"),
            "__olive_list_len" => Some("__olive_list_len"),
            "__olive_get_index_any" => Some("__olive_get_index_any"),
            "__olive_set_index_any" => Some("__olive_set_index_any"),
            "__olive_free_any" => Some("__olive_free_any"),
            "__olive_str_get" => Some("__olive_str_get"),
            "__olive_str_concat" => Some("__olive_str_concat"),
            "__olive_list_concat" => Some("__olive_list_concat"),
            "__olive_str_eq" => Some("__olive_str_eq"),
            "__olive_obj_new" => Some("__olive_obj_new"),
            "__olive_obj_get" => Some("__olive_obj_get"),
            "__olive_obj_set" => Some("__olive_obj_set"),
            "__olive_pow" => Some("__olive_pow"),
            "__olive_in_list" => Some("__olive_in_list"),
            "__olive_in_obj" => Some("__olive_in_obj"),
            "__olive_set_add" => Some("__olive_set_add"),
            "__olive_set_new" => Some("__olive_set_new"),
            "__olive_free" => Some("__olive_free"),
            "__olive_free_str" => Some("__olive_free_str"),
            "__olive_free_list" => Some("__olive_free_list"),
            "__olive_free_obj" => Some("__olive_free_obj"),
            "__olive_cache_get" => Some("__olive_cache_get"),
            "__olive_cache_has" => Some("__olive_cache_has"),
            "__olive_cache_set" => Some("__olive_cache_set"),
            "__olive_cache_has_tuple" => Some("__olive_cache_has_tuple"),
            "__olive_cache_get_tuple" => Some("__olive_cache_get_tuple"),
            "__olive_cache_set_tuple" => Some("__olive_cache_set_tuple"),
            "__olive_memo_get" => Some("__olive_memo_get"),
            "__olive_iter" => Some("__olive_iter"),
            "__olive_next" => Some("__olive_next"),
            "__olive_has_next" => Some("__olive_has_next"),
            "__olive_time_now" => Some("__olive_time_now"),
            "__olive_time_sleep" => Some("__olive_time_sleep"),
            "__olive_enum_new" => Some("__olive_enum_new"),
            "__olive_enum_tag" => Some("__olive_enum_tag"),
            "__olive_enum_type_id" => Some("__olive_enum_type_id"),
            "__olive_enum_get" => Some("__olive_enum_get"),
            "__olive_enum_set" => Some("__olive_enum_set"),
            "__olive_str_char" => Some("__olive_str_char"),
            "__olive_str_slice" => Some("__olive_str_slice"),
            "__olive_make_future" => Some("__olive_make_future"),
            "__olive_await" => Some("__olive_await"),
            "__olive_spawn_task" => Some("__olive_spawn_task"),
            "__olive_free_future" => Some("__olive_free_future"),
            "__olive_alloc" => Some("__olive_alloc"),
            "__olive_async_file_read" => Some("__olive_async_file_read"),
            "__olive_async_file_write" => Some("__olive_async_file_write"),
            "__olive_gather" => Some("__olive_gather"),
            "__olive_select" => Some("__olive_select"),
            "__olive_cancel_future" => Some("__olive_cancel_future"),
            "__olive_sm_poll" => Some("__olive_sm_poll"),
            "__olive_random_seed" => Some("__olive_random_seed"),
            "__olive_random_get" => Some("__olive_random_get"),
            "__olive_random_int" => Some("__olive_random_int"),
            "__olive_math_sin" => Some("__olive_math_sin"),
            "__olive_math_cos" => Some("__olive_math_cos"),
            "__olive_math_tan" => Some("__olive_math_tan"),
            "__olive_math_asin" => Some("__olive_math_asin"),
            "__olive_math_acos" => Some("__olive_math_acos"),
            "__olive_math_atan" => Some("__olive_math_atan"),
            "__olive_math_atan2" => Some("__olive_math_atan2"),
            "__olive_math_log" => Some("__olive_math_log"),
            "__olive_math_log10" => Some("__olive_math_log10"),
            "__olive_math_exp" => Some("__olive_math_exp"),
            "__olive_net_tcp_connect" => Some("__olive_net_tcp_connect"),
            "__olive_net_tcp_send" => Some("__olive_net_tcp_send"),
            "__olive_net_tcp_recv" => Some("__olive_net_tcp_recv"),
            "__olive_net_tcp_close" => Some("__olive_net_tcp_close"),
            "__olive_http_get" => Some("__olive_http_get"),
            "__olive_http_post" => Some("__olive_http_post"),
            "__olive_file_read" => Some("__olive_file_read"),
            "__olive_file_write" => Some("__olive_file_write"),
            _ => None,
        };
    }
    match name {
        "print" | "str" | "int" | "float" | "bool" | "iter" | "next" | "has_next" | "len"
            if !args.is_empty() =>
        {
            let arg_type = match &args[0] {
                Operand::Constant(Constant::Str(_)) => OliveType::Str,
                Operand::Constant(Constant::Float(_)) => OliveType::Float,
                Operand::Copy(l) | Operand::Move(l) => func_mir.locals[l.0].ty.clone(),
                _ => OliveType::Int,
            };
            map_builtin_to_runtime(name, &arg_type)
        }

        "list_new" => Some("__olive_list_new"),
        _ => None,
    }
}

pub(super) fn map_builtin_to_runtime(name: &str, arg_ty: &OliveType) -> Option<&'static str> {
    let mut current_ty = arg_ty;
    while let OliveType::Ref(inner) | OliveType::MutRef(inner) = current_ty {
        current_ty = inner;
    }

    match name {
        "len" => match current_ty {
            OliveType::Str => Some("__olive_str_len"),
            OliveType::Dict(_, _) | OliveType::Struct(_) | OliveType::Any => {
                Some("__olive_obj_len")
            }
            _ => Some("__olive_list_len"),
        },
        "print" => match current_ty {
            OliveType::Str => Some("__olive_print_str"),
            OliveType::Float => Some("__olive_print_float"),
            OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_) => {
                Some("__olive_print_list")
            }
            OliveType::Dict(_, _) | OliveType::Struct(_) => Some("__olive_print_obj"),
            _ => Some("__olive_print_int"),
        },
        "str" => match current_ty {
            OliveType::Str => Some("__olive_copy"),
            OliveType::Float => Some("__olive_float_to_str"),
            _ => Some("__olive_str"),
        },
        "int" => match current_ty {
            OliveType::Float => Some("__olive_float_to_int"),
            OliveType::Str => Some("__olive_str_to_int"),
            _ => Some("__olive_int"),
        },
        "float" => match current_ty {
            OliveType::Float => Some("__olive_copy_float"),
            OliveType::Int => Some("__olive_int_to_float"),
            OliveType::Str => Some("__olive_str_to_float"),
            _ => Some("__olive_float"),
        },
        "bool" => {
            if *current_ty == OliveType::Float {
                Some("__olive_bool_from_float")
            } else {
                Some("__olive_bool")
            }
        }
        "iter" => Some("__olive_iter"),
        "next" => Some("__olive_next"),
        "has_next" => Some("__olive_has_next"),
        _ => None,
    }
}

pub(super) fn is_str_op(func_mir: &MirFunction, op: &Operand) -> bool {
    match op {
        Operand::Constant(Constant::Str(_)) => true,
        Operand::Copy(loc) | Operand::Move(loc) => func_mir.locals[loc.0].ty == OliveType::Str,
        _ => false,
    }
}

pub(super) fn is_float_op(func_mir: &MirFunction, op: &Operand) -> bool {
    match op {
        Operand::Constant(Constant::Float(_)) => true,
        Operand::Copy(loc) | Operand::Move(loc) => func_mir.locals[loc.0].ty == OliveType::Float,
        _ => false,
    }
}

pub(super) fn is_list_op(func_mir: &MirFunction, op: &Operand) -> bool {
    match op {
        Operand::Copy(loc) | Operand::Move(loc) => {
            let ty = &func_mir.locals[loc.0].ty;
            matches!(
                ty,
                OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_)
            )
        }
        _ => false,
    }
}

pub(super) fn cl_type(ty: &OliveType) -> cranelift::prelude::Type {
    match ty {
        OliveType::Int | OliveType::Bool => types::I64,
        OliveType::Float => types::F64,
        OliveType::Vector(inner, width) => match &**inner {
            OliveType::Int => types::I64.by(*width as u32).expect("invalid vector width"),
            OliveType::Float => types::F64.by(*width as u32).expect("invalid vector width"),
            _ => types::I64,
        },
        _ => types::I64,
    }
}
