use super::CraneliftCodegen;
use super::imports::{is_float_op, is_list_op, is_pyobj_op, is_str_op, is_u64_op};
use crate::mir::{Constant, Local, MirFunction, Operand};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::FxHashMap as HashMap;

impl<M: Module> CraneliftCodegen<M> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_binop(
        func_mir: &MirFunction,
        module: &mut M,
        func_ids: &HashMap<String, FuncId>,
        string_ids: &HashMap<String, DataId>,
        builder: &mut FunctionBuilder,
        vars: &HashMap<Local, Variable>,
        op: &crate::parser::BinOp,
        lhs: &Operand,
        rhs: &Operand,
    ) -> Value {
        let l = Self::translate_operand(builder, lhs, vars, string_ids, module, func_ids);
        let r = Self::translate_operand(builder, rhs, vars, string_ids, module, func_ids);
        use crate::parser::BinOp::*;

        let is_py = is_pyobj_op(func_mir, lhs) || is_pyobj_op(func_mir, rhs);
        if is_py && matches!(op, Add | Sub | Mul | Div | Mod | Pow) {
            let l_val = if is_float_op(func_mir, lhs) {
                let func_id = func_ids
                    .get("__olive_py_from_float")
                    .expect("missing __olive_py_from_float");
                let local_func = module.declare_func_in_func(*func_id, builder.func);
                let inst = builder.ins().call(local_func, &[l]);
                builder.inst_results(inst)[0]
            } else if !is_pyobj_op(func_mir, lhs) {
                let func_id = func_ids
                    .get("__olive_py_from_int")
                    .expect("missing __olive_py_from_int");
                let local_func = module.declare_func_in_func(*func_id, builder.func);
                let inst = builder.ins().call(local_func, &[l]);
                builder.inst_results(inst)[0]
            } else {
                l
            };

            let r_val = if is_float_op(func_mir, rhs) {
                let func_id = func_ids
                    .get("__olive_py_from_float")
                    .expect("missing __olive_py_from_float");
                let local_func = module.declare_func_in_func(*func_id, builder.func);
                let inst = builder.ins().call(local_func, &[r]);
                builder.inst_results(inst)[0]
            } else if !is_pyobj_op(func_mir, rhs) {
                let func_id = func_ids
                    .get("__olive_py_from_int")
                    .expect("missing __olive_py_from_int");
                let local_func = module.declare_func_in_func(*func_id, builder.func);
                let inst = builder.ins().call(local_func, &[r]);
                builder.inst_results(inst)[0]
            } else {
                r
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
            return builder.inst_results(inst)[0];
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
                } else {
                    builder.ins().iadd(l, r)
                }
            }
            Sub => {
                if is_float_op(func_mir, lhs) {
                    builder.ins().fsub(l, r)
                } else {
                    builder.ins().isub(l, r)
                }
            }
            Mul => {
                if is_float_op(func_mir, lhs) {
                    builder.ins().fmul(l, r)
                } else {
                    builder.ins().imul(l, r)
                }
            }
            Div => {
                if is_float_op(func_mir, lhs) {
                    builder.ins().fdiv(l, r)
                } else if is_u64_op(func_mir, lhs) || is_u64_op(func_mir, rhs) {
                    builder.ins().udiv(l, r)
                } else {
                    builder.ins().sdiv(l, r)
                }
            }
            Mod => {
                if is_u64_op(func_mir, lhs) || is_u64_op(func_mir, rhs) {
                    builder.ins().urem(l, r)
                } else {
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
                        if *ty == OliveType::Float {
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

                let mut is_float = false;
                if let Operand::Copy(loc) | Operand::Move(loc) = lhs {
                    if func_mir.locals[loc.0].ty == OliveType::Float {
                        is_float = true;
                    }
                } else if let Operand::Constant(Constant::Float(_)) = lhs {
                    is_float = true;
                }
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
                if let Operand::Copy(loc) | Operand::Move(loc) = rhs {
                    let mut ty = &func_mir.locals[loc.0].ty;
                    while let OliveType::Ref(inner) | OliveType::MutRef(inner) = ty {
                        ty = inner;
                    }
                    if matches!(ty, OliveType::Dict(_, _) | OliveType::Struct(_, _)) {
                        is_obj = true;
                    } else if matches!(ty, OliveType::Str) {
                        is_str = true;
                    }
                } else if let Operand::Constant(Constant::Str(_)) = rhs {
                    is_str = true;
                }

                let func_name = if is_str {
                    "__olive_str_contains"
                } else if is_obj {
                    "__olive_in_obj"
                } else {
                    "__olive_in_list"
                };
                let in_id = func_ids
                    .get(func_name)
                    .unwrap_or_else(|| panic!("missing in fn: {}", func_name));
                let local_func = module.declare_func_in_func(*in_id, builder.func);

                let inst = if is_str {
                    builder.ins().call(local_func, &[r, l])
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
        }
    }

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
