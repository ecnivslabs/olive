use crate::mir::{
    Constant, Local, MirFunction, Operand, Rvalue, Statement, StatementKind, Terminator,
    TerminatorKind,
};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use rustc_hash::FxHashMap as HashMap;

#[repr(C)]
struct StableVec {
    ptr: *mut i64,
    cap: usize,
    len: usize,
}

pub struct CraneliftCodegen<'a> {
    functions: &'a [MirFunction],
    module: JITModule,
    func_ids: HashMap<String, FuncId>,
    string_ids: HashMap<String, DataId>,
}

impl<'a> CraneliftCodegen<'a> {
    pub fn new(functions: &'a [MirFunction]) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        flag_builder.set("enable_alias_analysis", "true").unwrap();
        flag_builder.set("enable_verifier", "false").unwrap();
        let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
            panic!("host machine is not supported: {}", msg);
        });
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|msg| panic!("host machine is not supported: {}", msg))
            .unwrap();

        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        builder.symbol("__olive_print_int", olive_print as *const u8);
        builder.symbol("__olive_print_float", olive_print_float as *const u8);
        builder.symbol("__olive_print_str", olive_print_str as *const u8);
        builder.symbol("__olive_print_list", olive_print_list as *const u8);
        builder.symbol("__olive_print_obj", olive_print_obj as *const u8);
        builder.symbol("__olive_str", olive_str as *const u8);
        builder.symbol("__olive_int", olive_int as *const u8);
        builder.symbol("__olive_float", olive_float as *const u8);
        builder.symbol("__olive_bool", olive_bool as *const u8);
        builder.symbol(
            "__olive_bool_from_float",
            olive_bool_from_float as *const u8,
        );
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
        builder.symbol(
            "__olive_cache_has_tuple",
            olive_cache_has_tuple as *const u8,
        );
        builder.symbol(
            "__olive_cache_get_tuple",
            olive_cache_get_tuple as *const u8,
        );
        builder.symbol(
            "__olive_cache_set_tuple",
            olive_cache_set_tuple as *const u8,
        );
        builder.symbol("__olive_str_len", olive_str_len as *const u8);
        builder.symbol("__olive_str_get", olive_str_get as *const u8);
        builder.symbol("__olive_time_now", olive_time_now as *const u8);
        builder.symbol("__olive_time_monotonic", olive_time_monotonic as *const u8);
        builder.symbol("__olive_time_sleep", olive_time_sleep as *const u8);
        builder.symbol("__olive_str_slice", olive_str_slice as *const u8);
        builder.symbol("__olive_str_char", olive_str_char as *const u8);
        builder.symbol("__olive_file_read", olive_file_read as *const u8);
        builder.symbol("__olive_file_write", olive_file_write as *const u8);
        builder.symbol("__olive_free_str", olive_free_str as *const u8);
        builder.symbol("__olive_free_list", olive_free_list as *const u8);
        builder.symbol("__olive_free_obj", olive_free_obj as *const u8);
        builder.symbol("__olive_pow", olive_pow as *const u8);
        builder.symbol("__olive_in_list", olive_in_list as *const u8);
        builder.symbol("__olive_in_obj", olive_in_obj as *const u8);
        builder.symbol("__olive_list_append", olive_list_append as *const u8);
        builder.symbol("__olive_enum_new", olive_enum_new as *const u8);
        builder.symbol("__olive_enum_tag", olive_enum_tag as *const u8);
        builder.symbol("__olive_enum_get", olive_enum_get as *const u8);
        builder.symbol("__olive_enum_set", olive_enum_set as *const u8);
        builder.symbol("__olive_free_enum", olive_free_enum as *const u8);
        builder.symbol("__olive_set_add", olive_set_add as *const u8);
        builder.symbol("__olive_iter", olive_iter as *const u8);
        builder.symbol("__olive_next", olive_next as *const u8);
        builder.symbol("__olive_has_next", olive_has_next as *const u8);
        builder.symbol("__olive_pow_float", olive_pow_float as *const u8);
        let module = JITModule::new(builder);

        Self {
            functions,
            module,
            func_ids: HashMap::default(),
            string_ids: HashMap::default(),
        }
    }

    pub fn generate(&mut self) {
        let needed = collect_needed_imports(self.functions);

        let mut sig_i64_i64 = self.module.make_signature();
        sig_i64_i64.params.push(AbiParam::new(types::I64));
        sig_i64_i64.returns.push(AbiParam::new(types::I64));

        let mut sig_f64_i64 = self.module.make_signature();
        sig_f64_i64.params.push(AbiParam::new(types::F64));
        sig_f64_i64.returns.push(AbiParam::new(types::I64));

        let mut sig_i64_f64 = self.module.make_signature();
        sig_i64_f64.params.push(AbiParam::new(types::I64));
        sig_i64_f64.returns.push(AbiParam::new(types::F64));

        let mut sig_f64_f64 = self.module.make_signature();
        sig_f64_f64.params.push(AbiParam::new(types::F64));
        sig_f64_f64.returns.push(AbiParam::new(types::F64));

        let mut sig_void_i64 = self.module.make_signature();
        sig_void_i64.returns.push(AbiParam::new(types::I64));

        let mut sig_void_f64 = self.module.make_signature();
        sig_void_f64.returns.push(AbiParam::new(types::F64));

        let mut sig_f64_void = self.module.make_signature();
        sig_f64_void.params.push(AbiParam::new(types::F64));

        let mut sig_i64_i64_i64 = self.module.make_signature();
        sig_i64_i64_i64.params.push(AbiParam::new(types::I64));
        sig_i64_i64_i64.params.push(AbiParam::new(types::I64));
        sig_i64_i64_i64.returns.push(AbiParam::new(types::I64));

        let mut sig_i64_i64_void = self.module.make_signature();
        sig_i64_i64_void.params.push(AbiParam::new(types::I64));
        sig_i64_i64_void.params.push(AbiParam::new(types::I64));

        let mut sig_f64_f64_f64 = self.module.make_signature();
        sig_f64_f64_f64.params.push(AbiParam::new(types::F64));
        sig_f64_f64_f64.params.push(AbiParam::new(types::F64));
        sig_f64_f64_f64.returns.push(AbiParam::new(types::F64));

        let mut sig_i64_i64_i64_i64 = self.module.make_signature();
        sig_i64_i64_i64_i64.params.push(AbiParam::new(types::I64));
        sig_i64_i64_i64_i64.params.push(AbiParam::new(types::I64));
        sig_i64_i64_i64_i64.params.push(AbiParam::new(types::I64));
        sig_i64_i64_i64_i64.returns.push(AbiParam::new(types::I64));

        let mut sig_i64_void = self.module.make_signature();
        sig_i64_void.params.push(AbiParam::new(types::I64));

        let mut sig_i64_i64_i64_void = self.module.make_signature();
        sig_i64_i64_i64_void.params.push(AbiParam::new(types::I64));
        sig_i64_i64_i64_void.params.push(AbiParam::new(types::I64));
        sig_i64_i64_i64_void.params.push(AbiParam::new(types::I64));

        let imports: &[(&str, &cranelift::prelude::Signature)] = &[
            ("__olive_print_int", &sig_i64_i64),
            ("__olive_print_str", &sig_i64_i64),
            ("__olive_print_list", &sig_i64_i64),
            ("__olive_print_obj", &sig_i64_i64),
            ("__olive_str", &sig_i64_i64),
            ("__olive_int", &sig_i64_i64),
            ("__olive_bool", &sig_i64_i64),
            ("__olive_str_to_int", &sig_i64_i64),
            ("__olive_copy", &sig_i64_i64),
            ("__olive_list_new", &sig_i64_i64),
            ("__olive_str_len", &sig_i64_i64),
            ("__olive_print_float", &sig_f64_i64),
            ("__olive_float_to_str", &sig_f64_i64),
            ("__olive_float_to_int", &sig_f64_i64),
            ("__olive_bool_from_float", &sig_f64_i64),
            ("__olive_str_to_float", &sig_i64_f64),
            ("__olive_int_to_float", &sig_i64_f64),
            ("__olive_float", &sig_i64_f64),
            ("__olive_copy_float", &sig_f64_f64),
            ("__olive_obj_new", &sig_void_i64),
            ("__olive_str_concat", &sig_i64_i64_i64),
            ("__olive_str_eq", &sig_i64_i64_i64),
            ("__olive_list_get", &sig_i64_i64_i64),
            ("__olive_str_get", &sig_i64_i64_i64),
            ("__olive_cache_get", &sig_i64_i64_i64),
            ("__olive_cache_has", &sig_i64_i64_i64),
            ("__olive_obj_get", &sig_i64_i64_i64),
            ("__olive_memo_get", &sig_i64_i64_i64),
            ("__olive_cache_has_tuple", &sig_i64_i64_i64),
            ("__olive_cache_get_tuple", &sig_i64_i64_i64),
            ("__olive_list_set", &sig_i64_i64_i64_i64),
            ("__olive_obj_set", &sig_i64_i64_i64_i64),
            ("__olive_cache_set", &sig_i64_i64_i64_i64),
            ("__olive_cache_set_tuple", &sig_i64_i64_i64_i64),
            ("__olive_free", &sig_i64_void),
            ("__olive_free_str", &sig_i64_void),
            ("__olive_free_list", &sig_i64_void),
            ("__olive_free_obj", &sig_i64_void),
            ("__olive_pow", &sig_i64_i64_i64),
            ("__olive_pow_float", &sig_f64_f64_f64),
            ("__olive_in_list", &sig_i64_i64_i64),
            ("__olive_in_obj", &sig_i64_i64_i64),
            ("__olive_list_append", &sig_i64_i64_void),
            ("__olive_set_add", &sig_i64_i64_void),
            ("__olive_iter", &sig_i64_i64),
            ("__olive_next", &sig_i64_i64),
            ("__olive_has_next", &sig_i64_i64),
            ("__olive_time_now", &sig_void_f64),
            ("__olive_time_sleep", &sig_f64_void),
            ("__olive_enum_new", &sig_i64_i64_i64),
            ("__olive_enum_tag", &sig_i64_i64),
            ("__olive_enum_get", &sig_i64_i64_i64),
            ("__olive_enum_set", &sig_i64_i64_i64_void),
            ("__olive_free_enum", &sig_i64_void),
        ];

        for &(name, sig) in imports {
            if !needed.contains(name) {
                continue;
            }
            let id = self
                .module
                .declare_function(name, Linkage::Import, sig)
                .unwrap();
            self.func_ids.insert(name.to_string(), id);
        }

        for func in self.functions {
            let mut sig = self.module.make_signature();
            for i in 0..func.arg_count {
                let ty = &func.locals[i + 1].ty;
                sig.params.push(AbiParam::new(cl_type(ty)));
            }
            let ret_ty = &func.locals[0].ty;
            sig.returns.push(AbiParam::new(cl_type(ret_ty)));

            let func_id = self
                .module
                .declare_function(&func.name, Linkage::Export, &sig)
                .unwrap();
            self.func_ids.insert(func.name.clone(), func_id);
        }

        for func in self.functions {
            self.collect_strings(func);
        }

        let func_count = self.functions.len();
        for i in 0..func_count {
            let func = self.functions[i].clone();
            self.translate_function(&func);
        }
    }

    fn collect_strings(&mut self, func: &MirFunction) {
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(_, rval) = &stmt.kind {
                    self.collect_strings_in_rvalue(rval);
                }
            }
        }
    }

    fn collect_strings_in_rvalue(&mut self, rval: &Rvalue) {
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) | Rvalue::GetAttr(op, _) => {
                self.collect_strings_in_operand(op);
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
                self.collect_strings_in_operand(l);
                self.collect_strings_in_operand(r);
            }
            Rvalue::Call { func, args } => {
                self.collect_strings_in_operand(func);
                for arg in args {
                    self.collect_strings_in_operand(arg);
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for op in ops {
                    self.collect_strings_in_operand(op);
                }
            }
            _ => {}
        }
    }

    fn collect_strings_in_operand(&mut self, op: &Operand) {
        if let Operand::Constant(Constant::Str(s)) = op {
            if !self.string_ids.contains_key(s) {
                let mut data_ctx = DataDescription::new();
                let mut bytes = s.as_bytes().to_vec();
                bytes.push(0);
                data_ctx.define(bytes.into_boxed_slice());

                let name = format!("str_{}", self.string_ids.len());
                let id = self
                    .module
                    .declare_data(&name, Linkage::Export, false, false)
                    .unwrap();
                self.module.define_data(id, &data_ctx).unwrap();
                self.string_ids.insert(s.clone(), id);
            }
        }
    }
}

fn collect_needed_imports(functions: &[MirFunction]) -> std::collections::HashSet<&'static str> {
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
                                OliveType::Dict(_, _) | OliveType::Class(_) => {
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

fn scan_rvalue_imports(
    func_mir: &MirFunction,
    rval: &Rvalue,
    needed: &mut std::collections::HashSet<&'static str>,
) {
    match rval {
        Rvalue::Call { func, args } => {
            if let Operand::Constant(Constant::Function(name)) = func {
                if let Some(r) = resolve_builtin_import(func_mir, name, args) {
                    needed.insert(r);
                }
            }
        }
        Rvalue::BinaryOp(op, lhs, _) => {
            use crate::parser::BinOp::*;
            match op {
                Add => {
                    if is_str_op(func_mir, lhs) {
                        needed.insert("__olive_str_concat");
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
        Rvalue::GetIndex(obj, _) => {
            needed.insert("__olive_list_get");
            if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                let ty = &func_mir.locals[loc.0].ty;
                if matches!(ty, OliveType::Str) {
                    needed.insert("__olive_str_get");
                } else if matches!(ty, OliveType::Enum(_)) {
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
                }
                AggregateKind::EnumVariant(_) => {
                    needed.insert("__olive_enum_new");
                    needed.insert("__olive_enum_set");
                }
                _ => {
                    needed.insert("__olive_list_new");
                    needed.insert("__olive_list_append");
                }
            }
        }
        _ => {}
    }
}

fn resolve_builtin_import(
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
            "__olive_str_get" => Some("__olive_str_get"),
            "__olive_str_concat" => Some("__olive_str_concat"),
            "__olive_str_eq" => Some("__olive_str_eq"),
            "__olive_obj_new" => Some("__olive_obj_new"),
            "__olive_obj_get" => Some("__olive_obj_get"),
            "__olive_obj_set" => Some("__olive_obj_set"),
            "__olive_pow" => Some("__olive_pow"),
            "__olive_in_list" => Some("__olive_in_list"),
            "__olive_in_obj" => Some("__olive_in_obj"),
            "__olive_set_add" => Some("__olive_set_add"),
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
            "__olive_enum_get" => Some("__olive_enum_get"),
            "__olive_enum_set" => Some("__olive_enum_set"),
            _ => None,
        };
    }
    match name {
        "print" | "str" | "int" | "float" | "bool" | "iter" | "next" | "has_next"
            if !args.is_empty() =>
        {
            let mut arg_type = OliveType::Int;
            match &args[0] {
                Operand::Constant(Constant::Str(_)) => arg_type = OliveType::Str,
                Operand::Constant(Constant::Float(_)) => arg_type = OliveType::Float,
                Operand::Copy(l) | Operand::Move(l) => arg_type = func_mir.locals[l.0].ty.clone(),
                _ => {}
            }
            let mut current_ty = &arg_type;
            while let OliveType::Ref(inner) | OliveType::MutRef(inner) = current_ty {
                current_ty = inner;
            }
            Some(match name {
                "print" => match current_ty {
                    OliveType::Str => "__olive_print_str",
                    OliveType::Float => "__olive_print_float",
                    t if matches!(
                        t,
                        OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_)
                    ) =>
                    {
                        "__olive_print_list"
                    }
                    t if matches!(t, OliveType::Dict(_, _) | OliveType::Class(_)) => {
                        "__olive_print_obj"
                    }
                    _ => "__olive_print_int",
                },
                "str" => match current_ty {
                    OliveType::Str => "__olive_copy",
                    OliveType::Float => "__olive_float_to_str",
                    _ => "__olive_str",
                },
                "int" => match current_ty {
                    OliveType::Float => "__olive_float_to_int",
                    OliveType::Str => "__olive_str_to_int",
                    _ => "__olive_int",
                },
                "float" => match current_ty {
                    OliveType::Float => "__olive_copy_float",
                    OliveType::Int => "__olive_int_to_float",
                    OliveType::Str => "__olive_str_to_float",
                    _ => "__olive_float",
                },
                "bool" => {
                    if *current_ty == OliveType::Float {
                        "__olive_bool_from_float"
                    } else {
                        "__olive_bool"
                    }
                }
                "iter" => "__olive_iter",
                "next" => "__olive_next",
                "has_next" => "__olive_has_next",
                _ => return None,
            })
        }
        "list_new" => Some("__olive_list_new"),
        // Module functions (math)
        "math::sin" => Some("__olive_math_sin"),
        "math::cos" => Some("__olive_math_cos"),
        "math::tan" => Some("__olive_math_tan"),
        "math::asin" => Some("__olive_math_asin"),
        "math::acos" => Some("__olive_math_acos"),
        "math::atan" => Some("__olive_math_atan"),
        "math::atan2" => Some("__olive_math_atan2"),
        "math::log" => Some("__olive_math_log"),
        "math::log10" => Some("__olive_math_log10"),
        "math::exp" => Some("__olive_math_exp"),
        // Module functions (time)
        "time::time" | "time::now" => Some("__olive_time_now"),
        "time::sleep" => Some("__olive_time_sleep"),
        // Module functions (random)
        "random::seed" => Some("__olive_random_seed"),
        "random::random" => Some("__olive_random_get"),
        // Module functions (json)
        _ => None,
    }
}

fn is_str_op(func_mir: &MirFunction, op: &Operand) -> bool {
    match op {
        Operand::Constant(Constant::Str(_)) => true,
        Operand::Copy(loc) | Operand::Move(loc) => func_mir.locals[loc.0].ty == OliveType::Str,
        _ => false,
    }
}

fn is_float_op(func_mir: &MirFunction, op: &Operand) -> bool {
    match op {
        Operand::Constant(Constant::Float(_)) => true,
        Operand::Copy(loc) | Operand::Move(loc) => func_mir.locals[loc.0].ty == OliveType::Float,
        _ => false,
    }
}

impl<'a> CraneliftCodegen<'a> {
    pub fn finalize(&mut self) {
        self.module.finalize_definitions().unwrap();
    }

    pub fn get_function(&mut self, name: &str) -> Option<*const u8> {
        let func_id = self.func_ids.get(name)?;
        Some(self.module.get_finalized_function(*func_id))
    }

    fn translate_function(&mut self, func: &MirFunction) {
        let mut ctx = self.module.make_context();

        for i in 0..func.arg_count {
            let ty = &func.locals[i + 1].ty;
            ctx.func.signature.params.push(AbiParam::new(cl_type(ty)));
        }
        let ret_ty = &func.locals[0].ty;
        ctx.func.signature.returns.push(AbiParam::new(cl_type(ret_ty)));

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

    fn translate_statement(
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

                // bitcast if types mismatch
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
                let mut is_list = true;
                if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    if matches!(func_mir.locals[loc.0].ty, OliveType::Str) {
                        is_list = false;
                    }
                }

                let o = Self::translate_operand(builder, obj, vars, string_ids, module);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module);
                let v = Self::translate_operand(builder, val_op, vars, string_ids, module);

                if is_list {
                    let data_ptr =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted().with_readonly(), o, 0);

                    let offset = builder.ins().imul_imm(i, 8);
                    let addr = builder.ins().iadd(data_ptr, offset);
                    builder.ins().store(MemFlags::trusted(), v, addr, 0);
                } else {
                    let set_id = func_ids.get("__olive_list_set").unwrap();
                    let local_func = module.declare_func_in_func(*set_id, builder.func);
                    builder.ins().call(local_func, &[o, i, v]);
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
                    OliveType::Dict(_, _) | OliveType::Class(_) => "__olive_free_obj",
                    OliveType::Enum(_) => "__olive_free_enum",
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

                let data_ptr = builder.ins().load(types::I64, MemFlags::trusted(), o, 0);
                let offset = builder.ins().imul_imm(i, 8);
                let addr = builder.ins().iadd(data_ptr, offset);
                builder.ins().store(MemFlags::trusted(), v, addr, 0);
            }
        }
    }

    fn translate_rvalue(
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
                        || name == "has_next")
                        && !args.is_empty()
                    {
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

                        match name.as_str() {
                            "print" => {
                                if *current_ty == OliveType::Str {
                                    "__olive_print_str"
                                } else if *current_ty == OliveType::Float {
                                    "__olive_print_float"
                                } else if matches!(
                                    current_ty,
                                    OliveType::List(_) | OliveType::Tuple(_) | OliveType::Set(_)
                                ) {
                                    "__olive_print_list"
                                } else if matches!(
                                    current_ty,
                                    OliveType::Dict(_, _) | OliveType::Class(_)
                                ) {
                                    "__olive_print_obj"
                                } else {
                                    "__olive_print_int"
                                }
                            }
                            "str" => {
                                if *current_ty == OliveType::Str {
                                    "__olive_copy"
                                } else if *current_ty == OliveType::Float {
                                    "__olive_float_to_str"
                                } else {
                                    "__olive_str"
                                }
                            }
                            "int" => {
                                if *current_ty == OliveType::Int {
                                    "__olive_int"
                                } else if *current_ty == OliveType::Float {
                                    "__olive_float_to_int"
                                } else if *current_ty == OliveType::Str {
                                    "__olive_str_to_int"
                                } else {
                                    "__olive_int"
                                }
                            }
                            "float" => {
                                if *current_ty == OliveType::Float {
                                    "__olive_copy_float"
                                } else if *current_ty == OliveType::Int {
                                    "__olive_int_to_float"
                                } else if *current_ty == OliveType::Str {
                                    "__olive_str_to_float"
                                } else {
                                    "__olive_float"
                                }
                            }
                            "bool" => {
                                if *current_ty == OliveType::Float {
                                    "__olive_bool_from_float"
                                } else {
                                    "__olive_bool"
                                }
                            }
                            "iter" => "__olive_iter",
                            "next" => "__olive_next",
                            "has_next" => "__olive_has_next",
                            _ => name.as_str(),
                        }
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

                        if is_str {
                            let concat_func_id = func_ids.get("__olive_str_concat").unwrap();
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
                    FloorDiv => builder.ins().sdiv(l, r),
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
                    Is => {
                        let res = builder.ins().icmp(IntCC::Equal, l, r);
                        builder.ins().uextend(types::I64, res)
                    }
                    IsNot => {
                        let res = builder.ins().icmp(IntCC::NotEqual, l, r);
                        builder.ins().uextend(types::I64, res)
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
                        let func_name = if is_float { "__olive_pow_float" } else { "__olive_pow" };
                        let pow_id = func_ids.get(func_name).unwrap();
                        let local_func = module.declare_func_in_func(*pow_id, builder.func);
                        let inst = builder.ins().call(local_func, &[l, r]);
                        builder.inst_results(inst)[0]
                    }
                    In => {
                        let mut is_obj = false;
                        if let Operand::Copy(loc) | Operand::Move(loc) = rhs {
                            let ty = &func_mir.locals[loc.0].ty;
                            if matches!(ty, OliveType::Dict(_, _) | OliveType::Class(_)) {
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
                            let ty = &func_mir.locals[loc.0].ty;
                            if matches!(ty, OliveType::Dict(_, _) | OliveType::Class(_)) {
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
            Rvalue::GetIndex(obj, idx) => {
                let mut is_list = true;
                if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    if matches!(func_mir.locals[loc.0].ty, OliveType::Str) {
                        is_list = false;
                    }
                }

                let mut is_enum = false;
                if let Operand::Copy(loc) | Operand::Move(loc) = obj {
                    if let OliveType::Enum(_) = &func_mir.locals[loc.0].ty {
                        is_enum = true;
                    }
                }

                let o = Self::translate_operand(builder, obj, vars, string_ids, module);
                let i = Self::translate_operand(builder, idx, vars, string_ids, module);

                if is_enum {
                    let get_id = func_ids.get("__olive_enum_get").unwrap();
                    let local_func = module.declare_func_in_func(*get_id, builder.func);
                    let inst = builder.ins().call(local_func, &[o, i]);
                    builder.inst_results(inst)[0]
                } else if is_list {
                    let data_ptr =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted().with_readonly(), o, 0);

                    let offset = builder.ins().imul_imm(i, 8);
                    let addr = builder.ins().iadd(data_ptr, offset);
                    builder.ins().load(types::I64, MemFlags::trusted(), addr, 0)
                } else {
                    let get_id = func_ids.get("__olive_list_get").unwrap();
                    let local_func = module.declare_func_in_func(*get_id, builder.func);
                    let inst = builder.ins().call(local_func, &[o, i]);
                    builder.inst_results(inst)[0]
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
                    AggregateKind::EnumVariant(tag) => {
                        let tag_val = builder.ins().iconst(types::I64, *tag as i64);
                        let count = builder.ins().iconst(types::I64, ops.len() as i64);
                        let new_id = func_ids.get("__olive_enum_new").unwrap();
                        let new_func = module.declare_func_in_func(*new_id, builder.func);
                        let inst = builder.ins().call(new_func, &[tag_val, count]);
                        let enum_ptr = builder.inst_results(inst)[0];

                        let set_id = func_ids.get("__olive_enum_set").unwrap();
                        let set_func = module.declare_func_in_func(*set_id, builder.func);

                        for (i, op) in ops.iter().enumerate() {
                            let idx = builder.ins().iconst(types::I64, i as i64);
                            let val = Self::translate_operand(builder, op, vars, string_ids, module);
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
                        let new_id = func_ids.get("__olive_list_new").unwrap();
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
                        let count = builder.ins().iconst(types::I64, ops.len() as i64);
                        let new_id = func_ids.get("__olive_list_new").unwrap();
                        let new_func = module.declare_func_in_func(*new_id, builder.func);
                        let inst = builder.ins().call(new_func, &[count]);
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
                let data_ptr = builder.ins().load(types::I64, MemFlags::trusted(), o, 0);
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

    fn translate_operand(
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

    fn translate_terminator(
        builder: &mut FunctionBuilder,
        term: &Terminator,
        blocks: &[Block],
        vars: &HashMap<Local, Variable>,
        string_ids: &HashMap<String, DataId>,
        module: &mut JITModule,
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
        OliveType::Vector(inner, width) => match &**inner {
            OliveType::Int => types::I64.by(*width as u32).expect("invalid vector width"),
            OliveType::Float => types::F64.by(*width as u32).expect("invalid vector width"),
            _ => types::I64,
        },
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
        let ptr = val & !1;
        let s = unsafe { std::ffi::CStr::from_ptr(ptr as *const i8) };
        println!("{}", s.to_string_lossy());
    }
    0
}

extern "C" fn olive_print_list(ptr: i64) -> i64 {
    if ptr == 0 {
        println!("[]");
        return 0;
    }
    let v = unsafe { &*(ptr as *const StableVec) };
    print!("[");
    for i in 0..v.len {
        if i > 0 { print!(", "); }
        let elem = unsafe { *v.ptr.add(i) };
        print!("{}", elem);
    }
    println!("]");
    0
}

extern "C" fn olive_print_obj(ptr: i64) -> i64 {
    if ptr == 0 {
        println!("{{}}");
        return 0;
    }
    let m = unsafe { &*(ptr as *const HashMap<String, i64>) };
    print!("{{");
    for (i, (k, &v)) in m.iter().enumerate() {
        if i > 0 { print!(", "); }
        print!("'{}': {}", k, v);
    }
    println!("}}");
    0
}

extern "C" fn olive_str(val: i64) -> i64 {
    let s = val.to_string();
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
    olive_str_internal(&s)
}

extern "C" fn olive_float_to_int(val: f64) -> i64 {
    val as i64
}

extern "C" fn olive_int_to_float(val: i64) -> f64 {
    val as f64
}

extern "C" fn olive_str_to_int(ptr: i64) -> i64 {
    if ptr == 0 { return 0; }
    let p = ptr & !1;
    let s = unsafe { std::ffi::CStr::from_ptr(p as *const i8) };
    s.to_string_lossy().parse::<i64>().unwrap_or(0)
}

extern "C" fn olive_str_to_float(ptr: i64) -> f64 {
    if ptr == 0 { return 0.0; }
    let p = ptr & !1;
    let s = unsafe { std::ffi::CStr::from_ptr(p as *const i8) };
    s.to_string_lossy().parse::<f64>().unwrap_or(0.0)
}

extern "C" fn olive_str_concat(l: i64, r: i64) -> i64 {
    let sl = if l == 0 {
        "".to_string()
    } else {
        let p = l & !1;
        unsafe { std::ffi::CStr::from_ptr(p as *const i8).to_string_lossy().into_owned() }
    };
    let sr = if r == 0 {
        "".to_string()
    } else {
        let p = r & !1;
        unsafe { std::ffi::CStr::from_ptr(p as *const i8).to_string_lossy().into_owned() }
    };
    let s = format!("{}{}", sl, sr);
    olive_str_internal(&s)
}

extern "C" fn olive_free(_ptr: i64) {}

extern "C" fn olive_free_str(ptr: i64) {
    if ptr != 0 && (ptr & 1) == 0 {
        unsafe {
            let _ = std::ffi::CString::from_raw(ptr as *mut i8);
        }
    }
}


extern "C" fn olive_free_list(ptr: i64) {
    if ptr != 0 {
        unsafe {
            let s = Box::from_raw(ptr as *mut StableVec);
            if s.ptr != std::ptr::null_mut() {
                let _ = Vec::from_raw_parts(s.ptr, s.len, s.cap);
            }
        }
    }
}

extern "C" fn olive_free_obj(ptr: i64) {
    if ptr != 0 {
        unsafe {
            let _ = Box::from_raw(ptr as *mut HashMap<String, i64>);
        }
    }
}

extern "C" fn olive_pow(l: i64, r: i64) -> i64 {
    (l as f64).powf(r as f64) as i64
}

extern "C" fn olive_pow_float(l: f64, r: f64) -> f64 {
    l.powf(r)
}

extern "C" fn olive_in_list(val: i64, list: i64) -> i64 {
    if list == 0 {
        return 0;
    }
    unsafe {
        let ptr = list as *const i64;
        let len = *ptr;
        for i in 0..len {
            if *ptr.add((i + 1) as usize) == val {
                return 1;
            }
        }
    }
    0
}

extern "C" fn olive_obj_new() -> i64 {
    let m = Box::new(HashMap::<String, i64>::default());
    Box::into_raw(m) as i64
}

extern "C" fn olive_copy_float(val: f64) -> f64 {
    val
}

extern "C" fn olive_obj_set(obj_ptr: i64, attr: i64, val: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        return obj_ptr;
    }
    let m = unsafe { &mut *(obj_ptr as *mut HashMap<String, i64>) };
    let s = unsafe { std::ffi::CStr::from_ptr(attr as *const i8).to_string_lossy().into_owned() };
    m.insert(s, val);
    obj_ptr
}

extern "C" fn olive_obj_get(obj_ptr: i64, attr: i64) -> i64 {
    if obj_ptr == 0 || attr == 0 {
        return 0;
    }
    let m = unsafe { &*(obj_ptr as *const HashMap<String, i64>) };
    let s = unsafe { std::ffi::CStr::from_ptr(attr as *const i8).to_string_lossy() };
    *m.get(s.as_ref()).unwrap_or(&0)
}

extern "C" fn olive_memo_get(name_ptr: i64, is_tuple: i64) -> i64 {
    use std::sync::{Mutex, OnceLock};
    if is_tuple == 0 {
        static GLOBAL_CACHES_INT: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
        let caches_mutex = GLOBAL_CACHES_INT.get_or_init(|| Mutex::new(HashMap::default()));
        let mut caches = caches_mutex.lock().unwrap();
        let name = unsafe { std::ffi::CStr::from_ptr(name_ptr as *const i8).to_string_lossy().into_owned() };
        if let Some(&cache) = caches.get(&name) {
            cache
        } else {
            let m: HashMap<i64, i64> = HashMap::default();
            let new_cache = Box::into_raw(Box::new(m)) as i64;
            caches.insert(name, new_cache);
            new_cache
        }
    } else {
        static GLOBAL_CACHES_TUPLE: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
        let caches_mutex = GLOBAL_CACHES_TUPLE.get_or_init(|| Mutex::new(HashMap::default()));
        let mut caches = caches_mutex.lock().unwrap();
        let name = unsafe { std::ffi::CStr::from_ptr(name_ptr as *const i8).to_string_lossy().into_owned() };
        if let Some(&cache) = caches.get(&name) {
            cache
        } else {
            let m: HashMap<Vec<i64>, i64> = HashMap::default();
            let new_cache = Box::into_raw(Box::new(m)) as i64;
            caches.insert(name, new_cache);
            new_cache
        }
    }
}

extern "C" fn olive_cache_get(cache: i64, key: i64) -> i64 {
    if cache == 0 {
        return 0;
    }
    let m = unsafe { &*(cache as *const HashMap<i64, i64>) };
    *m.get(&key).unwrap_or(&0)
}

extern "C" fn olive_cache_has(cache: i64, key: i64) -> i64 {
    if cache == 0 {
        return 0;
    }
    let m = unsafe { &*(cache as *const HashMap<i64, i64>) };
    if m.contains_key(&key) { 1 } else { 0 }
}

extern "C" fn olive_cache_set(cache: i64, key: i64, val: i64) -> i64 {
    if cache == 0 {
        return cache;
    }
    let m = unsafe { &mut *(cache as *mut HashMap<i64, i64>) };
    m.insert(key, val);
    cache
}

fn read_tuple(ptr: i64) -> Vec<i64> {
    if ptr == 0 {
        return vec![];
    }
    unsafe {
        let p = ptr as *const i64;
        let len = *p as usize;
        let mut v = Vec::with_capacity(len);
        for i in 0..len {
            v.push(*(p.add(i + 1)));
        }
        v
    }
}

extern "C" fn olive_cache_has_tuple(cache: i64, key_ptr: i64) -> i64 {
    if cache == 0 || key_ptr == 0 {
        return 0;
    }
    let m = unsafe { &*(cache as *const HashMap<Vec<i64>, i64>) };
    let v = read_tuple(key_ptr);
    if m.contains_key(&v) { 1 } else { 0 }
}

extern "C" fn olive_cache_get_tuple(cache: i64, key_ptr: i64) -> i64 {
    if cache == 0 || key_ptr == 0 {
        return 0;
    }
    let m = unsafe { &*(cache as *const HashMap<Vec<i64>, i64>) };
    let v = read_tuple(key_ptr);
    *m.get(&v).unwrap_or(&0)
}

extern "C" fn olive_cache_set_tuple(cache: i64, key_ptr: i64, val: i64) -> i64 {
    if cache == 0 || key_ptr == 0 {
        return cache;
    }
    let m = unsafe { &mut *(cache as *mut HashMap<Vec<i64>, i64>) };
    let v = read_tuple(key_ptr);
    m.insert(v, val);
    cache
}
extern "C" fn olive_copy(ptr: i64) -> i64 {
    if ptr == 0 { return 0; }
    let p = ptr & !1;
    let s = unsafe { std::ffi::CStr::from_ptr(p as *const i8) };
    olive_str_internal(&s.to_string_lossy())
}

extern "C" fn olive_str_eq(l: i64, r: i64) -> i64 {
    if l == r { return 1; }
    if l == 0 || r == 0 { return 0; }
    let pl = l & !1;
    let pr = r & !1;
    let sl = unsafe { std::ffi::CStr::from_ptr(pl as *const i8) };
    let sr = unsafe { std::ffi::CStr::from_ptr(pr as *const i8) };
    if sl == sr { 1 } else { 0 }
}


extern "C" fn olive_list_new(len: i64) -> i64 {
    let mut v: Vec<i64> = vec![0i64; len as usize];
    let ptr = v.as_mut_ptr();
    let cap = v.capacity();
    let length = v.len();
    std::mem::forget(v);
    let stable = Box::new(StableVec {
        ptr,
        cap,
        len: length,
    });
    Box::into_raw(stable) as i64
}

extern "C" fn olive_list_set(list_ptr: i64, idx: i64, val: i64) {
    if list_ptr == 0 {
        return;
    }
    let s = unsafe { &mut *(list_ptr as *mut StableVec) };
    if (idx as usize) < s.len {
        unsafe {
            *s.ptr.add(idx as usize) = val;
        }
    }
}

extern "C" fn olive_list_get(list_ptr: i64, idx: i64) -> i64 {
    if list_ptr == 0 {
        return 0;
    }
    let s = unsafe { &*(list_ptr as *const StableVec) };
    if (idx as usize) < s.len {
        unsafe { *s.ptr.add(idx as usize) }
    } else {
        0
    }
}

extern "C" fn olive_list_append(list_ptr: i64, val: i64) {
    if list_ptr == 0 {
        return;
    }
    unsafe {
        let s = &mut *(list_ptr as *mut StableVec);
        let mut v = Vec::from_raw_parts(s.ptr, s.len, s.cap);
        v.push(val);
        s.ptr = v.as_mut_ptr();
        s.cap = v.capacity();
        s.len = v.len();
        std::mem::forget(v);
    }
}

extern "C" fn olive_in_obj(val: i64, obj_ptr: i64) -> i64 {
    if obj_ptr == 0 || val == 0 {
        return 0;
    }
    let m = unsafe { &*(obj_ptr as *const HashMap<String, i64>) };
    let s = unsafe { std::ffi::CStr::from_ptr(val as *const i8).to_string_lossy().into_owned() };
    if m.contains_key(&s) {
        1
    } else {
        0
    }
}

struct OliveIter {
    list_ptr: i64,
    index: usize,
}

extern "C" fn olive_iter(list_ptr: i64) -> i64 {
    let it = Box::new(OliveIter { list_ptr, index: 0 });
    Box::into_raw(it) as i64
}

extern "C" fn olive_has_next(iter_ptr: i64) -> i64 {
    if iter_ptr == 0 {
        return 0;
    }
    let it = unsafe { &*(iter_ptr as *const OliveIter) };
    if it.list_ptr == 0 {
        return 0;
    }
    let v = unsafe { &*(it.list_ptr as *const Vec<i64>) };
    if it.index < v.len() { 1 } else { 0 }
}

extern "C" fn olive_next(iter_ptr: i64) -> i64 {
    if iter_ptr == 0 {
        return 0;
    }
    let it = unsafe { &mut *(iter_ptr as *mut OliveIter) };
    if it.list_ptr == 0 {
        return 0;
    }
    let v = unsafe { &*(it.list_ptr as *const Vec<i64>) };
    if it.index < v.len() {
        let val = v[it.index];
        it.index += 1;
        val
    } else {
        0
    }
}

extern "C" fn olive_str_len(s: i64) -> i64 {
    if s == 0 { return 0; }
    let p = s & !1;
    let c_str = unsafe { std::ffi::CStr::from_ptr(p as *const i8) };
    c_str.to_bytes().len() as i64
}

extern "C" fn olive_str_get(s: i64, i: i64) -> i64 {
    if s == 0 { return 0; }
    let p = s & !1;
    let c_str = unsafe { std::ffi::CStr::from_ptr(p as *const i8) };
    let bytes = c_str.to_bytes();
    if (i as usize) < bytes.len() {
        bytes[i as usize] as i64
    } else {
        0
    }
}

extern "C" fn olive_set_add(list_ptr: i64, val: i64) {
    if list_ptr == 0 {
        return;
    }
    let v = unsafe { &mut *(list_ptr as *mut Vec<i64>) };
    if !v.contains(&val) {
        v.push(val);
    }
}

// time
extern "C" fn olive_time_now() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64()
}

extern "C" fn olive_time_monotonic() -> f64 {
    use std::time::Instant;
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_secs_f64()
}

extern "C" fn olive_time_sleep(secs: f64) {
    use std::{thread, time::Duration};
    thread::sleep(Duration::from_secs_f64(secs));
}

// string ops
extern "C" fn olive_str_slice(s: i64, start: i64, end: i64) -> i64 {
    if s == 0 { return 0; }
    let p = s & !1;
    let c_str = unsafe { std::ffi::CStr::from_ptr(p as *const i8) };
    let text = c_str.to_string_lossy();
    let chars: Vec<char> = text.chars().collect();
    let mut st = start;
    let mut en = end;
    if st < 0 { st = 0; }
    if en > chars.len() as i64 { en = chars.len() as i64; }
    if st >= en { return olive_str_internal(""); }
    let sliced: String = chars[st as usize..en as usize].iter().collect();
    olive_str_internal(&sliced)
}

extern "C" fn olive_str_char(s: i64, i: i64) -> i64 {
    if s == 0 { return 0; }
    let p = s & !1;
    let c_str = unsafe { std::ffi::CStr::from_ptr(p as *const i8) };
    let text = c_str.to_string_lossy();
    let chars: Vec<char> = text.chars().collect();
    if i < 0 || i >= chars.len() as i64 { return olive_str_internal(""); }
    let mut sliced = String::new();
    sliced.push(chars[i as usize]);
    olive_str_internal(&sliced)
}

// io
extern "C" fn olive_file_read(path: i64) -> i64 {
    if path == 0 { return 0; }
    let p = path & !1;
    let c_str = unsafe { std::ffi::CStr::from_ptr(p as *const i8) };
    let path_str = c_str.to_string_lossy();
    if let Ok(content) = std::fs::read_to_string(path_str.as_ref()) {
        olive_str_internal(&content)
    } else {
        0
    }
}

extern "C" fn olive_file_write(path: i64, data: i64) -> i64 {
    if path == 0 || data == 0 { return 0; }
    let p_path = path & !1;
    let p_data = data & !1;
    let c_path = unsafe { std::ffi::CStr::from_ptr(p_path as *const i8) };
    let c_data = unsafe { std::ffi::CStr::from_ptr(p_data as *const i8) };
    if std::fs::write(c_path.to_string_lossy().as_ref(), c_data.to_bytes()).is_ok() {
        1
    } else {
        0
    }
}

fn olive_str_internal(s: &str) -> i64 {
    let c_str = std::ffi::CString::new(s).unwrap();
    c_str.into_raw() as i64
}


#[repr(C)]
struct OliveEnum {
    tag: i64,
    payload_ptr: *mut i64,
    payload_len: usize,
}

extern "C" fn olive_enum_new(tag: i64, arg_count: i64) -> i64 {
    let mut payload = vec![0i64; arg_count as usize];
    let payload_ptr = payload.as_mut_ptr();
    let payload_len = payload.len();
    std::mem::forget(payload);
    let e = Box::new(OliveEnum {
        tag,
        payload_ptr,
        payload_len,
    });
    Box::into_raw(e) as i64
}

extern "C" fn olive_enum_tag(ptr: i64) -> i64 {
    if ptr == 0 {
        return -1;
    }
    let e = unsafe { &*(ptr as *const OliveEnum) };
    e.tag
}

extern "C" fn olive_enum_get(ptr: i64, index: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    let e = unsafe { &*(ptr as *const OliveEnum) };
    if (index as usize) < e.payload_len {
        unsafe { *e.payload_ptr.add(index as usize) }
    } else {
        0
    }
}

extern "C" fn olive_enum_set(ptr: i64, index: i64, val: i64) {
    if ptr == 0 {
        return;
    }
    let e = unsafe { &mut *(ptr as *mut OliveEnum) };
    if (index as usize) < e.payload_len {
        unsafe {
            *e.payload_ptr.add(index as usize) = val;
        }
    }
}

extern "C" fn olive_free_enum(ptr: i64) {
    if ptr == 0 {
        return;
    }
    unsafe {
        let e = Box::from_raw(ptr as *mut OliveEnum);
        let _ = Vec::from_raw_parts(e.payload_ptr, e.payload_len, e.payload_len);
    }
}


