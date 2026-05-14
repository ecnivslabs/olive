use super::CraneliftCodegen;
use super::imports::{cl_type, is_float_op, is_list_op, is_str_op, map_builtin_to_runtime};
use crate::mir::{
    Constant, Local, MirFunction, Operand, Rvalue, Statement, StatementKind, Terminator,
    TerminatorKind,
};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_jit::JITModule;
use cranelift_module::{DataId, FuncId, Module};
use rustc_hash::FxHashMap as HashMap;

impl<'a> CraneliftCodegen<'a> {
    pub(super) fn translate_function(&mut self, func: &MirFunction) {
        let mut ctx = self.module.make_context();

        for i in 0..func.arg_count {
            let ty = &func.locals[i + 1].ty;
            ctx.func.signature.params.push(AbiParam::new(cl_type(ty)));
        }
        let ret_ty = &func.locals[0].ty;
        ctx.func
            .signature
            .returns
            .push(AbiParam::new(cl_type(ret_ty)));

        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);

        let blocks: Vec<Block> = func
            .basic_blocks
            .iter()
            .map(|_| builder.create_block())
            .collect();
        let mut vars = HashMap::default();

        for (i, decl) in func.locals.iter().enumerate() {
            let var = builder.declare_var(cl_type(&decl.ty));
            vars.insert(Local(i), var);
        }

        let mut pred_count = vec![0u32; func.basic_blocks.len()];
        for bb in &func.basic_blocks {
            if let Some(term) = &bb.terminator {
                match &term.kind {
                    TerminatorKind::Goto { target } => {
                        pred_count[target.0] += 1;
                    }
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        for (_, t) in targets {
                            pred_count[t.0] += 1;
                        }
                        pred_count[otherwise.0] += 1;
                    }
                    _ => {}
                }
            }
        }
        let mut sealed = vec![false; func.basic_blocks.len()];
        let mut filled_pred = vec![0u32; func.basic_blocks.len()];

        for (i, bb) in func.basic_blocks.iter().enumerate() {
            builder.switch_to_block(blocks[i]);

            if i == 0 && !sealed[i] {
                builder.seal_block(blocks[i]);
                sealed[i] = true;
            }

            if i == 0 {
                builder.append_block_params_for_function_params(blocks[i]);
                let params: Vec<Value> = builder.block_params(blocks[i]).to_vec();

                for (j, val) in params.iter().enumerate() {
                    let var = vars.get(&Local(j + 1)).unwrap();
                    builder.def_var(*var, *val);
                }
            }

            for stmt in &bb.statements {
                Self::translate_statement(
                    func,
                    &mut self.module,
                    &self.func_ids,
                    &self.string_ids,
                    &mut builder,
                    stmt,
                    &vars,
                );
            }

            if let Some(term) = &bb.terminator {
                Self::translate_terminator(
                    &mut builder,
                    term,
                    &blocks,
                    &vars,
                    &self.string_ids,
                    &mut self.module,
                    &self.func_ids,
                    func.is_async,
                );
                match &term.kind {
                    TerminatorKind::Goto { target } => {
                        filled_pred[target.0] += 1;
                        if filled_pred[target.0] == pred_count[target.0] && !sealed[target.0] {
                            builder.seal_block(blocks[target.0]);
                            sealed[target.0] = true;
                        }
                    }
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        for (_, t) in targets {
                            filled_pred[t.0] += 1;
                            if filled_pred[t.0] == pred_count[t.0] && !sealed[t.0] {
                                builder.seal_block(blocks[t.0]);
                                sealed[t.0] = true;
                            }
                        }
                        filled_pred[otherwise.0] += 1;
                        if filled_pred[otherwise.0] == pred_count[otherwise.0]
                            && !sealed[otherwise.0]
                        {
                            builder.seal_block(blocks[otherwise.0]);
                            sealed[otherwise.0] = true;
                        }
                    }
                    _ => {}
                }
            } else {
                let zero = builder.ins().iconst(types::I64, 0);
                builder.ins().return_(&[zero]);
            }
        }

        for (i, block) in blocks.iter().enumerate() {
            if !sealed[i] {
                builder.seal_block(*block);
            }
        }

        builder.finalize();

        let func_id = self.func_ids.get(&func.name).unwrap();
        self.module.define_function(*func_id, &mut ctx).unwrap();
    }

    pub(super) fn translate_statement(
        func_mir: &MirFunction,
        module: &mut JITModule,
        func_ids: &HashMap<String, FuncId>,
        string_ids: &HashMap<String, DataId>,
        builder: &mut FunctionBuilder,
        stmt: &Statement,
        vars: &HashMap<Local, Variable>,
    ) {
        match &stmt.kind {
            StatementKind::Assign(local, rval) => {
                let val = Self::translate_rvalue(
                    func_mir, module, func_ids, string_ids, builder, rval, vars,
                );
                let var = vars.get(local).unwrap();

                let val_ty = builder.func.dfg.value_type(val);
                let decl_ty = cl_type(&func_mir.locals[local.0].ty);
                let val = if val_ty != decl_ty && val_ty.bits() == decl_ty.bits() {
                    builder.ins().bitcast(decl_ty, MemFlags::new(), val)
                } else {
                    val
                };

                builder.def_var(*var, val);
            }
            StatementKind::SetAttr(obj, attr, val_op) => {
                let o = Self::translate_operand(builder, obj, vars, string_ids, module);
                let v = Self::translate_operand(builder, val_op, vars, string_ids, module);

                let c_str = std::ffi::CString::new(attr.clone()).unwrap();
                let attr_ptr = c_str.into_raw() as i64;
                let attr_val = builder.ins().iconst(types::I64, attr_ptr);

                let set_id = func_ids.get("__olive_obj_set").unwrap();
                let local_func = module.declare_func_in_func(*set_id, builder.func);
                builder.ins().call(local_func, &[o, attr_val, v]);
            }
            StatementKind::SetIndex(obj, idx, val_op) => {
                let ty = if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    &func_mir.locals[loc.0].ty
                } else {
                    &OliveType::Any
                };

                let o = Self::translate_operand(builder, obj, vars, string_ids, module);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module);
                let v = Self::translate_operand(builder, val_op, vars, string_ids, module);

                match ty {
                    OliveType::Dict(_, _) | OliveType::Struct(_, _) => {
                        let set_id = func_ids.get("__olive_obj_set").unwrap();
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v]);
                    }
                    OliveType::Any => {
                        let set_id = func_ids.get("__olive_set_index_any").unwrap();
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v]);
                    }
                    OliveType::Enum(_, _) => {
                        let set_id = func_ids.get("__olive_enum_set").unwrap();
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v]);
                    }
                    _ => {
                        let data_ptr = builder.ins().load(
                            types::I64,
                            MemFlags::trusted().with_readonly(),
                            o,
                            8,
                        );
                        let offset = builder.ins().imul_imm(i, 8);
                        let addr = builder.ins().iadd(data_ptr, offset);
                        builder.ins().store(MemFlags::trusted(), v, addr, 0);
                    }
                }
            }
            StatementKind::Drop(local) => {
                let ty = &func_mir.locals[local.0].ty;
                if !ty.is_move_type() {
                    return;
                }

                let var = vars.get(local).unwrap();
                let val = builder.use_var(*var);

                let free_func_name = match ty {
                    OliveType::Str => "__olive_free_str",
                    OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_) => {
                        "__olive_free_list"
                    }
                    OliveType::Dict(_, _) | OliveType::Struct(_, _) => "__olive_free_obj",
                    OliveType::Enum(_, _) => "__olive_free_enum",
                    OliveType::Any => "__olive_free_any",
                    OliveType::Union(_) => "__olive_free_any",
                    _ => "__olive_free",
                };

                let free_id = func_ids.get(free_func_name).unwrap();
                let local_func = module.declare_func_in_func(*free_id, builder.func);
                builder.ins().call(local_func, &[val]);

                let zero = builder.ins().iconst(types::I64, 0);
                builder.def_var(*var, zero);
            }
            StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {}
            StatementKind::VectorStore(obj, idx, val_op) => {
                let o = Self::translate_operand(builder, obj, vars, string_ids, module);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module);
                let v = Self::translate_operand(builder, val_op, vars, string_ids, module);

                let data_ptr = builder.ins().load(types::I64, MemFlags::trusted(), o, 8);
                let offset = builder.ins().imul_imm(i, 8);
                let addr = builder.ins().iadd(data_ptr, offset);
                builder.ins().store(MemFlags::trusted(), v, addr, 0);
            }
        }
    }

    pub(super) fn translate_rvalue(
        func_mir: &MirFunction,
        module: &mut JITModule,
        func_ids: &HashMap<String, FuncId>,
        string_ids: &HashMap<String, DataId>,
        builder: &mut FunctionBuilder,
        rval: &Rvalue,
        vars: &HashMap<Local, Variable>,
    ) -> Value {
        match rval {
            Rvalue::Use(op) => Self::translate_operand(builder, op, vars, string_ids, module),
            Rvalue::Call { func, args } => {
                let call_args: Vec<Value> = args
                    .iter()
                    .map(|a| Self::translate_operand(builder, a, vars, string_ids, module))
                    .collect();

                if let Operand::Constant(Constant::Function(name)) = func {
                    let resolved_name = if (name == "print"
                        || name == "str"
                        || name == "int"
                        || name == "float"
                        || name == "bool"
                        || name == "iter"
                        || name == "next"
                        || name == "has_next"
                        || name == "len")
                        && !args.is_empty()
                    {
                        let arg_type = match &args[0] {
                            Operand::Constant(Constant::Str(_)) => OliveType::Str,
                            Operand::Constant(Constant::Float(_)) => OliveType::Float,
                            Operand::Copy(l) | Operand::Move(l) => func_mir.locals[l.0].ty.clone(),
                            _ => OliveType::Int,
                        };

                        map_builtin_to_runtime(name, &arg_type).unwrap_or(name.as_str())
                    } else {
                        name.as_str()
                    };

                    if let Some(&func_id) = func_ids.get(resolved_name) {
                        let local_func = module.declare_func_in_func(func_id, builder.func);
                        let inst = builder.ins().call(local_func, &call_args);
                        let results = builder.inst_results(inst);
                        return if results.is_empty() {
                            builder.ins().iconst(types::I64, 0)
                        } else {
                            results[0]
                        };
                    }
                }
                builder.ins().iconst(types::I64, 0)
            }
            Rvalue::BinaryOp(op, lhs, rhs) => {
                let l = Self::translate_operand(builder, lhs, vars, string_ids, module);
                let r = Self::translate_operand(builder, rhs, vars, string_ids, module);
                use crate::parser::BinOp::*;
                match op {
                    Add => {
                        let is_str = is_str_op(func_mir, lhs);
                        let is_float = is_float_op(func_mir, lhs);
                        let is_list = is_list_op(func_mir, lhs);

                        if is_str {
                            let concat_func_id = func_ids.get("__olive_str_concat").unwrap();
                            let local_func =
                                module.declare_func_in_func(*concat_func_id, builder.func);
                            let inst = builder.ins().call(local_func, &[l, r]);
                            builder.inst_results(inst)[0]
                        } else if is_list {
                            let concat_func_id = func_ids.get("__olive_list_concat").unwrap();
                            let local_func =
                                module.declare_func_in_func(*concat_func_id, builder.func);
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
                        } else {
                            builder.ins().sdiv(l, r)
                        }
                    }
                    Mod => builder.ins().srem(l, r),
                    Eq => {
                        let mut is_str = false;
                        let mut is_float = false;
                        match lhs {
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
                            }
                            _ => {}
                        }

                        if is_str {
                            let eq_func_id = func_ids.get("__olive_str_eq").unwrap();
                            let local_func = module.declare_func_in_func(*eq_func_id, builder.func);
                            let inst = builder.ins().call(local_func, &[l, r]);
                            builder.inst_results(inst)[0]
                        } else if is_float {
                            let res = builder.ins().fcmp(FloatCC::Equal, l, r);
                            builder.ins().uextend(types::I64, res)
                        } else {
                            let res = builder.ins().icmp(IntCC::Equal, l, r);
                            builder.ins().uextend(types::I64, res)
                        }
                    }

                    Lt | LtEq | Gt | GtEq | NotEq => {
                        let mut is_float = false;
                        if let Operand::Copy(loc) | Operand::Move(loc) = lhs {
                            if func_mir.locals[loc.0].ty == OliveType::Float {
                                is_float = true;
                            }
                        } else if let Operand::Constant(Constant::Float(_)) = lhs {
                            is_float = true;
                        }

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
                    Shr => builder.ins().sshr(l, r),
                    And => builder.ins().band(l, r),
                    Or => builder.ins().bor(l, r),
                    Pow => {
                        let is_float = is_float_op(func_mir, lhs);
                        let func_name = if is_float {
                            "__olive_pow_float"
                        } else {
                            "__olive_pow"
                        };
                        let pow_id = func_ids.get(func_name).unwrap();
                        let local_func = module.declare_func_in_func(*pow_id, builder.func);
                        let inst = builder.ins().call(local_func, &[l, r]);
                        builder.inst_results(inst)[0]
                    }
                    In => {
                        let mut is_obj = false;
                        if let Operand::Copy(loc) | Operand::Move(loc) = rhs {
                            let mut ty = &func_mir.locals[loc.0].ty;
                            while let OliveType::Ref(inner) | OliveType::MutRef(inner) = ty {
                                ty = inner;
                            }
                            if matches!(ty, OliveType::Dict(_, _) | OliveType::Struct(_, _)) {
                                is_obj = true;
                            }
                        }
                        let func_name = if is_obj {
                            "__olive_in_obj"
                        } else {
                            "__olive_in_list"
                        };
                        let in_id = func_ids.get(func_name).unwrap();
                        let local_func = module.declare_func_in_func(*in_id, builder.func);
                        let inst = builder.ins().call(local_func, &[l, r]);
                        builder.inst_results(inst)[0]
                    }
                    NotIn => {
                        let mut is_obj = false;
                        if let Operand::Copy(loc) | Operand::Move(loc) = rhs {
                            let mut ty = &func_mir.locals[loc.0].ty;
                            while let OliveType::Ref(inner) | OliveType::MutRef(inner) = ty {
                                ty = inner;
                            }
                            if matches!(ty, OliveType::Dict(_, _) | OliveType::Struct(_, _)) {
                                is_obj = true;
                            }
                        }
                        let func_name = if is_obj {
                            "__olive_in_obj"
                        } else {
                            "__olive_in_list"
                        };
                        let in_id = func_ids.get(func_name).unwrap();
                        let local_func = module.declare_func_in_func(*in_id, builder.func);
                        let inst = builder.ins().call(local_func, &[l, r]);
                        let res = builder.inst_results(inst)[0];
                        let is_zero = builder.ins().icmp_imm(IntCC::Equal, res, 0);
                        builder.ins().uextend(types::I64, is_zero)
                    }
                }
            }
            Rvalue::UnaryOp(op, operand) => {
                let o = Self::translate_operand(builder, operand, vars, string_ids, module);
                use crate::parser::UnaryOp::*;
                match op {
                    Neg => {
                        let is_float = builder.func.dfg.value_type(o) == types::F64;
                        if is_float {
                            builder.ins().fneg(o)
                        } else {
                            builder.ins().ineg(o)
                        }
                    }
                    Not => {
                        let res = builder.ins().icmp_imm(IntCC::Equal, o, 0);
                        builder.ins().uextend(types::I64, res)
                    }
                    Pos => o,
                }
            }
            Rvalue::Ref(local) | Rvalue::MutRef(local) => {
                let var = vars.get(local).unwrap();
                builder.use_var(*var)
            }
            Rvalue::GetAttr(obj, attr) => {
                let o = Self::translate_operand(builder, obj, vars, string_ids, module);
                let c_str = std::ffi::CString::new(attr.clone()).unwrap();
                let attr_ptr = c_str.into_raw() as i64;
                let attr_val = builder.ins().iconst(types::I64, attr_ptr);

                let get_id = func_ids.get("__olive_obj_get").unwrap();
                let local_func = module.declare_func_in_func(*get_id, builder.func);
                let inst = builder.ins().call(local_func, &[o, attr_val]);
                builder.inst_results(inst)[0]
            }
            Rvalue::GetTag(obj) => {
                let o = Self::translate_operand(builder, obj, vars, string_ids, module);
                let tag_id = func_ids.get("__olive_enum_tag").unwrap();
                let local_func = module.declare_func_in_func(*tag_id, builder.func);
                let inst = builder.ins().call(local_func, &[o]);
                builder.inst_results(inst)[0]
            }
            Rvalue::GetTypeId(obj) => {
                let o = Self::translate_operand(builder, obj, vars, string_ids, module);
                let fn_id = func_ids.get("__olive_enum_type_id").unwrap();
                let local_func = module.declare_func_in_func(*fn_id, builder.func);
                let inst = builder.ins().call(local_func, &[o]);
                builder.inst_results(inst)[0]
            }
            Rvalue::GetIndex(obj, idx) => {
                let ty = if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    &func_mir.locals[loc.0].ty
                } else {
                    &OliveType::Any
                };

                let o = Self::translate_operand(builder, obj, vars, string_ids, module);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module);

                match ty {
                    OliveType::Enum(_, _) => {
                        let get_id = func_ids.get("__olive_enum_get").unwrap();
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        builder.inst_results(inst)[0]
                    }
                    OliveType::Dict(_, _) | OliveType::Struct(_, _) => {
                        let get_id = func_ids.get("__olive_obj_get").unwrap();
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        builder.inst_results(inst)[0]
                    }
                    OliveType::Any => {
                        let get_id = func_ids.get("__olive_get_index_any").unwrap();
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        builder.inst_results(inst)[0]
                    }
                    OliveType::Str => {
                        let get_id = func_ids.get("__olive_str_get").unwrap();
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        builder.inst_results(inst)[0]
                    }
                    _ => {
                        let get_id = func_ids.get("__olive_get_index_any").unwrap();
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        builder.inst_results(inst)[0]
                    }
                }
            }
            Rvalue::Aggregate(kind, ops) => {
                use crate::mir::ir::AggregateKind;
                match kind {
                    AggregateKind::Dict => {
                        let new_id = func_ids.get("__olive_obj_new").unwrap();
                        let new_func = module.declare_func_in_func(*new_id, builder.func);
                        let inst = builder.ins().call(new_func, &[]);
                        let dict_ptr = builder.inst_results(inst)[0];

                        let set_id = func_ids.get("__olive_obj_set").unwrap();
                        let set_func = module.declare_func_in_func(*set_id, builder.func);

                        for i in (0..ops.len()).step_by(2) {
                            let key =
                                Self::translate_operand(builder, &ops[i], vars, string_ids, module);
                            let val = Self::translate_operand(
                                builder,
                                &ops[i + 1],
                                vars,
                                string_ids,
                                module,
                            );
                            builder.ins().call(set_func, &[dict_ptr, key, val]);
                        }
                        dict_ptr
                    }
                    AggregateKind::EnumVariant(type_id, tag) => {
                        let type_id_val = builder.ins().iconst(types::I64, *type_id);
                        let tag_val = builder.ins().iconst(types::I64, *tag as i64);
                        let count = builder.ins().iconst(types::I64, ops.len() as i64);
                        let new_id = func_ids.get("__olive_enum_new").unwrap();
                        let new_func = module.declare_func_in_func(*new_id, builder.func);
                        let inst = builder.ins().call(new_func, &[type_id_val, tag_val, count]);
                        let enum_ptr = builder.inst_results(inst)[0];

                        let set_id = func_ids.get("__olive_enum_set").unwrap();
                        let set_func = module.declare_func_in_func(*set_id, builder.func);

                        for (i, op) in ops.iter().enumerate() {
                            let idx = builder.ins().iconst(types::I64, i as i64);
                            let val =
                                Self::translate_operand(builder, op, vars, string_ids, module);
                            let val = if builder.func.dfg.value_type(val) == types::F64 {
                                builder.ins().bitcast(types::I64, MemFlags::new(), val)
                            } else {
                                val
                            };
                            builder.ins().call(set_func, &[enum_ptr, idx, val]);
                        }
                        enum_ptr
                    }
                    AggregateKind::Set => {
                        let count = builder.ins().iconst(types::I64, ops.len() as i64);
                        let new_id = func_ids.get("__olive_set_new").unwrap();
                        let new_func = module.declare_func_in_func(*new_id, builder.func);
                        let inst = builder.ins().call(new_func, &[count]);
                        let set_ptr = builder.inst_results(inst)[0];

                        let add_id = func_ids.get("__olive_set_add").unwrap();
                        let add_func = module.declare_func_in_func(*add_id, builder.func);

                        for op in ops {
                            let val =
                                Self::translate_operand(builder, op, vars, string_ids, module);
                            builder.ins().call(add_func, &[set_ptr, val]);
                        }
                        set_ptr
                    }
                    _ => {
                        let zero = builder.ins().iconst(types::I64, 0i64);
                        let new_id = func_ids.get("__olive_list_new").unwrap();
                        let new_func = module.declare_func_in_func(*new_id, builder.func);
                        let inst = builder.ins().call(new_func, &[zero]);
                        let list_ptr = builder.inst_results(inst)[0];

                        let append_id = func_ids.get("__olive_list_append").unwrap();
                        let append_func = module.declare_func_in_func(*append_id, builder.func);

                        for op in ops {
                            let val =
                                Self::translate_operand(builder, op, vars, string_ids, module);
                            builder.ins().call(append_func, &[list_ptr, val]);
                        }
                        list_ptr
                    }
                }
            }
            Rvalue::VectorSplat(op, width) => {
                let val = Self::translate_operand(builder, op, vars, string_ids, module);
                let inner_ty = builder.func.dfg.value_type(val);
                let vec_ty = inner_ty.by(*width as u32).expect("invalid splat width");
                builder.ins().splat(vec_ty, val)
            }
            Rvalue::VectorLoad(obj, idx, width) => {
                let o = Self::translate_operand(builder, obj, vars, string_ids, module);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module);
                let data_ptr = builder.ins().load(types::I64, MemFlags::trusted(), o, 8);
                let offset = builder.ins().imul_imm(i, 8);
                let addr = builder.ins().iadd(data_ptr, offset);
                let vec_ty = types::I64.by(*width as u32).unwrap();
                builder.ins().load(vec_ty, MemFlags::trusted(), addr, 0)
            }
            Rvalue::VectorFMA(a_op, b_op, c_op) => {
                let a = Self::translate_operand(builder, a_op, vars, string_ids, module);
                let b = Self::translate_operand(builder, b_op, vars, string_ids, module);
                let c = Self::translate_operand(builder, c_op, vars, string_ids, module);
                let ty = builder.func.dfg.value_type(a);
                if ty.is_int() || ty.lane_type().is_int() {
                    let prod = builder.ins().imul(a, b);
                    builder.ins().iadd(prod, c)
                } else {
                    builder.ins().fma(a, b, c)
                }
            }
        }
    }

    pub(super) fn translate_operand(
        builder: &mut FunctionBuilder,
        op: &Operand,
        vars: &HashMap<Local, Variable>,
        string_ids: &HashMap<String, DataId>,
        module: &mut JITModule,
    ) -> Value {
        match op {
            Operand::Copy(l) | Operand::Move(l) => {
                let var = vars.get(l).expect("variable not found");
                let val = builder.use_var(*var);
                if matches!(op, Operand::Move(_)) {
                    let var_ty = builder.func.dfg.value_type(val);
                    let zero = if var_ty == types::F64 {
                        builder.ins().f64const(0.0)
                    } else {
                        builder.ins().iconst(types::I64, 0)
                    };
                    builder.def_var(*var, zero);
                }
                val
            }
            Operand::Constant(c) => match c {
                Constant::Int(i) => builder.ins().iconst(types::I64, *i),
                Constant::Float(f) => builder.ins().f64const(f64::from_bits(*f)),
                Constant::Bool(b) => {
                    let val = if *b { 1 } else { 0 };
                    builder.ins().iconst(types::I64, val)
                }
                Constant::Str(s) => {
                    let id = *string_ids.get(s).expect("string constant not found");
                    let local_id = module.declare_data_in_func(id, builder.func);
                    let ptr = builder.ins().symbol_value(types::I64, local_id);
                    builder.ins().bor_imm(ptr, 1)
                }
                _ => builder.ins().iconst(types::I64, 0),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn translate_terminator(
        builder: &mut FunctionBuilder,
        term: &Terminator,
        blocks: &[Block],
        vars: &HashMap<Local, Variable>,
        string_ids: &HashMap<String, DataId>,
        module: &mut JITModule,
        func_ids: &HashMap<String, FuncId>,
        is_async: bool,
    ) {
        match &term.kind {
            TerminatorKind::Goto { target } => {
                builder.ins().jump(blocks[target.0], &[]);
            }
            TerminatorKind::SwitchInt {
                discr,
                targets,
                otherwise,
            } => {
                let val = Self::translate_operand(builder, discr, vars, string_ids, module);
                if targets.len() == 1 && targets[0].0 == 1 {
                    let target_block = blocks[targets[0].1.0];
                    let else_block = blocks[otherwise.0];
                    let cond = builder.ins().icmp_imm(IntCC::NotEqual, val, 0);
                    builder.ins().brif(cond, target_block, &[], else_block, &[]);
                } else {
                    let mut switch = cranelift::frontend::Switch::new();
                    for (v, target_bb) in targets {
                        switch.set_entry(*v as u128, blocks[target_bb.0]);
                    }
                    switch.emit(builder, val, blocks[otherwise.0]);
                }
            }
            TerminatorKind::Return => {
                let var = vars.get(&Local(0)).unwrap();
                let ret_val = builder.use_var(*var);
                if is_async {
                    let make_future_id = func_ids.get("__olive_make_future").unwrap();
                    let local_func = module.declare_func_in_func(*make_future_id, builder.func);
                    let call = builder.ins().call(local_func, &[ret_val]);
                    let future_val = builder.inst_results(call)[0];
                    builder.ins().return_(&[future_val]);
                } else {
                    builder.ins().return_(&[ret_val]);
                }
            }
            TerminatorKind::Unreachable => {
                builder.ins().trap(TrapCode::unwrap_user(1));
            }
        }
    }
}
