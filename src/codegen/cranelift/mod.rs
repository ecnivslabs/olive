mod async_sm;
mod imports;
mod translate;

use crate::mir::{Constant, Local, MirFunction, Operand, Rvalue, StatementKind};
use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use rustc_hash::FxHashMap as HashMap;

pub(super) const KIND_SM_FUTURE: i64 = 5;
pub(super) const POLL_PENDING: i64 = i64::MIN;

pub(super) struct SmAwaitPoint {
    pub(super) bb_idx: usize,
    pub(super) stmt_idx: usize,
    pub(super) result_local: Local,
    pub(super) sub_future_local: Local,
}

pub struct CraneliftCodegen<'a> {
    pub(super) functions: &'a [MirFunction],
    pub(super) module: JITModule,
    pub(super) func_ids: HashMap<String, FuncId>,
    pub(super) string_ids: HashMap<String, DataId>,
    pub(super) _std_lib: Option<libloading::Library>,
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
        let needed = imports::collect_needed_imports(self.functions);

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

        let import_table: &[(&str, &cranelift::prelude::Signature)] = &[
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

        for &(name, sig) in import_table {
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
                sig.params.push(AbiParam::new(imports::cl_type(ty)));
            }
            let ret_ty = &func.locals[0].ty;
            sig.returns.push(AbiParam::new(imports::cl_type(ret_ty)));

            if func.is_async {
                let can_sm = Self::analyze_async_sm(func).is_some();
                if can_sm {
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
                    let body_name = format!("{}__async_body", func.name);
                    let body_id = self
                        .module
                        .declare_function(&body_name, Linkage::Local, &sig)
                        .unwrap();
                    self.func_ids.insert(body_name, body_id);
                }
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
                    self.translate_async_sm_poll(&func, &await_points);
                    self.generate_sm_wrapper(&func);
                } else {
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

    pub fn finalize(&mut self) {
        self.module.finalize_definitions().unwrap();
    }

    pub fn get_function(&mut self, name: &str) -> Option<*const u8> {
        let func_id = self.func_ids.get(name)?;
        Some(self.module.get_finalized_function(*func_id))
    }
}
