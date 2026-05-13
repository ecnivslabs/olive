use crate::mir::{
    Constant, Local, MirFunction, Operand, Rvalue, Statement, StatementKind, Terminator,
    TerminatorKind,
};
use crate::semantic::types::Type as OliveType;
use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use rustc_hash::FxHashMap as HashMap;

const KIND_SM_FUTURE: i64 = 5;
const POLL_PENDING: i64 = i64::MIN;

struct SmAwaitPoint {
    bb_idx: usize,
    stmt_idx: usize,
    result_local: Local,
    sub_future_local: Local,
}

pub struct CraneliftCodegen<'a> {
    functions: &'a [MirFunction],
    module: JITModule,
    func_ids: HashMap<String, FuncId>,
    string_ids: HashMap<String, DataId>,
    _std_lib: Option<libloading::Library>,
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

        let mut std_lib = None;
        if let Ok(lib) = unsafe {
            libloading::Library::new("target/debug/libolive_std.so")
                .or_else(|_| libloading::Library::new("target/release/libolive_std.so"))
        } {
            unsafe {
                macro_rules! load {
                    ($name:expr, $bind:expr) => {
                        if let Ok(f) = lib.get::<unsafe extern "C" fn()>($name) {
                            builder.symbol($bind, *f as *const u8);
                        }
                    };
                }
                load!(b"olive_time_now", "__olive_time_now");
                load!(b"olive_time_monotonic", "__olive_time_monotonic");
                load!(b"olive_time_sleep", "__olive_time_sleep");
                load!(b"olive_pow", "__olive_pow");
                load!(b"olive_pow_float", "__olive_pow_float");
                load!(b"olive_math_sin", "__olive_math_sin");
                load!(b"olive_math_cos", "__olive_math_cos");
                load!(b"olive_math_tan", "__olive_math_tan");
                load!(b"olive_math_asin", "__olive_math_asin");
                load!(b"olive_math_acos", "__olive_math_acos");
                load!(b"olive_math_atan", "__olive_math_atan");
                load!(b"olive_math_atan2", "__olive_math_atan2");
                load!(b"olive_math_log", "__olive_math_log");
                load!(b"olive_math_log10", "__olive_math_log10");
                load!(b"olive_math_exp", "__olive_math_exp");
                load!(b"olive_random_seed", "__olive_random_seed");
                load!(b"olive_random_get", "__olive_random_get");
                load!(b"olive_random_int", "__olive_random_int");
                load!(b"olive_print", "__olive_print_int");
                load!(b"olive_print_float", "__olive_print_float");
                load!(b"olive_print_str", "__olive_print_str");
                load!(b"olive_print_list", "__olive_print_list");
                load!(b"olive_print_obj", "__olive_print_obj");
                load!(b"olive_str", "__olive_str");
                load!(b"olive_int", "__olive_int");
                load!(b"olive_float", "__olive_float");
                load!(b"olive_bool", "__olive_bool");
                load!(b"olive_bool_from_float", "__olive_bool_from_float");
                load!(b"olive_float_to_str", "__olive_float_to_str");
                load!(b"olive_float_to_int", "__olive_float_to_int");
                load!(b"olive_int_to_float", "__olive_int_to_float");
                load!(b"olive_str_to_int", "__olive_str_to_int");
                load!(b"olive_str_to_float", "__olive_str_to_float");
                load!(b"olive_str_concat", "__olive_str_concat");
                load!(b"olive_str_eq", "__olive_str_eq");
                load!(b"olive_str_len", "__olive_str_len");
                load!(b"olive_str_get", "__olive_str_get");
                load!(b"olive_copy", "__olive_copy");
                load!(b"olive_copy_float", "__olive_copy_float");
                load!(b"olive_list_new", "__olive_list_new");
                load!(b"olive_list_set", "__olive_list_set");
                load!(b"olive_list_get", "__olive_list_get");
                load!(b"olive_list_len", "__olive_list_len");
                load!(b"olive_list_append", "__olive_list_append");
                load!(b"olive_obj_new", "__olive_obj_new");
                load!(b"olive_obj_set", "__olive_obj_set");
                load!(b"olive_obj_get", "__olive_obj_get");
                load!(b"olive_obj_len", "__olive_obj_len");
                load!(b"olive_enum_new", "__olive_enum_new");
                load!(b"olive_enum_tag", "__olive_enum_tag");
                load!(b"olive_enum_type_id", "__olive_enum_type_id");
                load!(b"olive_enum_get", "__olive_enum_get");
                load!(b"olive_enum_set", "__olive_enum_set");
                load!(b"olive_free_enum", "__olive_free_enum");
                load!(b"olive_free_str", "__olive_free_str");
                load!(b"olive_free_list", "__olive_free_list");
                load!(b"olive_free_obj", "__olive_free_obj");
                load!(b"olive_iter", "__olive_iter");
                load!(b"olive_has_next", "__olive_has_next");
                load!(b"olive_next", "__olive_next");
                load!(b"olive_alloc", "__olive_alloc");
                load!(b"olive_cache_has", "__olive_cache_has");
                load!(b"olive_cache_get", "__olive_cache_get");
                load!(b"olive_cache_set", "__olive_cache_set");
                load!(b"olive_memo_get", "__olive_memo_get");
                load!(b"olive_cache_has_tuple", "__olive_cache_has_tuple");
                load!(b"olive_cache_get_tuple", "__olive_cache_get_tuple");
                load!(b"olive_cache_set_tuple", "__olive_cache_set_tuple");
                load!(b"olive_get_index_any", "__olive_get_index_any");
                load!(b"olive_set_index_any", "__olive_set_index_any");
                load!(b"olive_free_any", "__olive_free_any");
                load!(b"olive_free_any", "__olive_free");
                load!(b"olive_file_read", "__olive_file_read");
                load!(b"olive_file_write", "__olive_file_write");
                load!(b"olive_make_future", "__olive_make_future");
                load!(b"olive_await_future", "__olive_await");
                load!(b"olive_spawn_task", "__olive_spawn_task");
                load!(b"olive_free_future", "__olive_free_future");
                load!(b"olive_gather", "__olive_gather");
                load!(b"olive_select", "__olive_select");
                load!(b"olive_cancel_future", "__olive_cancel_future");
                load!(b"olive_sm_poll", "__olive_sm_poll");
                load!(b"olive_async_file_read", "__olive_async_file_read");
                load!(b"olive_async_file_write", "__olive_async_file_write");
                load!(b"olive_net_tcp_connect", "__olive_net_tcp_connect");
                load!(b"olive_net_tcp_send", "__olive_net_tcp_send");
                load!(b"olive_net_tcp_recv", "__olive_net_tcp_recv");
                load!(b"olive_net_tcp_close", "__olive_net_tcp_close");
                load!(b"olive_http_get", "__olive_http_get");
                load!(b"olive_http_post", "__olive_http_post");
                load!(b"olive_in_list", "__olive_in_list");
                load!(b"olive_in_obj", "__olive_in_obj");
                load!(b"olive_set_add", "__olive_set_add");
                load!(b"olive_set_new", "__olive_set_new");
                load!(b"olive_str_char", "__olive_str_char");
                load!(b"olive_str_slice", "__olive_str_slice");
                load!(b"olive_list_concat", "__olive_list_concat");
            }
            std_lib = Some(lib);
        }

        let module = JITModule::new(builder);

        Self {
            functions,
            module,
            func_ids: HashMap::default(),
            string_ids: HashMap::default(),
            _std_lib: std_lib,
        }
    }

    pub fn generate(&mut self) {
        let needed = collect_needed_imports(self.functions);

        let mk_sig = |params: &[cranelift::prelude::Type], returns: &[cranelift::prelude::Type]| {
            let mut sig = self.module.make_signature();
            for &p in params {
                sig.params.push(AbiParam::new(p));
            }
            for &r in returns {
                sig.returns.push(AbiParam::new(r));
            }
            sig
        };

        let sig_i64_i64 = mk_sig(&[types::I64], &[types::I64]);
        let sig_f64_i64 = mk_sig(&[types::F64], &[types::I64]);
        let sig_i64_f64 = mk_sig(&[types::I64], &[types::F64]);
        let sig_f64_f64 = mk_sig(&[types::F64], &[types::F64]);
        let sig_void_i64 = mk_sig(&[], &[types::I64]);
        let sig_void_f64 = mk_sig(&[], &[types::F64]);
        let sig_f64_void = mk_sig(&[types::F64], &[]);
        let sig_i64_void = mk_sig(&[types::I64], &[]);
        let sig_i64_i64_i64 = mk_sig(&[types::I64, types::I64], &[types::I64]);
        let sig_i64_i64_void = mk_sig(&[types::I64, types::I64], &[]);
        let sig_f64_f64_f64 = mk_sig(&[types::F64, types::F64], &[types::F64]);
        let sig_i64_i64_i64_i64 = mk_sig(&[types::I64, types::I64, types::I64], &[types::I64]);
        let sig_i64_i64_i64_void = mk_sig(&[types::I64, types::I64, types::I64], &[]);

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
            ("__olive_list_len", &sig_i64_i64),
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
            ("__olive_list_concat", &sig_i64_i64_i64),
            ("__olive_str_eq", &sig_i64_i64_i64),
            ("__olive_list_get", &sig_i64_i64_i64),
            ("__olive_get_index_any", &sig_i64_i64_i64),
            ("__olive_str_get", &sig_i64_i64_i64),
            ("__olive_cache_get", &sig_i64_i64_i64),
            ("__olive_cache_has", &sig_i64_i64_i64),
            ("__olive_obj_get", &sig_i64_i64_i64),
            ("__olive_memo_get", &sig_i64_i64_i64),
            ("__olive_cache_has_tuple", &sig_i64_i64_i64),
            ("__olive_cache_get_tuple", &sig_i64_i64_i64),
            ("__olive_list_set", &sig_i64_i64_i64_i64),
            ("__olive_obj_set", &sig_i64_i64_i64_i64),
            ("__olive_set_index_any", &sig_i64_i64_i64_i64),
            ("__olive_cache_set", &sig_i64_i64_i64_i64),
            ("__olive_cache_set_tuple", &sig_i64_i64_i64_i64),
            ("__olive_list_concat", &sig_i64_i64_i64),
            ("__olive_free", &sig_i64_void),
            ("__olive_free_str", &sig_i64_void),
            ("__olive_free_list", &sig_i64_void),
            ("__olive_free_obj", &sig_i64_void),
            ("__olive_free_any", &sig_i64_void),
            ("__olive_pow", &sig_i64_i64_i64),
            ("__olive_pow_float", &sig_f64_f64_f64),
            ("__olive_in_list", &sig_i64_i64_i64),
            ("__olive_in_obj", &sig_i64_i64_i64),
            ("__olive_list_append", &sig_i64_i64_void),
            ("__olive_set_add", &sig_i64_i64_void),
            ("__olive_set_new", &sig_i64_i64),
            ("__olive_iter", &sig_i64_i64),
            ("__olive_next", &sig_i64_i64),
            ("__olive_has_next", &sig_i64_i64),
            ("__olive_time_now", &sig_void_f64),
            ("__olive_time_sleep", &sig_f64_void),
            ("__olive_enum_new", &sig_i64_i64_i64_i64),
            ("__olive_enum_tag", &sig_i64_i64),
            ("__olive_enum_type_id", &sig_i64_i64),
            ("__olive_enum_get", &sig_i64_i64_i64),
            ("__olive_enum_set", &sig_i64_i64_i64_void),
            ("__olive_free_enum", &sig_i64_void),
            ("__olive_str_char", &sig_i64_i64_i64),
            ("__olive_str_slice", &sig_i64_i64_i64_i64),
            ("__olive_obj_len", &sig_i64_i64),
            ("__olive_make_future", &sig_i64_i64),
            ("__olive_await", &sig_i64_i64),
            ("__olive_spawn_task", &sig_i64_i64),
            ("__olive_free_future", &sig_i64_i64),
            ("__olive_alloc", &sig_i64_i64),
            ("__olive_async_file_read", &sig_i64_i64),
            ("__olive_async_file_write", &sig_i64_i64_i64),
            ("__olive_gather", &sig_i64_i64),
            ("__olive_select", &sig_i64_i64),
            ("__olive_cancel_future", &sig_i64_i64),
            ("__olive_sm_poll", &sig_i64_i64),
            ("__olive_random_seed", &sig_i64_void),
            ("__olive_random_get", &sig_void_f64),
            ("__olive_random_int", &sig_i64_i64_i64),
            ("__olive_math_sin", &sig_f64_f64),
            ("__olive_math_cos", &sig_f64_f64),
            ("__olive_math_tan", &sig_f64_f64),
            ("__olive_math_asin", &sig_f64_f64),
            ("__olive_math_acos", &sig_f64_f64),
            ("__olive_math_atan", &sig_f64_f64),
            ("__olive_math_atan2", &sig_f64_f64_f64),
            ("__olive_math_log", &sig_f64_f64),
            ("__olive_math_log10", &sig_f64_f64),
            ("__olive_math_exp", &sig_f64_f64),
            ("__olive_net_tcp_connect", &sig_i64_i64),
            ("__olive_net_tcp_send", &sig_i64_i64_i64),
            ("__olive_net_tcp_recv", &sig_i64_i64_i64),
            ("__olive_net_tcp_close", &sig_i64_void),
            ("__olive_http_get", &sig_i64_i64),
            ("__olive_http_post", &sig_i64_i64_i64),
        ];

        let has_async = self.functions.iter().any(|f| f.is_async);

        for &(name, sig) in imports {
            let always_needed = matches!(
                name,
                "__olive_make_future"
                    | "__olive_await"
                    | "__olive_spawn_task"
                    | "__olive_alloc"
                    | "__olive_free_future"
                    | "__olive_sm_poll"
            );
            if !(needed.contains(name) || always_needed && has_async) {
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

            if func.is_async {
                let can_sm = Self::analyze_async_sm(func).is_some();
                if can_sm {
                    // poll func
                    let poll_name = format!("{}__sm_poll", func.name);
                    let mut poll_sig = self.module.make_signature();
                    poll_sig.params.push(AbiParam::new(types::I64));
                    poll_sig.returns.push(AbiParam::new(types::I64));
                    let poll_id = self
                        .module
                        .declare_function(&poll_name, Linkage::Local, &poll_sig)
                        .unwrap();
                    self.func_ids.insert(poll_name, poll_id);
                } else {
                    // thread fallback
                    let body_name = format!("{}__async_body", func.name);
                    let body_id = self
                        .module
                        .declare_function(&body_name, Linkage::Local, &sig)
                        .unwrap();
                    self.func_ids.insert(body_name, body_id);
                }
                // future wrapper
                let wrapper_id = self
                    .module
                    .declare_function(&func.name, Linkage::Export, &sig)
                    .unwrap();
                self.func_ids.insert(func.name.clone(), wrapper_id);
            } else {
                let func_id = self
                    .module
                    .declare_function(&func.name, Linkage::Export, &sig)
                    .unwrap();
                self.func_ids.insert(func.name.clone(), func_id);
            }
        }

        for func in self.functions {
            self.collect_strings(func);
        }

        let func_count = self.functions.len();
        for i in 0..func_count {
            let func = self.functions[i].clone();
            if func.is_async {
                if let Some(await_points) = Self::analyze_async_sm(&func) {
                    // state machine
                    self.translate_async_sm_poll(&func, &await_points);
                    self.generate_sm_wrapper(&func);
                } else {
                    // thread fallback
                    let mut body_func = func.clone();
                    body_func.name = format!("{}__async_body", func.name);
                    body_func.is_async = false;
                    self.translate_function(&body_func);
                    self.generate_async_wrapper(&func);
                }
            } else {
                self.translate_function(&func);
            }
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
        if let Operand::Constant(Constant::Str(s)) = op
            && !self.string_ids.contains_key(s)
        {
            let mut data_ctx = DataDescription::new();
            let mut bytes = s.as_bytes().to_vec();
            bytes.push(0);
            // pad for tagging
            if bytes.len() % 2 != 0 {
                bytes.push(0);
            }
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

fn scan_rvalue_imports(
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

fn map_builtin_to_runtime(name: &str, arg_ty: &OliveType) -> Option<&'static str> {
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

fn is_list_op(func_mir: &MirFunction, op: &Operand) -> bool {
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

impl<'a> CraneliftCodegen<'a> {
    pub fn finalize(&mut self) {
        self.module.finalize_definitions().unwrap();
    }

    pub fn get_function(&mut self, name: &str) -> Option<*const u8> {
        let func_id = self.func_ids.get(name)?;
        Some(self.module.get_finalized_function(*func_id))
    }

    // Async state machine analysis
    fn analyze_async_sm(func: &MirFunction) -> Option<Vec<SmAwaitPoint>> {
        let n_bbs = func.basic_blocks.len();
        let mut visited = vec![false; n_bbs];

        let mut queue = std::collections::VecDeque::new();
        queue.push_back(0usize);
        let mut order = Vec::new();
        while let Some(bb_idx) = queue.pop_front() {
            if visited[bb_idx] {
                continue;
            }
            visited[bb_idx] = true;
            order.push(bb_idx);
            let bb = &func.basic_blocks[bb_idx];
            if let Some(term) = &bb.terminator {
                match &term.kind {
                    TerminatorKind::Goto { target } => queue.push_back(target.0),
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        for (_, t) in targets {
                            queue.push_back(t.0);
                        }
                        queue.push_back(otherwise.0);
                    }
                    _ => {}
                }
            }
        }
        let mut points = Vec::new();
        for bb_idx in order {
            let bb = &func.basic_blocks[bb_idx];
            for (stmt_idx, stmt) in bb.statements.iter().enumerate() {
                if let StatementKind::Assign(
                    result_local,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(name)),
                        args,
                    },
                ) = &stmt.kind
                    && name == "__olive_await"
                    && let Some(sub_op) = args.first()
                {
                    let sub_local = match sub_op {
                        Operand::Copy(l) | Operand::Move(l) => *l,
                        _ => return None,
                    };
                    points.push(SmAwaitPoint {
                        bb_idx,
                        stmt_idx,
                        result_local: *result_local,
                        sub_future_local: sub_local,
                    });
                }
            }
        }
        if points.is_empty() {
            None
        } else {
            Some(points)
        }
    }

    // generate sm poll function
    // frame: [state, future, locals...]
    fn translate_async_sm_poll(&mut self, func: &MirFunction, await_points: &[SmAwaitPoint]) {
        let poll_name = format!("{}__sm_poll", func.name);
        let poll_id = *self.func_ids.get(&poll_name).unwrap();
        let num_locals = func.locals.len();
        let n_awaits = await_points.len();
        let n_bbs = func.basic_blocks.len();
        let mf = MemFlags::trusted();

        // frame layout
        let frame_off = |local: Local| -> i32 { ((local.0 + 2) * 8) as i32 };

        let mut ctx = self.module.make_context();
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.returns.push(AbiParam::new(types::I64));
        let mut bctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);

        // declare ssa vars
        let mut vars: HashMap<Local, Variable> = HashMap::default();
        for (i, decl) in func.locals.iter().enumerate() {
            vars.insert(Local(i), builder.declare_var(cl_type(&decl.ty)));
        }
        let frame_var = builder.declare_var(types::I64);

        // map awaits to blocks
        let mut bb_awaits: Vec<Vec<usize>> = vec![Vec::new(); n_bbs];
        for (idx, ap) in await_points.iter().enumerate() {
            bb_awaits[ap.bb_idx].push(idx);
        }
        // segment count
        let n_segs: Vec<usize> = (0..n_bbs).map(|i| bb_awaits[i].len() + 1).collect();

        // create blocks
        let entry_blk = builder.create_block();
        let dispatch_blk = builder.create_block();
        let done_blk = builder.create_block();
        let state_blks: Vec<Block> = (0..=n_awaits).map(|_| builder.create_block()).collect();
        // block segments
        // segments
        let seg_blks: Vec<Vec<Block>> = (0..n_bbs)
            .map(|i| (0..n_segs[i]).map(|_| builder.create_block()).collect())
            .collect();

        // entry
        builder.switch_to_block(entry_blk);
        builder.seal_block(entry_blk);
        builder.append_block_params_for_function_params(entry_blk);
        let frame_ptr = builder.block_params(entry_blk)[0];
        for (i, decl) in func.locals.iter().enumerate() {
            let ty = cl_type(&decl.ty);
            let z = if ty == types::F64 {
                builder.ins().f64const(0.0)
            } else {
                builder.ins().iconst(types::I64, 0)
            };
            builder.def_var(vars[&Local(i)], z);
        }
        builder.def_var(frame_var, frame_ptr);
        // Jump to state
        builder.switch_to_block(dispatch_blk);

        let frame_d = builder.use_var(frame_var);
        let state_val = builder.ins().load(types::I64, mf, frame_d, 0);
        let mut sw = cranelift::frontend::Switch::new();
        for (k, &blk) in state_blks.iter().enumerate() {
            sw.set_entry(k as u128, blk);
        }
        sw.emit(&mut builder, state_val, done_blk);
        for blk in &state_blks {
            builder.seal_block(*blk);
        }
        builder.seal_block(done_blk);

        // state 0: load params
        builder.switch_to_block(state_blks[0]);
        {
            let frame_s = builder.use_var(frame_var);
            for i in 1..=func.arg_count {
                let local = Local(i);
                let ty = cl_type(&func.locals[i].ty);
                let val = builder.ins().load(ty, mf, frame_s, frame_off(local));
                builder.def_var(vars[&local], val);
            }
        }
        builder.ins().jump(seg_blks[0][0], &[]);

        // handle awaits
        for k in 1..=n_awaits {
            let ap = &await_points[k - 1];
            let seg_idx_in_bb = bb_awaits[ap.bb_idx]
                .iter()
                .position(|&i| i == k - 1)
                .unwrap();
            let resume_seg = seg_idx_in_bb + 1;

            builder.switch_to_block(state_blks[k]);
            {
                let frame_s = builder.use_var(frame_var);
                for i in 0..num_locals {
                    let local = Local(i);
                    let ty = cl_type(&func.locals[i].ty);
                    let val = builder.ins().load(ty, mf, frame_s, frame_off(local));
                    builder.def_var(vars[&local], val);
                }
                // load sub-future
                let sub_future = builder.ins().load(types::I64, mf, frame_s, 8);
                let sm_poll_id = *self.func_ids.get("__olive_sm_poll").unwrap();
                let sm_poll_ref = self.module.declare_func_in_func(sm_poll_id, builder.func);
                let poll_call = builder.ins().call(sm_poll_ref, &[sub_future]);
                let poll_result = builder.inst_results(poll_call)[0];

                let pend_c = builder.ins().iconst(types::I64, POLL_PENDING);
                let is_pend = builder.ins().icmp(IntCC::Equal, poll_result, pend_c);

                let pend_blk = builder.create_block();
                let cont_blk = builder.create_block();
                builder.ins().brif(is_pend, pend_blk, &[], cont_blk, &[]);

                builder.seal_block(pend_blk);
                builder.switch_to_block(pend_blk);
                builder.ins().return_(&[pend_c]);

                // Resume state machine
                builder.seal_block(cont_blk);
                builder.switch_to_block(cont_blk);
                let frame_c = builder.use_var(frame_var);
                for i in 0..num_locals {
                    let local = Local(i);
                    let ty = cl_type(&func.locals[i].ty);
                    let val = builder.ins().load(ty, mf, frame_c, frame_off(local));
                    builder.def_var(vars[&local], val);
                }

                // store result
                builder.def_var(vars[&ap.result_local], poll_result);
            }
            builder.ins().jump(seg_blks[ap.bb_idx][resume_seg], &[]);
        }

        // segments
        for bb_i in 0..n_bbs {
            let bb = func.basic_blocks[bb_i].clone();
            for seg_j in 0..n_segs[bb_i] {
                builder.switch_to_block(seg_blks[bb_i][seg_j]);

                // stmt range
                let start_stmt = if seg_j == 0 {
                    0
                } else {
                    await_points[bb_awaits[bb_i][seg_j - 1]].stmt_idx + 1
                };
                let (end_stmt, maybe_ap_idx) = if seg_j < bb_awaits[bb_i].len() {
                    let ap_idx = bb_awaits[bb_i][seg_j];
                    (await_points[ap_idx].stmt_idx, Some(ap_idx))
                } else {
                    (bb.statements.len(), None)
                };

                for stmt in &bb.statements[start_stmt..end_stmt] {
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

                if let Some(ap_idx) = maybe_ap_idx {
                    // yield point: save state
                    let ap = &await_points[ap_idx];
                    let frame_sw = builder.use_var(frame_var);
                    for i in 0..num_locals {
                        let local = Local(i);
                        let val = builder.use_var(vars[&local]);
                        builder.ins().store(mf, val, frame_sw, frame_off(local));
                    }

                    let sub_fv = builder.use_var(vars[&ap.sub_future_local]);
                    builder.ins().store(mf, sub_fv, frame_sw, 8);
                    let new_st = builder.ins().iconst(types::I64, (ap_idx + 1) as i64);
                    builder.ins().store(mf, new_st, frame_sw, 0);
                    let pv = builder.ins().iconst(types::I64, POLL_PENDING);
                    builder.ins().return_(&[pv]);
                } else {
                    // terminator
                    match bb.terminator.as_ref().map(|t| t.kind.clone()) {
                        Some(TerminatorKind::Return) => {
                            let ret_val = builder.use_var(vars[&Local(0)]);
                            let frame_r = builder.use_var(frame_var);
                            let done_s = builder.ins().iconst(types::I64, -1i64);
                            builder.ins().store(mf, done_s, frame_r, 0);
                            builder.ins().return_(&[ret_val]);
                        }
                        Some(TerminatorKind::Goto { target }) => {
                            builder.ins().jump(seg_blks[target.0][0], &[]);
                        }
                        Some(TerminatorKind::SwitchInt {
                            discr,
                            targets,
                            otherwise,
                        }) => {
                            let val = Self::translate_operand(
                                &mut builder,
                                &discr,
                                &vars,
                                &self.string_ids,
                                &mut self.module,
                            );
                            if targets.len() == 1 && targets[0].0 == 1 {
                                let cond = builder.ins().icmp_imm(IntCC::NotEqual, val, 0);
                                builder.ins().brif(
                                    cond,
                                    seg_blks[targets[0].1.0][0],
                                    &[],
                                    seg_blks[otherwise.0][0],
                                    &[],
                                );
                            } else {
                                let mut sw2 = cranelift::frontend::Switch::new();
                                for (v, t) in &targets {
                                    sw2.set_entry(*v as u128, seg_blks[t.0][0]);
                                }
                                sw2.emit(&mut builder, val, seg_blks[otherwise.0][0]);
                            }
                        }
                        Some(TerminatorKind::Unreachable) | None => {
                            builder.ins().trap(TrapCode::unwrap_user(1));
                        }
                    }
                }
            }
        }

        // done
        builder.switch_to_block(done_blk);
        let z = builder.ins().iconst(types::I64, 0);
        builder.ins().return_(&[z]);

        // seal segments
        for seg_row in &seg_blks {
            for &blk in seg_row {
                builder.seal_block(blk);
            }
        }

        builder.finalize();
        self.module.define_function(poll_id, &mut ctx).unwrap();
    }

    // Future allocation wrapper
    fn generate_sm_wrapper(&mut self, func: &MirFunction) {
        let poll_name = format!("{}__sm_poll", func.name);
        let poll_fn_id = *self.func_ids.get(&poll_name).unwrap();
        let num_locals = func.locals.len();
        // frame: [state, future, locals...]
        let frame_size = ((num_locals + 2) * 8) as i64;

        let mut ctx = self.module.make_context();
        for i in 0..func.arg_count {
            let ty = &func.locals[i + 1].ty;
            ctx.func.signature.params.push(AbiParam::new(cl_type(ty)));
        }
        ctx.func.signature.returns.push(AbiParam::new(types::I64));

        let mut bctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        builder.append_block_params_for_function_params(entry);
        let params: Vec<Value> = builder.block_params(entry).to_vec();

        let mf = MemFlags::trusted();
        let alloc_id = *self.func_ids.get("__olive_alloc").unwrap();
        let alloc_ref = self.module.declare_func_in_func(alloc_id, builder.func);

        // alloc frame
        let fsz = builder.ins().iconst(types::I64, frame_size);
        let frame_call = builder.ins().call(alloc_ref, &[fsz]);
        let frame_ptr = builder.inst_results(frame_call)[0];

        // initial state
        let zero = builder.ins().iconst(types::I64, 0);
        builder.ins().store(mf, zero, frame_ptr, 0);

        // store args
        for (i, &param) in params.iter().enumerate() {
            let offset = ((i + 3) * 8) as i32; // Local(i+1) at (i+1+2)*8
            builder.ins().store(mf, param, frame_ptr, offset);
        }

        // alloc future
        let future_sz = builder.ins().iconst(types::I64, 32);
        let fut_call = builder.ins().call(alloc_ref, &[future_sz]);
        let fut_ptr = builder.inst_results(fut_call)[0];

        let kind_val = builder.ins().iconst(types::I64, KIND_SM_FUTURE);
        builder.ins().store(mf, kind_val, fut_ptr, 0);

        let poll_ref = self.module.declare_func_in_func(poll_fn_id, builder.func);
        let poll_addr = builder.ins().func_addr(types::I64, poll_ref);
        builder.ins().store(mf, poll_addr, fut_ptr, 8);
        builder.ins().store(mf, frame_ptr, fut_ptr, 16);

        builder.ins().return_(&[fut_ptr]);
        builder.finalize();

        let wrapper_id = *self.func_ids.get(&func.name).unwrap();
        self.module.define_function(wrapper_id, &mut ctx).unwrap();
    }

    // Thread-based async wrapper
    fn generate_async_wrapper(&mut self, func: &MirFunction) {
        let body_name = format!("{}__async_body", func.name);
        let body_func_id = *self.func_ids.get(&body_name).unwrap();

        let mut ctx = self.module.make_context();
        for i in 0..func.arg_count {
            let ty = &func.locals[i + 1].ty;
            ctx.func.signature.params.push(AbiParam::new(cl_type(ty)));
        }
        ctx.func.signature.returns.push(AbiParam::new(types::I64));

        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);

        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        builder.append_block_params_for_function_params(entry);
        let params: Vec<Value> = builder.block_params(entry).to_vec();

        // callback layout
        let callback_size = 8i64 * (2 + func.arg_count as i64);

        let alloc_id = *self.func_ids.get("__olive_alloc").unwrap();
        let alloc_ref = self.module.declare_func_in_func(alloc_id, builder.func);
        let size_val = builder.ins().iconst(types::I64, callback_size);
        let call = builder.ins().call(alloc_ref, &[size_val]);
        let cb_ptr = builder.inst_results(call)[0];

        let body_ref = self.module.declare_func_in_func(body_func_id, builder.func);
        let fn_ptr_val = builder.ins().func_addr(types::I64, body_ref);
        let mf = MemFlags::new();
        builder.ins().store(mf, fn_ptr_val, cb_ptr, 0);

        let nargs_val = builder.ins().iconst(types::I64, func.arg_count as i64);
        builder.ins().store(mf, nargs_val, cb_ptr, 8);

        for (i, &arg) in params.iter().enumerate() {
            builder.ins().store(mf, arg, cb_ptr, 8 * (2 + i) as i32);
        }

        let spawn_id = *self.func_ids.get("__olive_spawn_task").unwrap();
        let spawn_ref = self.module.declare_func_in_func(spawn_id, builder.func);
        let call = builder.ins().call(spawn_ref, &[cb_ptr]);
        let future_ptr = builder.inst_results(call)[0];

        builder.ins().return_(&[future_ptr]);
        builder.finalize();

        let wrapper_id = *self.func_ids.get(&func.name).unwrap();
        self.module.define_function(wrapper_id, &mut ctx).unwrap();
    }

    fn translate_function(&mut self, func: &MirFunction) {
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

                // Type cast if needed
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
                    OliveType::Dict(_, _) | OliveType::Struct(_) => {
                        let set_id = func_ids.get("__olive_obj_set").unwrap();
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v]);
                    }
                    OliveType::Any => {
                        let set_id = func_ids.get("__olive_set_index_any").unwrap();
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v]);
                    }

                    OliveType::Enum(_) => {
                        let set_id = func_ids.get("__olive_enum_set").unwrap();
                        let local_func = module.declare_func_in_func(*set_id, builder.func);
                        builder.ins().call(local_func, &[o, i, v]);
                    }
                    _ => {
                        // List
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
                    OliveType::Dict(_, _) | OliveType::Struct(_) => "__olive_free_obj",
                    OliveType::Enum(_) => "__olive_free_enum",
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
                            if matches!(ty, OliveType::Dict(_, _) | OliveType::Struct(_)) {
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
                            if matches!(ty, OliveType::Dict(_, _) | OliveType::Struct(_)) {
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
                    OliveType::Enum(_) => {
                        let get_id = func_ids.get("__olive_enum_get").unwrap();
                        let local_func = module.declare_func_in_func(*get_id, builder.func);
                        let inst = builder.ins().call(local_func, &[o, i]);
                        builder.inst_results(inst)[0]
                    }
                    OliveType::Dict(_, _) | OliveType::Struct(_) => {
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

    #[allow(clippy::too_many_arguments)]
    fn translate_terminator(
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
