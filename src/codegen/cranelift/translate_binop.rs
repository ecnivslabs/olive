use super::CraneliftCodegen;
use super::imports::{is_any_op, is_float_op, is_list_op, is_pyobj_op, is_str_op, is_u64_op};
use crate::mir::{Constant, Local, MirFunction, Operand};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// `Any` inline-scalar tag bits (`std_lib/src/boxed.rs`, kept in sync by hand --
/// the two crates share no common dependency to hold this in one place). Only
/// `TAG_INT` matters here: the guarded fast path below only ever fires for two
/// operands that are already-inline 61-bit integers.
const ANY_TAG_MASK: i64 = 7;
const ANY_TAG_INT: i64 = 2;
const ANY_INT_MIN: i64 = -(1 << 60);
const ANY_INT_MAX: i64 = (1 << 60) - 1;

impl<M: Module> CraneliftCodegen<M> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_binop(
        func_mir: &MirFunction,
        module: &mut M,
        func_ids: &HashMap<String, FuncId>,
        string_ids: &HashMap<String, DataId>,
        struct_fields: &HashMap<String, Vec<String>>,
        field_types: &HashMap<(String, String), OliveType>,
        enum_defs: &HashMap<String, Vec<(String, Vec<OliveType>)>>,
        any_add_site_ids: &[DataId],
        any_add_site_cursor: &mut usize,
        specialize_sites: &HashSet<usize>,
        builder: &mut FunctionBuilder,
        vars: &HashMap<Local, Variable>,
        op: &crate::parser::BinOp,
        lhs: &Operand,
        rhs: &Operand,
        loc_id: Option<DataId>,
        checked: bool,
    ) -> Value {
        let l = Self::translate_operand(builder, lhs, vars, string_ids, module, func_ids);
        let r = Self::translate_operand(builder, rhs, vars, string_ids, module, func_ids);

        // f32 locals and untyped float constants (always emitted as f64 by
        // `translate_operand`) can reach the same binop with mismatched
        // cranelift widths -- `fadd`/`fcmp`/etc require identical operand
        // types, so an unwidened f32 operand here silently produced wrong
        // results (e.g. `f32_var < 10.0` always false) rather than a crash.
        let l_ty = builder.func.dfg.value_type(l);
        let r_ty = builder.func.dfg.value_type(r);
        let (l, r) = if l_ty == types::F32 && r_ty == types::F64 {
            (builder.ins().fpromote(types::F64, l), r)
        } else if l_ty == types::F64 && r_ty == types::F32 {
            (l, builder.ins().fpromote(types::F64, r))
        } else {
            (l, r)
        };

        use crate::parser::BinOp::*;

        let is_py = is_pyobj_op(func_mir, lhs) || is_pyobj_op(func_mir, rhs);
        if is_py && matches!(op, Add | Sub | Mul | Div | Mod | Pow) {
            let (l_val, l_coerced) = if is_float_op(func_mir, lhs) {
                let func_id = func_ids
                    .get("__olive_py_from_float")
                    .expect("missing __olive_py_from_float");
                let local_func = module.declare_func_in_func(*func_id, builder.func);
                let inst = builder.ins().call(local_func, &[l]);
                (builder.inst_results(inst)[0], true)
            } else if !is_pyobj_op(func_mir, lhs) {
                let func_id = func_ids
                    .get("__olive_py_from_int")
                    .expect("missing __olive_py_from_int");
                let local_func = module.declare_func_in_func(*func_id, builder.func);
                let inst = builder.ins().call(local_func, &[l]);
                (builder.inst_results(inst)[0], true)
            } else {
                (l, false)
            };

            let (r_val, r_coerced) = if is_float_op(func_mir, rhs) {
                let func_id = func_ids
                    .get("__olive_py_from_float")
                    .expect("missing __olive_py_from_float");
                let local_func = module.declare_func_in_func(*func_id, builder.func);
                let inst = builder.ins().call(local_func, &[r]);
                (builder.inst_results(inst)[0], true)
            } else if !is_pyobj_op(func_mir, rhs) {
                let func_id = func_ids
                    .get("__olive_py_from_int")
                    .expect("missing __olive_py_from_int");
                let local_func = module.declare_func_in_func(*func_id, builder.func);
                let inst = builder.ins().call(local_func, &[r]);
                (builder.inst_results(inst)[0], true)
            } else {
                (r, false)
            };

            let fn_name = match op {
                Add => "__olive_py_add",
                Sub => "__olive_py_sub",
                Mul => "__olive_py_mul",
                Div => "__olive_py_div",
                Mod => "__olive_py_mod",
                Pow => "__olive_py_pow",
                _ => unreachable!(),
            };
            let func_id = func_ids
                .get(fn_name)
                .unwrap_or_else(|| panic!("missing py_arith fn: {}", fn_name));
            let local_func = module.declare_func_in_func(*func_id, builder.func);
            let inst = builder.ins().call(local_func, &[l_val, r_val]);
            let result = builder.inst_results(inst)[0];

            // Coerced arguments were boxed just for this call; nothing else owns them.
            let decref_id = func_ids
                .get("__olive_py_decref")
                .expect("missing __olive_py_decref");
            if l_coerced {
                let local_func = module.declare_func_in_func(*decref_id, builder.func);
                builder.ins().call(local_func, &[l_val]);
            }
            if r_coerced {
                let local_func = module.declare_func_in_func(*decref_id, builder.func);
                builder.ins().call(local_func, &[r_val]);
            }
            return result;
        }

        // An `Any` operand carries no static type, so arithmetic and comparison
        // dispatch on the runtime kind (string/float/int/list) rather than the
        // default integer path. `Any == None` is already rewritten earlier.
        if is_any_op(func_mir, lhs) || is_any_op(func_mir, rhs) {
            // Specializable ops share one site sequence; guard always
            // re-checked at runtime, so a stale specialize decision only
            // costs a missed fast path, never a wrong result.
            if super::is_specializable_any_binop(op) {
                if let Some(&site_id) = any_add_site_ids.get(*any_add_site_cursor) {
                    let site_index = *any_add_site_cursor;
                    *any_add_site_cursor += 1;
                    let profiled_name = Self::any_binop_profiled_name(op);
                    let fid = func_ids
                        .get(profiled_name)
                        .unwrap_or_else(|| panic!("missing {profiled_name}"));
                    let local_func = module.declare_func_in_func(*fid, builder.func);
                    let local_data = module.declare_data_in_func(site_id, builder.func);
                    let site_ptr = builder.ins().symbol_value(types::I64, local_data);

                    if specialize_sites.contains(&site_index) {
                        return Self::translate_any_binop_specialized(
                            builder,
                            op,
                            l,
                            r,
                            |builder| {
                                let inst = builder.ins().call(local_func, &[l, r, site_ptr]);
                                builder.inst_results(inst)[0]
                            },
                        );
                    }
                    let inst = builder.ins().call(local_func, &[l, r, site_ptr]);
                    return builder.inst_results(inst)[0];
                }
                *any_add_site_cursor += 1;
            }

            let any_fn = match op {
                Add => Some("__olive_any_add"),
                Sub => Some("__olive_any_sub"),
                Mul => Some("__olive_any_mul"),
                Div => Some("__olive_any_div"),
                Mod => Some("__olive_any_mod"),
                Lt => Some("__olive_any_lt"),
                LtEq => Some("__olive_any_le"),
                Gt => Some("__olive_any_gt"),
                GtEq => Some("__olive_any_ge"),
                Eq => Some("__olive_any_eq"),
                NotEq => Some("__olive_any_ne"),
                _ => None,
            };
            if let Some(name) = any_fn {
                let fid = func_ids
                    .get(name)
                    .unwrap_or_else(|| panic!("missing {name}"));
                let local_func = module.declare_func_in_func(*fid, builder.func);
                let inst = builder.ins().call(local_func, &[l, r]);
                return builder.inst_results(inst)[0];
            }
        }

        match op {
            Add => {
                let is_str = is_str_op(func_mir, lhs);
                let is_float = is_float_op(func_mir, lhs);
                let is_list = is_list_op(func_mir, lhs);

                if is_str {
                    let concat_func_id = func_ids
                        .get("__olive_str_concat")
                        .expect("missing __olive_str_concat");
                    let local_func = module.declare_func_in_func(*concat_func_id, builder.func);
                    let inst = builder.ins().call(local_func, &[l, r]);
                    builder.inst_results(inst)[0]
                } else if is_list {
                    let concat_func_id = func_ids
                        .get("__olive_list_concat")
                        .expect("missing __olive_list_concat");
                    let local_func = module.declare_func_in_func(*concat_func_id, builder.func);
                    let inst = builder.ins().call(local_func, &[l, r]);
                    builder.inst_results(inst)[0]
                } else if is_float {
                    builder.ins().fadd(l, r)
                } else if checked {
                    let loc = super::translate_rvalue::loc_value(builder, module, loc_id);
                    let kind = if is_u64_op(func_mir, lhs) || is_u64_op(func_mir, rhs) {
                        super::translate_rvalue::OVERFLOW_ADD_U
                    } else {
                        super::translate_rvalue::OVERFLOW_ADD
                    };
                    super::translate_rvalue::emit_checked_arith(
                        builder, module, func_ids, kind, l, r, loc,
                    )
                } else {
                    builder.ins().iadd(l, r)
                }
            }
            Sub => {
                if is_float_op(func_mir, lhs) {
                    builder.ins().fsub(l, r)
                } else if checked {
                    let loc = super::translate_rvalue::loc_value(builder, module, loc_id);
                    let kind = if is_u64_op(func_mir, lhs) || is_u64_op(func_mir, rhs) {
                        super::translate_rvalue::OVERFLOW_SUB_U
                    } else {
                        super::translate_rvalue::OVERFLOW_SUB
                    };
                    super::translate_rvalue::emit_checked_arith(
                        builder, module, func_ids, kind, l, r, loc,
                    )
                } else {
                    builder.ins().isub(l, r)
                }
            }
            Mul => {
                if is_float_op(func_mir, lhs) {
                    builder.ins().fmul(l, r)
                } else if checked {
                    let loc = super::translate_rvalue::loc_value(builder, module, loc_id);
                    let kind = if is_u64_op(func_mir, lhs) || is_u64_op(func_mir, rhs) {
                        super::translate_rvalue::OVERFLOW_MUL_U
                    } else {
                        super::translate_rvalue::OVERFLOW_MUL
                    };
                    super::translate_rvalue::emit_checked_arith(
                        builder, module, func_ids, kind, l, r, loc,
                    )
                } else {
                    builder.ins().imul(l, r)
                }
            }
            Div => {
                if is_float_op(func_mir, lhs) {
                    builder.ins().fdiv(l, r)
                } else {
                    let loc = super::translate_rvalue::loc_value(builder, module, loc_id);
                    super::translate_rvalue::emit_div_zero_check(
                        builder, module, func_ids, r, false, loc,
                    );
                    if is_u64_op(func_mir, lhs) || is_u64_op(func_mir, rhs) {
                        builder.ins().udiv(l, r)
                    } else {
                        super::translate_rvalue::emit_signed_div_overflow_check(
                            builder,
                            module,
                            func_ids,
                            super::translate_rvalue::OVERFLOW_DIV_MIN,
                            l,
                            r,
                            loc,
                        );
                        builder.ins().sdiv(l, r)
                    }
                }
            }
            Mod => {
                let loc = super::translate_rvalue::loc_value(builder, module, loc_id);
                super::translate_rvalue::emit_div_zero_check(
                    builder, module, func_ids, r, true, loc,
                );
                if is_u64_op(func_mir, lhs) || is_u64_op(func_mir, rhs) {
                    builder.ins().urem(l, r)
                } else {
                    super::translate_rvalue::emit_signed_div_overflow_check(
                        builder,
                        module,
                        func_ids,
                        super::translate_rvalue::OVERFLOW_MOD_MIN,
                        l,
                        r,
                        loc,
                    );
                    builder.ins().srem(l, r)
                }
            }
            Eq => {
                let mut is_str = false;
                let mut is_float = false;
                let mut is_pyobj = false;

                let mut check_op = |op: &Operand| match op {
                    Operand::Constant(Constant::Str(_)) => is_str = true,
                    Operand::Constant(Constant::Float(_)) => is_float = true,
                    Operand::Copy(loc) | Operand::Move(loc) => {
                        let ty = &func_mir.locals[loc.0].ty;
                        if *ty == OliveType::Str {
                            is_str = true;
                        }
                        if matches!(*ty, OliveType::Float | OliveType::F32) {
                            is_float = true;
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
                    let eq_func_id = func_ids
                        .get("__olive_str_eq")
                        .expect("missing __olive_str_eq");
                    let local_func = module.declare_func_in_func(*eq_func_id, builder.func);
                    let call = builder.ins().call(local_func, &[l, r]);
                    let results = builder.inst_results(call);
                    results[0]
                } else if is_pyobj {
                    let mut to_pyobj = |val: Value, op: &Operand| -> Value {
                        if is_float_op(func_mir, op) {
                            let fid = func_ids
                                .get("__olive_py_from_float")
                                .expect("missing __olive_py_from_float");
                            let lf = module.declare_func_in_func(*fid, builder.func);
                            let inst = builder.ins().call(lf, &[val]);
                            builder.inst_results(inst)[0]
                        } else if !is_pyobj_op(func_mir, op) {
                            let fid = func_ids
                                .get("__olive_py_from_int")
                                .expect("missing __olive_py_from_int");
                            let lf = module.declare_func_in_func(*fid, builder.func);
                            let inst = builder.ins().call(lf, &[val]);
                            builder.inst_results(inst)[0]
                        } else {
                            val
                        }
                    };
                    let l_val = to_pyobj(l, lhs);
                    let r_val = to_pyobj(r, rhs);
                    let eq_func_id = func_ids
                        .get("__olive_py_eq")
                        .expect("missing __olive_py_eq");
                    let local_func = module.declare_func_in_func(*eq_func_id, builder.func);
                    let call = builder.ins().call(local_func, &[l_val, r_val]);
                    let results = builder.inst_results(call);
                    results[0]
                } else if is_float {
                    let res = builder.ins().fcmp(FloatCC::Equal, l, r);
                    builder.ins().uextend(types::I64, res)
                } else {
                    let res = builder.ins().icmp(IntCC::Equal, l, r);
                    builder.ins().uextend(types::I64, res)
                }
            }

            Lt | LtEq | Gt | GtEq | NotEq => {
                if matches!(op, NotEq) && (is_str_op(func_mir, lhs) || is_str_op(func_mir, rhs)) {
                    let eq_func_id = func_ids
                        .get("__olive_str_eq")
                        .expect("missing __olive_str_eq");
                    let local_func = module.declare_func_in_func(*eq_func_id, builder.func);
                    let call = builder.ins().call(local_func, &[l, r]);
                    let eq = builder.inst_results(call)[0];
                    let ne = builder.ins().icmp_imm(IntCC::Equal, eq, 0);
                    return builder.ins().uextend(types::I64, ne);
                }

                let is_py = is_pyobj_op(func_mir, lhs) || is_pyobj_op(func_mir, rhs);
                if is_py {
                    let mut to_pyobj = |val: Value, op: &Operand| -> Value {
                        if is_float_op(func_mir, op) {
                            let fid = func_ids
                                .get("__olive_py_from_float")
                                .expect("missing __olive_py_from_float");
                            let lf = module.declare_func_in_func(*fid, builder.func);
                            let inst = builder.ins().call(lf, &[val]);
                            builder.inst_results(inst)[0]
                        } else if !is_pyobj_op(func_mir, op) {
                            let fid = func_ids
                                .get("__olive_py_from_int")
                                .expect("missing __olive_py_from_int");
                            let lf = module.declare_func_in_func(*fid, builder.func);
                            let inst = builder.ins().call(lf, &[val]);
                            builder.inst_results(inst)[0]
                        } else {
                            val
                        }
                    };
                    let l_val = to_pyobj(l, lhs);
                    let r_val = to_pyobj(r, rhs);
                    let fn_name = match op {
                        Lt => "__olive_py_lt",
                        LtEq => "__olive_py_le",
                        Gt => "__olive_py_gt",
                        GtEq => "__olive_py_ge",
                        NotEq => "__olive_py_ne",
                        _ => unreachable!(),
                    };
                    let fid = func_ids
                        .get(fn_name)
                        .unwrap_or_else(|| panic!("missing py_cmp fn: {}", fn_name));
                    let lf = module.declare_func_in_func(*fid, builder.func);
                    let inst = builder.ins().call(lf, &[l_val, r_val]);
                    return builder.inst_results(inst)[0];
                }

                let is_float = is_float_op(func_mir, lhs) || is_float_op(func_mir, rhs);
                let is_u64 = is_u64_op(func_mir, lhs) || is_u64_op(func_mir, rhs);

                if is_float {
                    let cc = match op {
                        Lt => FloatCC::LessThan,
                        LtEq => FloatCC::LessThanOrEqual,
                        Gt => FloatCC::GreaterThan,
                        GtEq => FloatCC::GreaterThanOrEqual,
                        NotEq => FloatCC::NotEqual,
                        _ => unreachable!(),
                    };
                    let res = builder.ins().fcmp(cc, l, r);
                    builder.ins().uextend(types::I64, res)
                } else if is_u64 {
                    let cc = match op {
                        Lt => IntCC::UnsignedLessThan,
                        LtEq => IntCC::UnsignedLessThanOrEqual,
                        Gt => IntCC::UnsignedGreaterThan,
                        GtEq => IntCC::UnsignedGreaterThanOrEqual,
                        NotEq => IntCC::NotEqual,
                        _ => unreachable!(),
                    };
                    let res = builder.ins().icmp(cc, l, r);
                    builder.ins().uextend(types::I64, res)
                } else {
                    let cc = match op {
                        Lt => IntCC::SignedLessThan,
                        LtEq => IntCC::SignedLessThanOrEqual,
                        Gt => IntCC::SignedGreaterThan,
                        GtEq => IntCC::SignedGreaterThanOrEqual,
                        NotEq => IntCC::NotEqual,
                        _ => unreachable!(),
                    };
                    let res = builder.ins().icmp(cc, l, r);
                    builder.ins().uextend(types::I64, res)
                }
            }
            Shl => builder.ins().ishl(l, r),
            Shr => {
                if is_u64_op(func_mir, lhs) {
                    builder.ins().ushr(l, r)
                } else {
                    builder.ins().sshr(l, r)
                }
            }
            And => builder.ins().band(l, r),
            Or => builder.ins().bor(l, r),
            BitAnd => builder.ins().band(l, r),
            BitOr => {
                let mut is_pyobj = false;
                let mut check_op = |op: &Operand| match op {
                    Operand::Copy(loc) | Operand::Move(loc) => {
                        let ty = &func_mir.locals[loc.0].ty;
                        if *ty == OliveType::PyObject {
                            is_pyobj = true;
                        }
                    }
                    _ => {}
                };
                check_op(lhs);
                check_op(rhs);
                if is_pyobj {
                    let bitor_id = func_ids
                        .get("__olive_py_bitor")
                        .expect("missing __olive_py_bitor");
                    let local_func = module.declare_func_in_func(*bitor_id, builder.func);
                    let inst = builder.ins().call(local_func, &[l, r]);
                    builder.inst_results(inst)[0]
                } else {
                    builder.ins().bor(l, r)
                }
            }
            BitXor => builder.ins().bxor(l, r),
            Pow => {
                let is_float = is_float_op(func_mir, lhs);
                let func_name = if is_float {
                    "__olive_pow_float"
                } else {
                    "__olive_pow"
                };
                let pow_id = func_ids
                    .get(func_name)
                    .unwrap_or_else(|| panic!("missing pow fn: {}", func_name));
                let local_func = module.declare_func_in_func(*pow_id, builder.func);
                let inst = builder.ins().call(local_func, &[l, r]);
                builder.inst_results(inst)[0]
            }
            In | NotIn => {
                let mut is_obj = false;
                let mut is_str = false;
                let mut structural_key: Option<&OliveType> = None;
                if let Operand::Copy(loc) | Operand::Move(loc) = rhs {
                    let ty = super::imports::concrete_ty(&func_mir.locals[loc.0].ty);
                    if let OliveType::Dict(k, _) = ty {
                        is_obj = true;
                        if super::imports::needs_structural_key(k) {
                            structural_key = Some(k);
                        }
                    } else if matches!(ty, OliveType::Struct(_, _, _)) {
                        is_obj = true;
                    } else if matches!(ty, OliveType::Str) {
                        is_str = true;
                    } else if let OliveType::Set(e) = ty
                        && super::imports::needs_structural_key(e)
                    {
                        structural_key = Some(e);
                    }
                } else if let Operand::Constant(Constant::Str(_)) = rhs {
                    is_str = true;
                }

                let func_name = if is_str {
                    "__olive_str_contains"
                } else if is_obj {
                    if structural_key.is_some() {
                        "__olive_in_obj_typed"
                    } else {
                        "__olive_in_obj"
                    }
                } else if structural_key.is_some() {
                    "__olive_in_list_typed"
                } else {
                    "__olive_in_list"
                };
                let in_id = func_ids
                    .get(func_name)
                    .unwrap_or_else(|| panic!("missing in fn: {}", func_name));
                let local_func = module.declare_func_in_func(*in_id, builder.func);

                let inst = if is_str {
                    builder.ins().call(local_func, &[r, l])
                } else if let Some(key_ty) = structural_key {
                    let desc = super::imports::type_descriptor(
                        key_ty,
                        struct_fields,
                        field_types,
                        enum_defs,
                    );
                    let data_id = *string_ids
                        .get(&desc)
                        .expect("in-operator key descriptor not interned during collection");
                    let local_data = module.declare_data_in_func(data_id, builder.func);
                    let desc_ptr = builder.ins().symbol_value(types::I64, local_data);
                    builder.ins().call(local_func, &[l, r, desc_ptr])
                } else {
                    builder.ins().call(local_func, &[l, r])
                };

                let res = builder.inst_results(inst)[0];
                if matches!(op, NotIn) {
                    let is_zero = builder.ins().icmp_imm(IntCC::Equal, res, 0);
                    builder.ins().uextend(types::I64, is_zero)
                } else {
                    res
                }
            }
            Coalesce => {
                unreachable!("Coalesce is lowered to MIR blocks, not Rvalue::BinaryOp")
            }
        }
    }

    /// Panics on a non-specializable op -- caller already checked.
    fn any_binop_profiled_name(op: &crate::parser::BinOp) -> &'static str {
        use crate::parser::BinOp::*;
        match op {
            Add => "__olive_any_add_profiled",
            Sub => "__olive_any_sub_profiled",
            Mul => "__olive_any_mul_profiled",
            Div => "__olive_any_div_profiled",
            Mod => "__olive_any_mod_profiled",
            Lt => "__olive_any_lt_profiled",
            LtEq => "__olive_any_le_profiled",
            Gt => "__olive_any_gt_profiled",
            GtEq => "__olive_any_ge_profiled",
            Eq => "__olive_any_eq_profiled",
            NotEq => "__olive_any_ne_profiled",
            _ => unreachable!("{op:?} is not a specializable Any binop"),
        }
    }

    /// Both operands inline `TAG_INT` -> unbox, compute, validity-check,
    /// repack; else `fallback` (same call an unspecialized site makes).
    ///
    /// `*` needs a widening `smulhi`/`imul` overflow check, not a post-hoc
    /// range check: two 61-bit values can multiply past `i64`, and a wrapped
    /// product can coincidentally land back in-range and look valid.
    /// `%` needs only a zero-divisor check: a remainder's magnitude is below
    /// the divisor's, which is in range. `/` also range-checks the quotient
    /// for the one escaping case, `ANY_INT_MIN / -1 = 2^60 > ANY_INT_MAX`.
    /// Comparisons need no check and return a raw `i64` 0/1, not a repacked
    /// `Any` (matches `any_cmp!`'s "stays a bare word" semantics).
    fn translate_any_binop_specialized(
        builder: &mut FunctionBuilder,
        op: &crate::parser::BinOp,
        l: Value,
        r: Value,
        fallback: impl FnOnce(&mut FunctionBuilder) -> Value,
    ) -> Value {
        use crate::parser::BinOp::*;

        let result_var = builder.declare_var(types::I64);

        let fast_block = builder.create_block();
        let compute_ok_block = builder.create_block();
        let compute_fail_block = builder.create_block();
        let slow_block = builder.create_block();
        let merge_block = builder.create_block();

        let diff_l = builder.ins().bxor_imm(l, ANY_TAG_INT);
        let diff_r = builder.ins().bxor_imm(r, ANY_TAG_INT);
        let diff = builder.ins().bor(diff_l, diff_r);
        let tag_bits = builder.ins().band_imm(diff, ANY_TAG_MASK);
        let both_int = builder.ins().icmp_imm(IntCC::Equal, tag_bits, 0);
        builder
            .ins()
            .brif(both_int, fast_block, &[], slow_block, &[]);

        builder.switch_to_block(fast_block);
        builder.seal_block(fast_block);
        let vl = builder.ins().sshr_imm(l, 3);
        let vr = builder.ins().sshr_imm(r, 3);

        match op {
            Add | Sub => {
                let raw = if matches!(op, Add) {
                    builder.ins().iadd(vl, vr)
                } else {
                    builder.ins().isub(vl, vr)
                };
                let biased = builder.ins().iadd_imm(raw, -ANY_INT_MIN);
                let in_range = builder.ins().icmp_imm(
                    IntCC::UnsignedLessThanOrEqual,
                    biased,
                    ANY_INT_MAX - ANY_INT_MIN,
                );
                builder
                    .ins()
                    .brif(in_range, compute_ok_block, &[], compute_fail_block, &[]);

                builder.switch_to_block(compute_ok_block);
                builder.seal_block(compute_ok_block);
                let shifted = builder.ins().ishl_imm(raw, 3);
                let tagged = builder.ins().bor_imm(shifted, ANY_TAG_INT);
                builder.def_var(result_var, tagged);
                builder.ins().jump(merge_block, &[]);
            }
            Mul => {
                let hi = builder.ins().smulhi(vl, vr);
                let lo = builder.ins().imul(vl, vr);
                let sign = builder.ins().sshr_imm(lo, 63);
                let fits_i64 = builder.ins().icmp(IntCC::Equal, hi, sign);
                let biased = builder.ins().iadd_imm(lo, -ANY_INT_MIN);
                let in_range = builder.ins().icmp_imm(
                    IntCC::UnsignedLessThanOrEqual,
                    biased,
                    ANY_INT_MAX - ANY_INT_MIN,
                );
                let ok = builder.ins().band(fits_i64, in_range);
                builder
                    .ins()
                    .brif(ok, compute_ok_block, &[], compute_fail_block, &[]);

                builder.switch_to_block(compute_ok_block);
                builder.seal_block(compute_ok_block);
                let shifted = builder.ins().ishl_imm(lo, 3);
                let tagged = builder.ins().bor_imm(shifted, ANY_TAG_INT);
                builder.def_var(result_var, tagged);
                builder.ins().jump(merge_block, &[]);
            }
            Div | Mod => {
                let nonzero = builder.ins().icmp_imm(IntCC::NotEqual, vr, 0);
                builder
                    .ins()
                    .brif(nonzero, compute_ok_block, &[], compute_fail_block, &[]);

                builder.switch_to_block(compute_ok_block);
                builder.seal_block(compute_ok_block);
                let raw = if matches!(op, Div) {
                    builder.ins().sdiv(vl, vr)
                } else {
                    builder.ins().srem(vl, vr)
                };
                let shifted = builder.ins().ishl_imm(raw, 3);
                let tagged = builder.ins().bor_imm(shifted, ANY_TAG_INT);
                builder.def_var(result_var, tagged);
                if matches!(op, Div) {
                    let biased = builder.ins().iadd_imm(raw, -ANY_INT_MIN);
                    let in_range = builder.ins().icmp_imm(
                        IntCC::UnsignedLessThanOrEqual,
                        biased,
                        ANY_INT_MAX - ANY_INT_MIN,
                    );
                    builder
                        .ins()
                        .brif(in_range, merge_block, &[], compute_fail_block, &[]);
                } else {
                    builder.ins().jump(merge_block, &[]);
                }
            }
            Lt | LtEq | Gt | GtEq | Eq | NotEq => {
                let cc = match op {
                    Lt => IntCC::SignedLessThan,
                    LtEq => IntCC::SignedLessThanOrEqual,
                    Gt => IntCC::SignedGreaterThan,
                    GtEq => IntCC::SignedGreaterThanOrEqual,
                    Eq => IntCC::Equal,
                    NotEq => IntCC::NotEqual,
                    _ => unreachable!(),
                };
                let cmp = builder.ins().icmp(cc, vl, vr);
                let raw_bool = builder.ins().uextend(types::I64, cmp);
                builder.def_var(result_var, raw_bool);
                builder.ins().jump(merge_block, &[]);
            }
            _ => unreachable!("{op:?} is not a specializable Any binop"),
        }

        // Unreachable for comparisons (no validity check to fail); legal SSA regardless.
        builder.switch_to_block(compute_fail_block);
        builder.seal_block(compute_fail_block);
        builder.ins().jump(slow_block, &[]);

        builder.switch_to_block(slow_block);
        builder.seal_block(slow_block);
        let slow_result = fallback(builder);
        builder.def_var(result_var, slow_result);
        builder.ins().jump(merge_block, &[]);

        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
        builder.use_var(result_var)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_unaryop(
        builder: &mut FunctionBuilder,
        vars: &HashMap<Local, Variable>,
        string_ids: &HashMap<String, DataId>,
        module: &mut M,
        func_ids: &HashMap<String, FuncId>,
        op: &crate::parser::UnaryOp,
        operand: &Operand,
        operand_ty: &crate::semantic::types::Type,
    ) -> Value {
        let o = Self::translate_operand(builder, operand, vars, string_ids, module, func_ids);
        use crate::parser::UnaryOp::*;
        match op {
            Neg => {
                let is_float = builder.func.dfg.value_type(o) == types::F64;
                if is_float {
                    builder.ins().fneg(o)
                } else if *operand_ty == OliveType::PyObject {
                    let to_int_id = func_ids
                        .get("__olive_py_to_int")
                        .expect("missing __olive_py_to_int");
                    let local_func = module.declare_func_in_func(*to_int_id, builder.func);
                    let inst = builder.ins().call(local_func, &[o]);
                    let int_val = builder.inst_results(inst)[0];
                    let negated = builder.ins().ineg(int_val);
                    let from_int_id = func_ids
                        .get("__olive_py_from_int")
                        .expect("missing __olive_py_from_int");
                    let local_func = module.declare_func_in_func(*from_int_id, builder.func);
                    let inst = builder.ins().call(local_func, &[negated]);
                    builder.inst_results(inst)[0]
                } else {
                    builder.ins().ineg(o)
                }
            }
            Not => {
                if *operand_ty == crate::semantic::types::Type::PyObject {
                    let to_int_id = func_ids
                        .get("__olive_py_to_int")
                        .expect("missing __olive_py_to_int");
                    let local_func = module.declare_func_in_func(*to_int_id, builder.func);
                    let inst = builder.ins().call(local_func, &[o]);
                    let int_val = builder.inst_results(inst)[0];
                    let res = builder.ins().icmp_imm(IntCC::Equal, int_val, 0);
                    builder.ins().uextend(types::I64, res)
                } else {
                    let res = builder.ins().icmp_imm(IntCC::Equal, o, 0);
                    builder.ins().uextend(types::I64, res)
                }
            }
            BitNot => builder.ins().bnot(o),
            Pos => o,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::{call_i64_1, call_i64_2, compile};

    #[test]
    fn test_translate_binop_add() {
        let mut cg = compile("fn f(a: i64, b: i64) -> i64:\n    return a + b\n");
        assert_eq!(call_i64_2(&mut cg, "f", 30, 12), 42);
    }

    #[test]
    fn test_translate_binop_sub() {
        let mut cg = compile("fn f(a: i64, b: i64) -> i64:\n    return a - b\n");
        assert_eq!(call_i64_2(&mut cg, "f", 50, 8), 42);
    }

    #[test]
    fn test_translate_binop_mul() {
        let mut cg = compile("fn f(a: i64, b: i64) -> i64:\n    return a * b\n");
        assert_eq!(call_i64_2(&mut cg, "f", 6, 7), 42);
    }

    #[test]
    fn test_translate_binop_div() {
        let mut cg = compile("fn f(a: i64, b: i64) -> i64:\n    return a / b\n");
        assert_eq!(call_i64_2(&mut cg, "f", 84, 2), 42);
    }

    #[test]
    fn test_translate_binop_mod() {
        let mut cg = compile("fn f(a: i64, b: i64) -> i64:\n    return a % b\n");
        assert_eq!(call_i64_2(&mut cg, "f", 100, 58), 42);
    }

    #[test]
    fn test_translate_binop_bitwise() {
        let mut cg = compile("fn f(a: i64, b: i64) -> i64:\n    return a & b\n");
        assert_eq!(call_i64_2(&mut cg, "f", 0xFF, 0x0F), 0x0F);
    }

    #[test]
    fn test_translate_binop_shift() {
        let mut cg = compile("fn f(a: i64) -> i64:\n    return a << 1\n");
        assert_eq!(call_i64_1(&mut cg, "f", 21), 42);
    }

    #[test]
    fn test_translate_unaryop_neg() {
        let mut cg = compile("fn f(x: i64) -> i64:\n    return 0 - x\n");
        assert_eq!(call_i64_1(&mut cg, "f", 42), -42);
    }

    #[test]
    fn test_translate_unaryop_bitnot() {
        let mut cg = compile("fn f(x: i64) -> i64:\n    return ~x\n");
        assert_eq!(call_i64_1(&mut cg, "f", 0), -1);
    }

    #[test]
    fn test_translate_binop_cmp_eq() {
        let mut cg = compile(
            "fn f(a: i64, b: i64) -> i64:\n    if a == b:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_2(&mut cg, "f", 42, 42), 1);
        assert_eq!(call_i64_2(&mut cg, "f", 42, 43), 0);
    }

    #[test]
    fn test_translate_binop_cmp_chain() {
        let mut cg = compile(
            "fn f(a: i64, b: i64) -> i64:\n    if a < b:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_2(&mut cg, "f", 10, 20), 1);
        assert_eq!(call_i64_2(&mut cg, "f", 20, 10), 0);
    }

    #[test]
    fn test_translate_binop_logical_and() {
        let mut cg = compile(
            "fn f(a: i64, b: i64) -> i64:\n    if a != 0 and b != 0:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_2(&mut cg, "f", 1, 1), 1);
        assert_eq!(call_i64_2(&mut cg, "f", 1, 0), 0);
    }

    #[test]
    fn test_translate_binop_logical_or() {
        let mut cg = compile(
            "fn f(a: i64, b: i64) -> i64:\n    if a != 0 or b != 0:\n        return 1\n    return 0\n",
        );
        assert_eq!(call_i64_2(&mut cg, "f", 1, 0), 1);
        assert_eq!(call_i64_2(&mut cg, "f", 0, 0), 0);
    }
}
