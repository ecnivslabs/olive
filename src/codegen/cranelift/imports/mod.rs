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
                        // A PyObject struct field overwrite releases the old value it held.
                        needed.insert("__olive_py_decref");
                        if let Operand::Copy(src) = val_op
                            && matches!(func.locals[src.0].ty, OliveType::PyObject)
                        {
                            needed.insert("__olive_py_copy_ref");
                        }
                    }
                    StatementKind::SetIndex(_, _, val_op, _) => {
                        needed.insert("__olive_list_set");
                        needed.insert("__olive_obj_set");
                        needed.insert("__olive_obj_set_typed");
                        needed.insert("__olive_set_index_any");
                        needed.insert("__olive_bounds_fail");
                        needed.insert("__olive_nil_index_fail");
                        if let Operand::Copy(src) = val_op
                            && matches!(func.locals[src.0].ty, OliveType::PyObject)
                        {
                            needed.insert("__olive_py_copy_ref");
                        }
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
                                    needed.insert("__olive_free_typed");
                                }
                                OliveType::Struct(_, _, _) => {
                                    needed.insert("__olive_free_struct");
                                    needed.insert("__olive_free_obj");
                                    needed.insert("__olive_free_typed");
                                }
                                OliveType::Dict(_, _) => {
                                    needed.insert("__olive_free_obj");
                                    needed.insert("__olive_free_typed");
                                }
                                OliveType::Enum(_, _) => {
                                    needed.insert("__olive_free_enum");
                                    needed.insert("__olive_free_typed");
                                }
                                OliveType::PyObject | OliveType::PyNamed(_, _) => {
                                    needed.insert("__olive_py_decref");
                                }
                                OliveType::TraitObject(..) => {
                                    needed.insert("__olive_free_fatptr");
                                }
                                // The closure record's per-instance descriptor
                                // is loaded from the value itself at runtime
                                // (`translate.rs`'s `Drop` arm), not looked up
                                // statically, but the free entry point is the
                                // same `__olive_free_typed` every other typed
                                // drop uses.
                                OliveType::Fn(..) => {
                                    needed.insert("__olive_free_typed");
                                }
                                OliveType::Any => {
                                    needed.insert("__olive_free_any");
                                }
                                // `T | None` may dispatch to any of `T`'s own free
                                // functions (see `free_func_name_for_type`); register
                                // the whole set since the member isn't known here.
                                OliveType::Union(_) => {
                                    needed.insert("__olive_free_any");
                                    needed.insert("__olive_free_union_member");
                                    needed.insert("__olive_free_typed");
                                    needed.insert("__olive_free_str");
                                    needed.insert("__olive_buf_free");
                                    needed.insert("__olive_free_list");
                                    needed.insert("__olive_free_struct");
                                    needed.insert("__olive_free_obj");
                                    needed.insert("__olive_free_enum");
                                    needed.insert("__olive_py_decref");
                                    needed.insert("__olive_free");
                                }
                                _ => {
                                    needed.insert("__olive_free");
                                }
                            }
                        }
                    }
                    StatementKind::GenCheck { value, .. } => {
                        needed.insert("__olive_stale_ref_fail");
                        if crate::mir::optimizations::gencheck::str_backed(&func.locals[value.0].ty)
                        {
                            needed.insert("__olive_str_gen_stale");
                        }
                        if crate::mir::optimizations::gencheck::struct_backed(
                            &func.locals[value.0].ty,
                        ) {
                            needed.insert("__olive_struct_gen_stale");
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    for func in functions {
        if func.is_async
            && func
                .locals
                .iter()
                .skip(1)
                .take(func.arg_count)
                .any(|l| l.ty.is_move_type())
        {
            needed.insert("__olive_copy_typed");
        }
    }
    needed.insert("__olive_clear_typed");
    needed.insert("__olive_list_new_reuse");
    needed.insert("__olive_dict_new_reuse");
    needed.insert("__olive_set_new_reuse");
    needed.insert("__olive_enum_new_reuse");
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
            if (name == "print" || name == "str")
                && let [Operand::Copy(l) | Operand::Move(l)] = args.as_slice()
                && needs_type_descriptor(&func_mir.locals[l.0].ty)
            {
                needed.insert(if name == "print" {
                    "__olive_print_typed"
                } else {
                    "__olive_format_typed"
                });
            }
            if name == "__olive_write_any"
                && let [Operand::Copy(l) | Operand::Move(l)] = args.as_slice()
                && needs_type_descriptor(&func_mir.locals[l.0].ty)
            {
                needed.insert("__olive_write_typed");
            }
            if name == "__olive_copy_typed" {
                needed.insert("__olive_copy_typed");
            }
            if name == "__olive_relocate_typed" {
                needed.insert("__olive_relocate_typed");
            }
            if name == "__olive_eq_typed" {
                needed.insert("__olive_eq_typed");
            }
            if name == "__olive_list_concat_typed" {
                needed.insert("__olive_list_concat_typed");
            }
            if name == "__olive_list_concat_move" {
                needed.insert("__olive_list_concat_move");
            }
            if name == "__olive_list_push" {
                needed.insert("__olive_list_push");
            }
            if name == "__olive_list_getslice_typed" {
                needed.insert("__olive_list_getslice_typed");
            }
            if name == "__olive_list_extend_typed" {
                needed.insert("__olive_list_extend_typed");
            }
            if name == "__olive_set_add_typed" {
                needed.insert("__olive_set_add_typed");
            }
            if name == "__olive_set_remove_typed" {
                needed.insert("__olive_set_remove_typed");
            }
            if name == "__olive_set_contains_typed" {
                needed.insert("__olive_set_contains_typed");
            }
            if name == "__olive_obj_get_typed" {
                needed.insert("__olive_obj_get_typed");
            }
            if name == "__olive_obj_get_default_typed" {
                needed.insert("__olive_obj_get_default_typed");
            }
            if name == "__olive_list_count_typed" {
                needed.insert("__olive_list_count_typed");
            }
            if name == "__olive_list_index_typed" {
                needed.insert("__olive_list_index_typed");
            }
            if name == "__olive_obj_update_typed" {
                needed.insert("__olive_obj_update_typed");
            }
            if name == "__olive_set_remove_checked_typed" {
                needed.insert("__olive_set_remove_checked_typed");
            }
            if name == "__olive_obj_pop_checked_typed" {
                needed.insert("__olive_obj_pop_checked_typed");
            }
            if name == "__olive_obj_pop_default_typed" {
                needed.insert("__olive_obj_pop_default_typed");
            }
            if name == "__olive_obj_setdefault_typed" {
                needed.insert("__olive_obj_setdefault_typed");
            }
        }
        Rvalue::Call { .. } => {}
        Rvalue::GenOf(Operand::Copy(l) | Operand::Move(l))
            if crate::mir::optimizations::gencheck::str_backed(&func_mir.locals[l.0].ty) =>
        {
            needed.insert("__olive_str_gen_of");
        }
        Rvalue::GenOf(Operand::Copy(l) | Operand::Move(l))
            if crate::mir::optimizations::gencheck::struct_backed(&func_mir.locals[l.0].ty) =>
        {
            needed.insert("__olive_struct_gen_of");
        }
        Rvalue::BinaryOp(op, lhs, rhs) => {
            use crate::parser::BinOp::*;
            // An `Any` operand routes arithmetic and comparison to the runtime
            // kind-dispatching helpers, so make all of them (and what they call)
            // available.
            if is_any_op(func_mir, lhs) || is_any_op(func_mir, rhs) {
                for f in [
                    "__olive_any_add",
                    "__olive_any_add_profiled",
                    "__olive_any_sub",
                    "__olive_any_sub_profiled",
                    "__olive_any_mul",
                    "__olive_any_mul_profiled",
                    "__olive_any_div",
                    "__olive_any_div_profiled",
                    "__olive_any_mod",
                    "__olive_any_mod_profiled",
                    "__olive_any_lt",
                    "__olive_any_lt_profiled",
                    "__olive_any_le",
                    "__olive_any_le_profiled",
                    "__olive_any_gt",
                    "__olive_any_gt_profiled",
                    "__olive_any_ge",
                    "__olive_any_ge_profiled",
                    "__olive_any_eq",
                    "__olive_any_eq_profiled",
                    "__olive_any_ne",
                    "__olive_any_ne_profiled",
                    "__olive_str_concat",
                    "__olive_list_concat",
                    "__olive_box_int",
                    "__olive_unbox_int",
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
                    } else if !is_float_op(func_mir, lhs) {
                        needed.insert("__olive_overflow_fail");
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
                    } else if !is_float_op(func_mir, lhs) {
                        needed.insert("__olive_overflow_fail");
                        if matches!(op, Div | Mod) {
                            needed.insert("__olive_div_zero_fail");
                        }
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
                        // Codegen boxes a non-Python operand before the compare, so
                        // the conversion helpers must be imported alongside `py_eq`,
                        // exactly as the ordered comparisons below do.
                        needed.insert("__olive_py_from_float");
                        needed.insert("__olive_py_from_int");
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
                    needed.insert("__olive_in_list_typed");
                    needed.insert("__olive_in_obj_typed");
                }
                NotIn => {
                    needed.insert("__olive_in_list");
                    needed.insert("__olive_in_obj");
                    needed.insert("__olive_in_list_typed");
                    needed.insert("__olive_in_obj_typed");
                }
                _ => {}
            }
        }
        Rvalue::GetAttr(..) => {
            needed.insert("__olive_obj_get_checked");
            needed.insert("__olive_py_getattr");
        }
        Rvalue::GetTag(..) => {
            needed.insert("__olive_enum_tag");
        }
        Rvalue::GetTypeId(..) => {
            needed.insert("__olive_enum_type_id");
        }
        Rvalue::GetIndex(obj, _, _) => {
            needed.insert("__olive_list_get");
            needed.insert("__olive_obj_get_checked");
            needed.insert("__olive_obj_get_checked_typed");
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
        Rvalue::Aggregate(kind, ops) => {
            use crate::mir::ir::AggregateKind;
            match kind {
                AggregateKind::Dict => {
                    needed.insert("__olive_obj_new");
                    needed.insert("__olive_obj_set");
                    needed.insert("__olive_obj_set_typed");
                }
                AggregateKind::Set => {
                    needed.insert("__olive_list_new");
                    needed.insert("__olive_set_add");
                    needed.insert("__olive_set_add_typed");
                    needed.insert("__olive_set_new");
                }
                AggregateKind::EnumVariant(_, _) => {
                    needed.insert("__olive_enum_new");
                    needed.insert("__olive_enum_set");
                }
                AggregateKind::FatPtr => {
                    needed.insert("__olive_fatptr_alloc");
                    needed.insert("__olive_free_fatptr");
                }
                _ => {
                    needed.insert("__olive_list_new");
                    needed.insert("__olive_list_append");
                    needed.insert("__olive_set_index_any");
                }
            }
            if !matches!(kind, AggregateKind::FatPtr)
                && ops.iter().any(|op| {
                    matches!(op, Operand::Copy(src) if matches!(func_mir.locals[src.0].ty, OliveType::PyObject))
                })
            {
                needed.insert("__olive_py_copy_ref");
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
            // Narrowing a multi-member union back to its struct member peels
            // the box put on at the coercion site (translate_rvalue's Cast).
            if matches!(target_ty, OliveType::Struct(_, _, _))
                && let Operand::Copy(src) | Operand::Move(src) = op
                && matches!(&func_mir.locals[src.0].ty, OliveType::Union(members)
                    if members.iter().filter(|m| !matches!(m, OliveType::Null)).count() > 1)
            {
                needed.insert("__olive_struct_unbox");
            }
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
            if *target_ty == OliveType::Str
                && let Operand::Copy(src) | Operand::Move(src) = op
            {
                match func_mir.locals[src.0].ty {
                    OliveType::Int
                    | OliveType::I8
                    | OliveType::I16
                    | OliveType::I32
                    | OliveType::U8
                    | OliveType::U16
                    | OliveType::U32
                    | OliveType::U64
                    | OliveType::Usize => {
                        needed.insert("__olive_str");
                    }
                    OliveType::Float | OliveType::F32 => {
                        needed.insert("__olive_float_to_str");
                    }
                    OliveType::Bool => {
                        needed.insert("__olive_bool_to_str");
                    }
                    _ => {}
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
    cl_type, concrete_ty, drop_descriptor_type, is_any_op, is_float_op, is_list_op, is_pyobj_op,
    is_str_op, is_u64_op, map_builtin_to_runtime, needs_structural_key, needs_type_descriptor,
    operand_static_type, resolve_builtin_import, type_descriptor, typed_zero,
};
