use crate::mir::{Constant, MirFunction, Operand, Rvalue, StatementKind};
use crate::semantic::types::Type as OliveType;

pub(super) fn collect_needed_imports(
    functions: &[MirFunction],
) -> std::collections::HashSet<&'static str> {
    let mut needed = std::collections::HashSet::new();
    for func in functions {
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                match &stmt.kind {
                    StatementKind::Assign(local, rval) => {
                        scan_rvalue_imports(func, rval, &mut needed);
                        if matches!(func.locals[local.0].ty, OliveType::Float | OliveType::F32)
                            && let Rvalue::Use(Operand::Copy(src) | Operand::Move(src)) = rval
                            && matches!(func.locals[src.0].ty, OliveType::PyObject)
                        {
                            needed.insert("__olive_py_to_float");
                        }
                    }
                    StatementKind::SetAttr(_, _, val_op) => {
                        needed.insert("__olive_obj_set");
                        needed.insert("__olive_py_setattr");
                        if let Operand::Copy(src) = val_op
                            && matches!(func.locals[src.0].ty, OliveType::PyObject)
                        {
                            needed.insert("__olive_py_copy_ref");
                        }
                    }
                    StatementKind::SetIndex(..) => {
                        needed.insert("__olive_list_set");
                        needed.insert("__olive_obj_set");
                        needed.insert("__olive_set_index_any");
                        needed.insert("__olive_bounds_fail");
                        needed.insert("__olive_nil_index_fail");
                    }
                    StatementKind::Drop(local) => {
                        let ty = &func.locals[local.0].ty;
                        if ty.is_move_type() {
                            match ty {
                                OliveType::Str => {
                                    needed.insert("__olive_free_str");
                                }
                                OliveType::Bytes => {
                                    needed.insert("__olive_buf_free");
                                }
                                OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_) => {
                                    needed.insert("__olive_free_list");
                                }
                                OliveType::Struct(_, _) => {
                                    needed.insert("__olive_free_struct");
                                    needed.insert("__olive_free_obj");
                                }
                                OliveType::Dict(_, _) => {
                                    needed.insert("__olive_free_obj");
                                }
                                OliveType::Enum(_, _) => {
                                    needed.insert("__olive_free_enum");
                                }
                                OliveType::PyObject => {
                                    needed.insert("__olive_py_decref");
                                }
                                OliveType::Union(_) | OliveType::Any => {
                                    needed.insert("__olive_free_any");
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
                // Reading errno requires the post-call snapshot helper.
                if r == "__olive_ffi_errno" {
                    needed.insert("__olive_ffi_snapshot_errno");
                }
            }
        }
        Rvalue::Call { .. } => {}
        Rvalue::BinaryOp(op, lhs, rhs) => {
            use crate::parser::BinOp::*;
            // An `Any` operand routes arithmetic and comparison to the runtime
            // kind-dispatching helpers, so make all of them (and what they call)
            // available.
            if is_any_op(func_mir, lhs) || is_any_op(func_mir, rhs) {
                for f in [
                    "__olive_any_add",
                    "__olive_any_sub",
                    "__olive_any_mul",
                    "__olive_any_div",
                    "__olive_any_mod",
                    "__olive_any_lt",
                    "__olive_any_le",
                    "__olive_any_gt",
                    "__olive_any_ge",
                    "__olive_any_eq",
                    "__olive_any_ne",
                    "__olive_str_concat",
                    "__olive_list_concat",
                    "__olive_box_float",
                    "__olive_unbox_float",
                ] {
                    needed.insert(f);
                }
            }
            match op {
                Add => {
                    let mut is_pyobj = false;
                    let mut check_op = |op: &Operand| match op {
                        Operand::Copy(loc) | Operand::Move(loc)
                            if func_mir.locals[loc.0].ty == OliveType::PyObject =>
                        {
                            is_pyobj = true;
                        }
                        _ => {}
                    };
                    check_op(lhs);
                    check_op(rhs);
                    if is_pyobj {
                        needed.insert("__olive_py_add");
                        needed.insert("__olive_py_from_float");
                        needed.insert("__olive_py_from_int");
                    } else if is_str_op(func_mir, lhs) {
                        needed.insert("__olive_str_concat");
                    } else if is_list_op(func_mir, lhs) {
                        needed.insert("__olive_list_concat");
                    }
                }
                Sub | Mul | Div | Mod => {
                    let mut is_pyobj = false;
                    let mut check_op = |op: &Operand| match op {
                        Operand::Copy(loc) | Operand::Move(loc)
                            if func_mir.locals[loc.0].ty == OliveType::PyObject =>
                        {
                            is_pyobj = true;
                        }
                        _ => {}
                    };
                    check_op(lhs);
                    check_op(rhs);
                    if is_pyobj {
                        needed.insert("__olive_py_from_float");
                        needed.insert("__olive_py_from_int");
                        match op {
                            crate::parser::BinOp::Sub => {
                                needed.insert("__olive_py_sub");
                            }
                            crate::parser::BinOp::Mul => {
                                needed.insert("__olive_py_mul");
                            }
                            crate::parser::BinOp::Div => {
                                needed.insert("__olive_py_div");
                            }
                            crate::parser::BinOp::Mod => {
                                needed.insert("__olive_py_mod");
                            }
                            _ => {}
                        }
                    } else if matches!(op, Div | Mod) && !is_float_op(func_mir, lhs) {
                        needed.insert("__olive_div_zero_fail");
                    }
                }
                Eq => {
                    let mut is_str = false;
                    let mut is_pyobj = false;
                    let mut check_op = |op: &Operand| match op {
                        Operand::Constant(Constant::Str(_)) => is_str = true,
                        Operand::Copy(loc) | Operand::Move(loc) => {
                            let ty = &func_mir.locals[loc.0].ty;
                            if *ty == OliveType::Str {
                                is_str = true;
                            }
                            if *ty == OliveType::PyObject {
                                is_pyobj = true;
                            }
                        }
                        _ => {}
                    };
                    check_op(lhs);
                    check_op(rhs);

                    if is_str {
                        needed.insert("__olive_str_eq");
                    } else if is_pyobj {
                        needed.insert("__olive_py_eq");
                    }
                }
                Lt | LtEq | Gt | GtEq | NotEq => {
                    let is_pyobj = is_pyobj_op(func_mir, lhs) || is_pyobj_op(func_mir, rhs);
                    if is_pyobj {
                        needed.insert("__olive_py_from_float");
                        needed.insert("__olive_py_from_int");
                        match op {
                            crate::parser::BinOp::Lt => {
                                needed.insert("__olive_py_lt");
                            }
                            crate::parser::BinOp::LtEq => {
                                needed.insert("__olive_py_le");
                            }
                            crate::parser::BinOp::Gt => {
                                needed.insert("__olive_py_gt");
                            }
                            crate::parser::BinOp::GtEq => {
                                needed.insert("__olive_py_ge");
                            }
                            crate::parser::BinOp::NotEq => {
                                needed.insert("__olive_py_ne");
                            }
                            _ => {}
                        }
                    }
                }
                BitOr => {
                    let mut is_pyobj = false;
                    let mut check_op = |op: &Operand| match op {
                        Operand::Copy(loc) | Operand::Move(loc)
                            if func_mir.locals[loc.0].ty == OliveType::PyObject =>
                        {
                            is_pyobj = true;
                        }
                        _ => {}
                    };
                    check_op(lhs);
                    check_op(rhs);
                    if is_pyobj {
                        needed.insert("__olive_py_bitor");
                    }
                }
                Pow => {
                    let mut is_pyobj = false;
                    let mut check_op = |op: &Operand| match op {
                        Operand::Copy(loc) | Operand::Move(loc)
                            if func_mir.locals[loc.0].ty == OliveType::PyObject =>
                        {
                            is_pyobj = true;
                        }
                        _ => {}
                    };
                    check_op(lhs);
                    check_op(rhs);
                    if is_pyobj {
                        needed.insert("__olive_py_pow");
                        needed.insert("__olive_py_from_float");
                        needed.insert("__olive_py_from_int");
                    } else if is_float_op(func_mir, lhs) {
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
            needed.insert("__olive_py_getattr");
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
            needed.insert("__olive_bounds_fail");
            needed.insert("__olive_nil_index_fail");
            needed.insert("__olive_str_get_checked");
            if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                let ty = &func_mir.locals[loc.0].ty;
                if matches!(ty, OliveType::Str) {
                    needed.insert("__olive_str_get");
                } else if matches!(ty, OliveType::Enum(_, _) | OliveType::Union(_)) {
                    needed.insert("__olive_enum_get");
                } else if matches!(ty, OliveType::PyObject) {
                    needed.insert("__olive_py_getitem");
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
        Rvalue::UnaryOp(op, operand) => {
            use crate::parser::UnaryOp::*;
            if matches!(op, Neg | Not)
                && let Operand::Copy(src) | Operand::Move(src) = operand
                && matches!(func_mir.locals[src.0].ty, OliveType::PyObject)
            {
                needed.insert("__olive_py_to_int");
                if matches!(op, Neg) {
                    needed.insert("__olive_py_from_int");
                }
            }
        }
        Rvalue::Cast(op, target_ty) => {
            if let Operand::Copy(src) | Operand::Move(src) = op
                && matches!(func_mir.locals[src.0].ty, OliveType::PyObject)
            {
                if matches!(target_ty, OliveType::Float | OliveType::F32) {
                    needed.insert("__olive_py_to_float");
                } else if !matches!(
                    target_ty,
                    OliveType::PyObject | OliveType::Float | OliveType::F32
                ) {
                    needed.insert("__olive_py_to_int");
                }
            }
        }
        _ => {}
    }
}

mod builtins;
#[cfg(test)]
mod tests;

pub(super) use builtins::{
    cl_type, is_any_op, is_float_op, is_list_op, is_pyobj_op, is_str_op, is_u64_op,
    map_builtin_to_runtime, resolve_builtin_import,
};
