use rustc_hash::FxHashMap as HashMap;
use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use crate::mir::{MirFunction, Rvalue, Operand, Constant, Terminator, TerminatorKind, Statement, StatementKind, Local};
use crate::semantic::types::Type as OliveType;

pub struct CraneliftCodegen<'a> {
    functions: &'a [MirFunction],
    module: JITModule,
    func_ids: HashMap<String, FuncId>,
}

impl<'a> CraneliftCodegen<'a> {
    pub fn new(functions: &'a [MirFunction]) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
            panic!("host machine is not supported: {}", msg);
        });
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|msg| panic!("host machine is not supported: {}", msg))
            .unwrap();
            
        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        
        // Runtime symbols.
        builder.symbol("__olive_print_int", olive_print as *const u8);
        builder.symbol("__olive_print_float", olive_print_float as *const u8);
        builder.symbol("__olive_print_str", olive_print_str as *const u8);
        builder.symbol("__olive_str", olive_str as *const u8);
        builder.symbol("__olive_int", olive_int as *const u8);
        builder.symbol("__olive_float", olive_float as *const u8);
        builder.symbol("__olive_bool", olive_bool as *const u8);
        builder.symbol("__olive_bool_from_float", olive_bool_from_float as *const u8);
        builder.symbol("__olive_float_to_str", olive_float_to_str as *const u8);
        builder.symbol("__olive_float_to_int", olive_float_to_int as *const u8);
        builder.symbol("__olive_int_to_float", olive_int_to_float as *const u8);
        builder.symbol("__olive_str_to_int", olive_str_to_int as *const u8);
        builder.symbol("__olive_str_to_float", olive_str_to_float as *const u8);
        builder.symbol("__olive_str_concat", olive_str_concat as *const u8);
        builder.symbol("__olive_free", olive_free as *const u8);
        builder.symbol("__olive_copy", olive_copy as *const u8);
        builder.symbol("__olive_copy_float", olive_copy_float as *const u8);
        builder.symbol("__olive_str_eq", olive_str_eq as *const u8);
        builder.symbol("__olive_list_new", olive_list_new as *const u8);
        builder.symbol("__olive_list_set", olive_list_set as *const u8);
        builder.symbol("__olive_list_get", olive_list_get as *const u8);
        builder.symbol("__olive_obj_new", olive_obj_new as *const u8);
        builder.symbol("__olive_obj_set", olive_obj_set as *const u8);
        builder.symbol("__olive_obj_get", olive_obj_get as *const u8);
        builder.symbol("__olive_memo_get", olive_memo_get as *const u8);
        builder.symbol("__olive_cache_get", olive_cache_get as *const u8);
        builder.symbol("__olive_cache_set", olive_cache_set as *const u8);
        builder.symbol("__olive_cache_has", olive_cache_has as *const u8);
        builder.symbol("__olive_cache_has_tuple", olive_cache_has_tuple as *const u8);
        builder.symbol("__olive_cache_get_tuple", olive_cache_get_tuple as *const u8);
        builder.symbol("__olive_cache_set_tuple", olive_cache_set_tuple as *const u8);
        builder.symbol("__olive_str_len", olive_str_len as *const u8);
        builder.symbol("__olive_str_get", olive_str_get as *const u8);
        let module = JITModule::new(builder);
        
        Self {
            functions,
            module,
            func_ids: HashMap::default(),
        }
    }

    pub fn generate(&mut self) {
        let mut int_sig = self.module.make_signature();
        int_sig.params.push(AbiParam::new(types::I64));
        int_sig.returns.push(AbiParam::new(types::I64));

        let mut float_param_only_sig = self.module.make_signature();
        float_param_only_sig.params.push(AbiParam::new(types::F64));
        float_param_only_sig.returns.push(AbiParam::new(types::I64));

        let mut float_ret_sig = self.module.make_signature();
        float_ret_sig.params.push(AbiParam::new(types::I64));
        float_ret_sig.returns.push(AbiParam::new(types::F64));

        let mut float_param_sig = self.module.make_signature();
        float_param_sig.params.push(AbiParam::new(types::F64));
        float_param_sig.returns.push(AbiParam::new(types::I64));

        let mut int_to_float_sig = self.module.make_signature();
        int_to_float_sig.params.push(AbiParam::new(types::I64));
        int_to_float_sig.returns.push(AbiParam::new(types::F64));

        let mut float_to_float_sig = self.module.make_signature();
        float_to_float_sig.params.push(AbiParam::new(types::F64));
        float_to_float_sig.returns.push(AbiParam::new(types::F64));

        let int_fns = ["__olive_print_int", "__olive_print_str", "__olive_str", "__olive_int", "__olive_bool", "__olive_str_to_int", "__olive_copy", "__olive_list_new", "__olive_str_len"];
        let float_param_only_fns = ["__olive_print_float", "__olive_float_to_str", "__olive_float_to_int", "__olive_bool_from_float"];
        let float_ret_fns = ["__olive_str_to_float"];
        let int_to_float_fns = ["__olive_int_to_float", "__olive_float"];
        let float_to_float_fns = ["__olive_copy_float"];

        for name in &int_fns {
            let id = self.module.declare_function(name, Linkage::Import, &int_sig).unwrap();
            self.func_ids.insert(name.to_string(), id);
        }
        for name in &float_param_only_fns {
            let id = self.module.declare_function(name, Linkage::Import, &float_param_only_sig).unwrap();
            self.func_ids.insert(name.to_string(), id);
        }
        for name in &float_ret_fns {
            let id = self.module.declare_function(name, Linkage::Import, &float_ret_sig).unwrap();
            self.func_ids.insert(name.to_string(), id);
        }
        for name in &int_to_float_fns {
            let id = self.module.declare_function(name, Linkage::Import, &int_to_float_sig).unwrap();
            self.func_ids.insert(name.to_string(), id);
        }
        for name in float_to_float_fns {
            let id = self.module.declare_function(name, Linkage::Import, &float_to_float_sig).unwrap();
            self.func_ids.insert(name.to_string(), id);
        }

        let mut obj_new_sig = self.module.make_signature();
        obj_new_sig.returns.push(AbiParam::new(types::I64));
        let obj_new_id = self.module.declare_function("__olive_obj_new", Linkage::Import, &obj_new_sig).unwrap();
        self.func_ids.insert("__olive_obj_new".to_string(), obj_new_id);

        let mut concat_sig = self.module.make_signature();
        concat_sig.params.push(AbiParam::new(types::I64));
        concat_sig.params.push(AbiParam::new(types::I64));
        concat_sig.returns.push(AbiParam::new(types::I64));
        let concat_id = self.module.declare_function("__olive_str_concat", Linkage::Import, &concat_sig).unwrap();
        self.func_ids.insert("__olive_str_concat".to_string(), concat_id);
        
        let mut free_sig = self.module.make_signature();
        free_sig.params.push(AbiParam::new(types::I64));
        let free_id = self.module.declare_function("__olive_free", Linkage::Import, &free_sig).unwrap();
        self.func_ids.insert("__olive_free".to_string(), free_id);

        let mut eq_sig = self.module.make_signature();
        eq_sig.params.push(AbiParam::new(types::I64));
        eq_sig.params.push(AbiParam::new(types::I64));
        eq_sig.returns.push(AbiParam::new(types::I64));
        let eq_id = self.module.declare_function("__olive_str_eq", Linkage::Import, &eq_sig).unwrap();
        self.func_ids.insert("__olive_str_eq".to_string(), eq_id);

        let mut list_set_sig = self.module.make_signature();
        list_set_sig.params.push(AbiParam::new(types::I64));
        list_set_sig.params.push(AbiParam::new(types::I64));
        list_set_sig.params.push(AbiParam::new(types::I64));
        let list_set_id = self.module.declare_function("__olive_list_set", Linkage::Import, &list_set_sig).unwrap();
        self.func_ids.insert("__olive_list_set".to_string(), list_set_id);

        let mut list_get_sig = self.module.make_signature();
        list_get_sig.params.push(AbiParam::new(types::I64));
        list_get_sig.params.push(AbiParam::new(types::I64));
        list_get_sig.returns.push(AbiParam::new(types::I64));
        let list_get_id = self.module.declare_function("__olive_list_get", Linkage::Import, &list_get_sig).unwrap();
        self.func_ids.insert("__olive_list_get".to_string(), list_get_id);

        let mut str_get_sig = self.module.make_signature();
        str_get_sig.params.push(AbiParam::new(types::I64));
        str_get_sig.params.push(AbiParam::new(types::I64));
        str_get_sig.returns.push(AbiParam::new(types::I64));
        let str_get_id = self.module.declare_function("__olive_str_get", Linkage::Import, &str_get_sig).unwrap();
        self.func_ids.insert("__olive_str_get".to_string(), str_get_id);

        let mut obj_set_sig = self.module.make_signature();
        obj_set_sig.params.push(AbiParam::new(types::I64));
        obj_set_sig.params.push(AbiParam::new(types::I64));
        obj_set_sig.params.push(AbiParam::new(types::I64));
        obj_set_sig.returns.push(AbiParam::new(types::I64));
        let obj_set_id = self.module.declare_function("__olive_obj_set", Linkage::Import, &obj_set_sig).unwrap();
        self.func_ids.insert("__olive_obj_set".to_string(), obj_set_id);

        let mut cache_set_sig = self.module.make_signature();
        cache_set_sig.params.push(AbiParam::new(types::I64));
        cache_set_sig.params.push(AbiParam::new(types::I64));
        cache_set_sig.params.push(AbiParam::new(types::I64));
        cache_set_sig.returns.push(AbiParam::new(types::I64));
        let cache_set_id = self.module.declare_function("__olive_cache_set", Linkage::Import, &cache_set_sig).unwrap();
        self.func_ids.insert("__olive_cache_set".to_string(), cache_set_id);

        let mut cache_get_sig = self.module.make_signature();
        cache_get_sig.params.push(AbiParam::new(types::I64));
        cache_get_sig.params.push(AbiParam::new(types::I64));
        cache_get_sig.returns.push(AbiParam::new(types::I64));
        let cache_get_id = self.module.declare_function("__olive_cache_get", Linkage::Import, &cache_get_sig).unwrap();
        self.func_ids.insert("__olive_cache_get".to_string(), cache_get_id);

        let mut cache_has_sig = self.module.make_signature();
        cache_has_sig.params.push(AbiParam::new(types::I64));
        cache_has_sig.params.push(AbiParam::new(types::I64));
        cache_has_sig.returns.push(AbiParam::new(types::I64));
        let cache_has_id = self.module.declare_function("__olive_cache_has", Linkage::Import, &cache_has_sig).unwrap();
        self.func_ids.insert("__olive_cache_has".to_string(), cache_has_id);

        let mut obj_get_sig = self.module.make_signature();
        obj_get_sig.params.push(AbiParam::new(types::I64));
        obj_get_sig.params.push(AbiParam::new(types::I64));
        obj_get_sig.returns.push(AbiParam::new(types::I64));
        let obj_get_id = self.module.declare_function("__olive_obj_get", Linkage::Import, &obj_get_sig).unwrap();
        self.func_ids.insert("__olive_obj_get".to_string(), obj_get_id);

        let mut memo_sig = self.module.make_signature();
        memo_sig.params.push(AbiParam::new(types::I64)); // fn name string ptr
        memo_sig.params.push(AbiParam::new(types::I64)); // is_tuple bool
        memo_sig.returns.push(AbiParam::new(types::I64)); // cache dict ptr
        let memo_id = self.module.declare_function("__olive_memo_get", Linkage::Import, &memo_sig).unwrap();
        self.func_ids.insert("__olive_memo_get".to_string(), memo_id);

        let mut cache_tuple_sig = self.module.make_signature();
        cache_tuple_sig.params.push(AbiParam::new(types::I64)); // cache
        cache_tuple_sig.params.push(AbiParam::new(types::I64)); // key (ptr)
        cache_tuple_sig.returns.push(AbiParam::new(types::I64));
        
        let id = self.module.declare_function("__olive_cache_has_tuple", Linkage::Import, &cache_tuple_sig).unwrap();
        self.func_ids.insert("__olive_cache_has_tuple".to_string(), id);

        let id = self.module.declare_function("__olive_cache_get_tuple", Linkage::Import, &cache_tuple_sig).unwrap();
        self.func_ids.insert("__olive_cache_get_tuple".to_string(), id);

        let mut cache_set_tuple_sig = self.module.make_signature();
        cache_set_tuple_sig.params.push(AbiParam::new(types::I64)); // cache
        cache_set_tuple_sig.params.push(AbiParam::new(types::I64)); // key (ptr)
        cache_set_tuple_sig.params.push(AbiParam::new(types::I64)); // val
        cache_set_tuple_sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("__olive_cache_set_tuple", Linkage::Import, &cache_set_tuple_sig).unwrap();
        self.func_ids.insert("__olive_cache_set_tuple".to_string(), id);

        for func in self.functions {
            let mut sig = self.module.make_signature();
            for _ in 0..func.arg_count {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.returns.push(AbiParam::new(types::I64));
            
            let func_id = self.module
                .declare_function(&func.name, Linkage::Export, &sig)
                .unwrap();
            self.func_ids.insert(func.name.clone(), func_id);
        }

        let functions_mir = self.functions.to_vec();
        for func in functions_mir {
            self.translate_function(&func);
        }
    }

    pub fn finalize(&mut self) {
        self.module.finalize_definitions().unwrap();
    }

    pub fn get_function(&mut self, name: &str) -> Option<*const u8> {
        let func_id = self.func_ids.get(name)?;
        Some(self.module.get_finalized_function(*func_id))
    }

    fn translate_function(&mut self, func: &MirFunction) {
        let mut ctx = self.module.make_context();

        for _ in 0..func.arg_count {
            ctx.func.signature.params.push(AbiParam::new(types::I64));
        }
        ctx.func.signature.returns.push(AbiParam::new(types::I64));

        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);

        let blocks: Vec<Block> = func.basic_blocks.iter().map(|_| builder.create_block()).collect();
        let mut vars = HashMap::default();

        for (i, decl) in func.locals.iter().enumerate() {
            let var = builder.declare_var(cl_type(&decl.ty));
            vars.insert(Local(i), var);
        }
        
        for (i, bb) in func.basic_blocks.iter().enumerate() {
            builder.switch_to_block(blocks[i]);
            

            if i == 0 {
                builder.append_block_params_for_function_params(blocks[i]);
                let params: Vec<Value> = builder.block_params(blocks[i]).to_vec();
                
                /*
                for (local_idx, decl) in func.locals.iter().enumerate() {
                    let var = vars.get(&Local(local_idx)).unwrap();
                    let zero = if cl_type(&decl.ty) == types::F64 {
                        builder.ins().f64const(0.0)
                    } else {
                        builder.ins().iconst(types::I64, 0)
                    };
                    builder.def_var(*var, zero);
                }
                */

                for (j, val) in params.iter().enumerate() {
                    let var = vars.get(&Local(j + 1)).unwrap();
                    builder.def_var(*var, *val);
                }
            }
            
            for stmt in &bb.statements {
                Self::translate_statement(func, &mut self.module, &self.func_ids, &mut builder, stmt, &vars);
            }
            
            if let Some(term) = &bb.terminator {
                Self::translate_terminator(&mut builder, term, &blocks, &vars);
            } else {
                let zero = builder.ins().iconst(types::I64, 0);
                builder.ins().return_(&[zero]);
            }
        }
        

        for block in &blocks {
            builder.seal_block(*block);
        }
        
        builder.finalize();

        let func_id = self.func_ids.get(&func.name).unwrap();
        self.module.define_function(*func_id, &mut ctx).unwrap();
    }

    fn translate_statement(func_mir: &MirFunction, module: &mut JITModule, func_ids: &HashMap<String, FuncId>, builder: &mut FunctionBuilder, stmt: &Statement, vars: &HashMap<Local, Variable>) {
        match &stmt.kind {
            StatementKind::Assign(local, rval) => {
                let val = Self::translate_rvalue(func_mir, module, func_ids, builder, rval, vars);
                let var = vars.get(local).unwrap();
                builder.def_var(*var, val);
            }
            StatementKind::SetAttr(obj, attr, val_op) => {
                let o = Self::translate_operand(builder, obj, vars);
                let v = Self::translate_operand(builder, val_op, vars);
                
                let c_str = std::ffi::CString::new(attr.clone()).unwrap();
                let attr_ptr = c_str.into_raw() as i64;
                let attr_val = builder.ins().iconst(types::I64, attr_ptr);
                
                let set_id = func_ids.get("__olive_obj_set").unwrap();
                let local_func = module.declare_func_in_func(*set_id, builder.func);
                builder.ins().call(local_func, &[o, attr_val, v]);
            }
            _ => {}
        }
    }

    fn translate_rvalue(func_mir: &MirFunction, module: &mut JITModule, func_ids: &HashMap<String, FuncId>, builder: &mut FunctionBuilder, rval: &Rvalue, vars: &HashMap<Local, Variable>) -> Value {
        match rval {
            Rvalue::Use(op) => Self::translate_operand(builder, op, vars),
            Rvalue::Call { func, args } => {
                let call_args: Vec<Value> = args.iter().map(|a| Self::translate_operand(builder, a, vars)).collect();
                
                if let Operand::Constant(Constant::Function(name)) = func {
                    let (resolved_name, final_call_args) = if name == "print" && !args.is_empty() {
                        let mut arg_type = OliveType::Int;
                        match &args[0] {
                            Operand::Constant(Constant::Str(_)) => arg_type = OliveType::Str,
                            Operand::Constant(Constant::Float(_)) => arg_type = OliveType::Float,
                            Operand::Copy(l) | Operand::Move(l) => {
                                arg_type = func_mir.locals[l.0].ty.clone();
                            }
                            _ => {}
                        }

                        let mut current_ty = &arg_type;
                        while let OliveType::Ref(inner) | OliveType::MutRef(inner) = current_ty {
                            current_ty = inner;
                        }

                        let target = if *current_ty == OliveType::Str {
                            "__olive_print_str"
                        } else if *current_ty == OliveType::Float {
                            "__olive_print_float"
                        } else {
                            "__olive_print_int"
                        };
                        (target, call_args.clone())
                    } else if (name == "str" || name == "int" || name == "float" || name == "bool") && !args.is_empty() {
                        let mut arg_type = OliveType::Int;
                        match &args[0] {
                            Operand::Constant(Constant::Str(_)) => arg_type = OliveType::Str,
                            Operand::Constant(Constant::Float(_)) => arg_type = OliveType::Float,
                            Operand::Copy(l) | Operand::Move(l) => {
                                arg_type = func_mir.locals[l.0].ty.clone();
                            }
                            _ => {}
                        }
                        let mut current_ty = &arg_type;
                        while let OliveType::Ref(inner) | OliveType::MutRef(inner) = current_ty {
                            current_ty = inner;
                        }
                        
                        let target = match name.as_str() {
                            "str" => {
                                if *current_ty == OliveType::Str { "__olive_copy" }
                                else if *current_ty == OliveType::Float { "__olive_float_to_str" }
                                else { "__olive_str" }
                            },
                            "int" => {
                                if *current_ty == OliveType::Int { "__olive_int" }
                                else if *current_ty == OliveType::Float { "__olive_float_to_int" }
                                else if *current_ty == OliveType::Str { "__olive_str_to_int" }
                                else { "__olive_int" }
                            },
                            "float" => {
                                if *current_ty == OliveType::Float { "__olive_copy_float" }
                                else if *current_ty == OliveType::Int { "__olive_int_to_float" }
                                else if *current_ty == OliveType::Str { "__olive_str_to_float" }
                                else { "__olive_float" }
                            },
                            "bool" => {
                                if *current_ty == OliveType::Float { "__olive_bool_from_float" }
                                else { "__olive_bool" }
                            },
                            _ => unreachable!(),
                        };
                        (target, call_args.clone())
                    } else {
                        (name.as_str(), call_args.clone())
                    };
                    
                    if let Some(&func_id) = func_ids.get(resolved_name) {
                        let local_func = module.declare_func_in_func(func_id, builder.func);
                        let inst = builder.ins().call(local_func, &final_call_args);
                        let results = builder.inst_results(inst);
                        if results.is_empty() {
                            return builder.ins().iconst(types::I64, 0);
                        }
                        return results[0];
                    }
                }
                builder.ins().iconst(types::I64, 0)
            }
            Rvalue::BinaryOp(op, lhs, rhs) => {
                let l = Self::translate_operand(builder, lhs, vars);
                let r = Self::translate_operand(builder, rhs, vars);
                use crate::parser::BinOp::*;
                match op {
                    Add => {
                        let mut is_str = false;
                        match lhs {
                            Operand::Constant(Constant::Str(_)) => is_str = true,
                            Operand::Copy(loc) | Operand::Move(loc) => {
                                if func_mir.locals[loc.0].ty == OliveType::Str {
                                    is_str = true;
                                }
                            }
                            _ => {}
                        }
                        // Fallback: if rhs is a string constant, treat as string addition.
                        if !is_str {
                            if let Operand::Constant(Constant::Str(_)) = rhs {
                                is_str = true;
                            }
                        }
                        
                        if is_str {
                            let concat_func_id = func_ids.get("__olive_str_concat").unwrap();
                            let local_func = module.declare_func_in_func(*concat_func_id, builder.func);
                            let inst = builder.ins().call(local_func, &[l, r]);
                            builder.inst_results(inst)[0]
                        } else {
                            builder.ins().iadd(l, r)
                        }
                    }
                    Sub => builder.ins().isub(l, r),
                    Mul => builder.ins().imul(l, r),
                    Div | FloorDiv => builder.ins().sdiv(l, r),
                    Mod => builder.ins().srem(l, r),
                    Eq => {
                        let mut is_str = false;
                        let mut is_float = false;
                        match lhs {
                            Operand::Constant(Constant::Str(_)) => is_str = true,
                            Operand::Constant(Constant::Float(_)) => is_float = true,
                            Operand::Copy(loc) | Operand::Move(loc) => {
                                let ty = &func_mir.locals[loc.0].ty;
                                if *ty == OliveType::Str { is_str = true; }
                                if *ty == OliveType::Float { is_float = true; }
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
                    Is => {
                        let res = builder.ins().icmp(IntCC::Equal, l, r);
                        builder.ins().uextend(types::I64, res)
                    }
                    IsNot => {
                        let res = builder.ins().icmp(IntCC::NotEqual, l, r);
                        builder.ins().uextend(types::I64, res)
                    }
                    Lt => {
                        let res = builder.ins().icmp(IntCC::SignedLessThan, l, r);
                        builder.ins().uextend(types::I64, res)
                    }
                    LtEq => {
                        let res = builder.ins().icmp(IntCC::SignedLessThanOrEqual, l, r);
                        builder.ins().uextend(types::I64, res)
                    }
                    Gt => {
                        let res = builder.ins().icmp(IntCC::SignedGreaterThan, l, r);
                        builder.ins().uextend(types::I64, res)
                    }
                    GtEq => {
                        let res = builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, l, r);
                        builder.ins().uextend(types::I64, res)
                    }
                    _ => builder.ins().iconst(types::I64, 0),
                }
            }
            Rvalue::UnaryOp(op, operand) => {
                let o = Self::translate_operand(builder, operand, vars);
                use crate::parser::UnaryOp::*;
                match op {
                    Neg => builder.ins().ineg(o),
                    Not => builder.ins().bnot(o),
                    Pos => o,
                }
            }
            Rvalue::Ref(local) | Rvalue::MutRef(local) => {
                let var = vars.get(local).unwrap();
                builder.use_var(*var)
            }
            Rvalue::GetAttr(obj, attr) => {
                let o = Self::translate_operand(builder, obj, vars);
                let c_str = std::ffi::CString::new(attr.clone()).unwrap();
                let attr_ptr = c_str.into_raw() as i64;
                let attr_val = builder.ins().iconst(types::I64, attr_ptr);
                
                let get_id = func_ids.get("__olive_obj_get").unwrap();
                let local_func = module.declare_func_in_func(*get_id, builder.func);
                let inst = builder.ins().call(local_func, &[o, attr_val]);
                builder.inst_results(inst)[0]
            }
            Rvalue::GetIndex(obj, idx) => {
                let o = Self::translate_operand(builder, obj, vars);
                let i = Self::translate_operand(builder, idx, vars);
                let get_id = func_ids.get("__olive_list_get").unwrap();
                let local_func = module.declare_func_in_func(*get_id, builder.func);
                let inst = builder.ins().call(local_func, &[o, i]);
                builder.inst_results(inst)[0]
            }
            Rvalue::Aggregate(_, ops) => {
                let count = builder.ins().iconst(types::I64, ops.len() as i64);
                let new_id = func_ids.get("__olive_list_new").unwrap();
                let new_func = module.declare_func_in_func(*new_id, builder.func);
                let inst = builder.ins().call(new_func, &[count]);
                let list_ptr = builder.inst_results(inst)[0];

                let set_id = func_ids.get("__olive_list_set").unwrap();
                let set_func = module.declare_func_in_func(*set_id, builder.func);
                for (idx, op) in ops.iter().enumerate() {
                    let val = Self::translate_operand(builder, op, vars);
                    let idx_val = builder.ins().iconst(types::I64, idx as i64);
                    builder.ins().call(set_func, &[list_ptr, idx_val, val]);
                }
                list_ptr
            }
        }
    }

    fn translate_operand(builder: &mut FunctionBuilder, op: &Operand, vars: &HashMap<Local, Variable>) -> Value {
        match op {
            Operand::Copy(local) => {
                let var = vars.get(local).unwrap();
                builder.use_var(*var)
            }
            Operand::Move(local) => {
                let var = vars.get(local).unwrap();
                let val = builder.use_var(*var);
                let zero = builder.ins().iconst(types::I64, 0);
                builder.def_var(*var, zero);
                val
            }
            Operand::Constant(Constant::Int(i)) => builder.ins().iconst(types::I64, *i),
            Operand::Constant(Constant::Float(f)) => builder.ins().f64const(f64::from_bits(*f)),
            Operand::Constant(Constant::Str(s)) => {
                let c_str = std::ffi::CString::new(s.clone()).unwrap();
                let ptr = c_str.into_raw() as i64;
                builder.ins().iconst(types::I64, ptr)
            }
            Operand::Constant(Constant::Bool(b)) => builder.ins().iconst(types::I64, if *b { 1 } else { 0 }),
            _ => builder.ins().iconst(types::I64, 0),
        }
    }

    fn translate_terminator(builder: &mut FunctionBuilder, term: &Terminator, blocks: &[Block], vars: &HashMap<Local, Variable>) {
        match &term.kind {
            TerminatorKind::Goto { target } => {
                builder.ins().jump(blocks[target.0], &[]);
            }
            TerminatorKind::SwitchInt { discr, targets, otherwise } => {
                let val = Self::translate_operand(builder, discr, vars);
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
                builder.ins().return_(&[ret_val]);
            }
            TerminatorKind::Unreachable => {
                builder.ins().trap(TrapCode::unwrap_user(1));
            }
        }
    }
}

fn cl_type(ty: &OliveType) -> cranelift::prelude::Type {
    match ty {
        OliveType::Int | OliveType::Bool => types::I64,
        OliveType::Float => types::F64,
        _ => types::I64,
    }
}

extern "C" fn olive_print(val: i64) -> i64 {
    println!("{}", val);
    0
}

extern "C" fn olive_print_float(val: f64) -> i64 {
    println!("{}", val);
    0
}

extern "C" fn olive_print_str(val: i64) -> i64 {
    if val == 0 {
        println!("None");
    } else {
        let s = unsafe { std::ffi::CStr::from_ptr(val as *const i8) };
        println!("{}", s.to_string_lossy());
    }
    0
}

extern "C" fn olive_str(val: i64) -> i64 {
    let s = format!("{}", val);
    let c_str = std::ffi::CString::new(s).unwrap();
    c_str.into_raw() as i64
}

extern "C" fn olive_int(val: i64) -> i64 {
    val
}

extern "C" fn olive_float(val: i64) -> f64 {
    val as f64
}

extern "C" fn olive_bool(val: i64) -> i64 {
    if val != 0 { 1 } else { 0 }
}

extern "C" fn olive_bool_from_float(val: f64) -> i64 {
    if val != 0.0 { 1 } else { 0 }
}

extern "C" fn olive_float_to_str(val: f64) -> i64 {
    let s = format!("{}", val);
    let c_str = std::ffi::CString::new(s).unwrap();
    c_str.into_raw() as i64
}

extern "C" fn olive_float_to_int(val: f64) -> i64 {
    val as i64
}

extern "C" fn olive_int_to_float(val: i64) -> f64 {
    val as f64
}

extern "C" fn olive_str_to_int(ptr: i64) -> i64 {
    if ptr == 0 { return 0; }
    let s = unsafe { std::ffi::CStr::from_ptr(ptr as *const i8) }.to_string_lossy();
    s.parse::<i64>().unwrap_or(0)
}

extern "C" fn olive_str_to_float(ptr: i64) -> f64 {
    if ptr == 0 { return 0.0; }
    let s = unsafe { std::ffi::CStr::from_ptr(ptr as *const i8) }.to_string_lossy();
    s.parse::<f64>().unwrap_or(0.0)
}


extern "C" fn olive_str_concat(l: i64, r: i64) -> i64 {
    let sl = if l == 0 { "".into() } else { unsafe { std::ffi::CStr::from_ptr(l as *const i8) }.to_string_lossy() };
    let sr = if r == 0 { "".into() } else { unsafe { std::ffi::CStr::from_ptr(r as *const i8) }.to_string_lossy() };
    let s = format!("{}{}", sl, sr);
    let c_str = std::ffi::CString::new(s).unwrap();
    c_str.into_raw() as i64
}

extern "C" fn olive_free(_ptr: i64) {
    // Leak memory for now.
}

extern "C" fn olive_copy(ptr: i64) -> i64 {
    if ptr == 0 { return 0; }
    unsafe {
        let s = std::ffi::CStr::from_ptr(ptr as *const i8).to_owned();
        s.into_raw() as i64
    }
}

extern "C" fn olive_str_eq(l: i64, r: i64) -> i64 {
    if l == r { return 1; }
    if l == 0 || r == 0 { return 0; }
    let sl = unsafe { std::ffi::CStr::from_ptr(l as *const i8) };
    let sr = unsafe { std::ffi::CStr::from_ptr(r as *const i8) };
    if sl == sr { 1 } else { 0 }
}

extern "C" fn olive_list_new(len: i64) -> i64 {
    let v: Vec<i64> = vec![0; len as usize];
    Box::into_raw(Box::new(v)) as i64
}

extern "C" fn olive_list_set(list: i64, index: i64, val: i64) {
    if list == 0 { return; }
    let v = unsafe { &mut *(list as *mut Vec<i64>) };
    if (index as usize) < v.len() {
        v[index as usize] = val;
    }
}

extern "C" fn olive_list_get(list: i64, index: i64) -> i64 {
    if list == 0 { return 0; }
    let v = unsafe { &*(list as *const Vec<i64>) };
    if (index as usize) < v.len() {
        v[index as usize]
    } else {
        0
    }
}

extern "C" fn olive_obj_new() -> i64 {
    let m: HashMap<String, i64> = HashMap::default();
    Box::into_raw(Box::new(m)) as i64
}

extern "C" fn olive_copy_float(val: f64) -> f64 {
    val
}

extern "C" fn olive_obj_set(obj: i64, attr: i64, val: i64) -> i64 {
    if obj == 0 || attr == 0 { return obj; }
    let m = unsafe { &mut *(obj as *mut HashMap<String, i64>) };
    let s = unsafe { std::ffi::CStr::from_ptr(attr as *const i8) }.to_string_lossy().into_owned();
    m.insert(s, val);
    obj
}

extern "C" fn olive_str_len(s: i64) -> i64 {
    if s == 0 { return 0; }
    let s = unsafe { std::ffi::CStr::from_ptr(s as *const i8) };
    s.to_bytes().len() as i64
}

extern "C" fn olive_str_get(s: i64, i: i64) -> i64 {
    if s == 0 { return 0; }
    let s = unsafe { std::ffi::CStr::from_ptr(s as *const i8) }.to_bytes();
    if (i as usize) < s.len() {
        s[i as usize] as i64
    } else {
        0
    }
}

extern "C" fn olive_obj_get(obj: i64, attr: i64) -> i64 {
    if obj == 0 || attr == 0 { return 0; }
    let m = unsafe { &*(obj as *const HashMap<String, i64>) };
    let s = unsafe { std::ffi::CStr::from_ptr(attr as *const i8) }.to_string_lossy();
    *m.get(s.as_ref()).unwrap_or(&0)
}

extern "C" fn olive_memo_get(name_ptr: i64, is_tuple: i64) -> i64 {
    use std::sync::{Mutex, OnceLock};
    static GLOBAL_CACHES_INT: OnceLock<Mutex<HashMap<i64, i64>>> = OnceLock::new();
    static GLOBAL_CACHES_TUPLE: OnceLock<Mutex<HashMap<i64, i64>>> = OnceLock::new();
    
    if is_tuple == 0 {
        let caches_mutex = GLOBAL_CACHES_INT.get_or_init(|| Mutex::new(HashMap::default()));
        let mut caches = caches_mutex.lock().unwrap();
        if let Some(&cache) = caches.get(&name_ptr) {
            cache
        } else {
            let m: HashMap<i64, i64> = HashMap::default();
            let new_cache = Box::into_raw(Box::new(m)) as i64;
            caches.insert(name_ptr, new_cache);
            new_cache
        }
    } else {
        let caches_mutex = GLOBAL_CACHES_TUPLE.get_or_init(|| Mutex::new(HashMap::default()));
        let mut caches = caches_mutex.lock().unwrap();
        if let Some(&cache) = caches.get(&name_ptr) {
            cache
        } else {
            let m: HashMap<Vec<i64>, i64> = HashMap::default();
            let new_cache = Box::into_raw(Box::new(m)) as i64;
            caches.insert(name_ptr, new_cache);
            new_cache
        }
    }
}

extern "C" fn olive_cache_get(cache: i64, key: i64) -> i64 {
    if cache == 0 { return 0; }
    let m = unsafe { &*(cache as *const HashMap<i64, i64>) };
    *m.get(&key).unwrap_or(&0)
}

extern "C" fn olive_cache_has(cache: i64, key: i64) -> i64 {
    if cache == 0 { return 0; }
    let m = unsafe { &*(cache as *const HashMap<i64, i64>) };
    if m.contains_key(&key) { 1 } else { 0 }
}

extern "C" fn olive_cache_set(cache: i64, key: i64, val: i64) -> i64 {
    if cache == 0 { return cache; }
    let m = unsafe { &mut *(cache as *mut HashMap<i64, i64>) };
    m.insert(key, val);
    cache
}

extern "C" fn olive_cache_has_tuple(cache: i64, key_ptr: i64) -> i64 {
    if cache == 0 || key_ptr == 0 { return 0; }
    let m = unsafe { &*(cache as *const HashMap<Vec<i64>, i64>) };
    let v = unsafe { &*(key_ptr as *const Vec<i64>) };
    if m.contains_key(v) { 1 } else { 0 }
}

extern "C" fn olive_cache_get_tuple(cache: i64, key_ptr: i64) -> i64 {
    if cache == 0 || key_ptr == 0 { return 0; }
    let m = unsafe { &*(cache as *const HashMap<Vec<i64>, i64>) };
    let v = unsafe { &*(key_ptr as *const Vec<i64>) };
    *m.get(v).unwrap_or(&0)
}

extern "C" fn olive_cache_set_tuple(cache: i64, key_ptr: i64, val: i64) -> i64 {
    if cache == 0 || key_ptr == 0 { return cache; }
    let m = unsafe { &mut *(cache as *mut HashMap<Vec<i64>, i64>) };
    let v = unsafe { &*(key_ptr as *const Vec<i64>) };
    m.insert(v.clone(), val);
    cache
}
